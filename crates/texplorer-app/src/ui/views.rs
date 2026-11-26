use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::{
    Context, ExternalPaths, MouseButton, MouseDownEvent, ObjectFit, Render, SharedString, Window,
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
use crate::ui::workspace::Workspace;

/// Drag payload for internal file/folder drag & drop (into folders or across Dual-Pane).
#[derive(Clone)]
pub struct DraggedEntries {
    pub paths: Vec<PathBuf>,
    pub label: SharedString,
}

struct DragBadgeView {
    label: SharedString,
    palette: Palette,
}

impl Render for DragBadgeView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        h_flex()
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
                                                        .text_size(px(11.))
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
                    h_flex()
                        .flex_wrap()
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
                                .w(px(268.))
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
                                                    .text_size(px(10.))
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
                                                .text_size(px(11.))
                                                .text_color(solid(p.text_muted))
                                                .child(space_label),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .font_medium()
                                                .text_color(solid(p.text_secondary))
                                                .child(pct_label),
                                        ),
                                )
                        })),
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
    let pane_dir = model.location.as_local_path().map(|path| path.to_path_buf());
    let list_id = match side {
        PaneSide::Left => "details-list-left",
        PaneSide::Right => "details-list-right",
    };

    let pane_dir_int = pane_dir.clone();
    let pane_dir_ext = pane_dir;

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .drag_over::<DraggedEntries>(move |style, _, _, _| {
            style.border_1().border_color(solid(p.accent_primary))
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_1().border_color(solid(p.accent_primary))
        })
        .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
            if let Some(ref dest) = pane_dir_int {
                this.handle_dropped_paths(drag.paths.clone(), dest.clone(), false, window, cx);
            }
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
            if let Some(ref dest) = pane_dir_ext {
                this.handle_dropped_paths(ext.paths().to_vec(), dest.clone(), true, window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev.position, cx);
                }
            }),
        )
        .child(
            h_flex()
                .w_full()
                .h(px(30.))
                .px_2p5()
                .flex_shrink_0()
                .items_center()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
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
                .child(sort_header_cell(
                    side,
                    "Date Modified",
                    SortColumn::Modified,
                    sort,
                    Some(142.),
                    false,
                    p,
                    cx,
                ))
                .child(sort_header_cell(
                    side,
                    "Type",
                    SortColumn::Type,
                    sort,
                    Some(126.),
                    false,
                    p,
                    cx,
                ))
                .child(sort_header_cell(
                    side,
                    "Size",
                    SortColumn::Size,
                    sort,
                    Some(92.),
                    true,
                    p,
                    cx,
                )),
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
    let p = ws.palette();
    let unit = ws.settings.size_unit;

    let Some(tab) = ws.active_tab() else {
        return Vec::new();
    };
    let model = tab.model.pane(side);

    range
        .filter_map(|vis_idx| {
            let entry = model.visible_entry(vis_idx)?;
            let selected = model.selection.contains(vis_idx);
            let is_cursor = model.selection.cursor == Some(vis_idx);
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
            let drop_dir_int = if is_dir { local_path.clone() } else { None };
            let drop_dir_ext = drop_dir_int.clone();

            let row = h_flex()
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
                    el.on_drag(payload, move |drag, _, _, cx| {
                        cx.new(|_| DragBadgeView {
                            label: drag.label.clone(),
                            palette: p,
                        })
                    })
                })
                .when_some(drop_dir_int, |el, dest| {
                    el.drag_over::<DraggedEntries>(move |style, _, _, _| {
                        style.bg(soft_tint(p.accent_primary, 0.22))
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
                        .pr_2()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(icon).small().text_color(if is_hidden || is_cut {
                                solid(p.text_muted)
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
                .child(
                    div()
                        .w(px(142.))
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .text_color(solid(p.text_secondary))
                        .child(modified),
                )
                .child(
                    div()
                        .w(px(126.))
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child(type_label),
                )
                .child(
                    div()
                        .w(px(92.))
                        .flex_shrink_0()
                        .text_right()
                        .truncate()
                        .text_xs()
                        .text_color(solid(p.text_secondary))
                        .child(size_label),
                );

            Some(row.into_any_element())
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
    let cols = if tab.model.split_enabled { 4usize } else { 6usize };
    let row_count = total.div_ceil(cols);
    let list_id = match side {
        PaneSide::Left => "grid-list-left",
        PaneSide::Right => "grid-list-right",
    };
    let pane_dir_int = pane_dir.clone();
    let pane_dir_ext = pane_dir;

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .drag_over::<DraggedEntries>(move |style, _, _, _| {
            style.border_1().border_color(solid(p.accent_primary))
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_1().border_color(solid(p.accent_primary))
        })
        .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
            if let Some(ref dest) = pane_dir_int {
                this.handle_dropped_paths(drag.paths.clone(), dest.clone(), false, window, cx);
            }
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
            if let Some(ref dest) = pane_dir_ext {
                this.handle_dropped_paths(ext.paths().to_vec(), dest.clone(), true, window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev.position, cx);
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

    // Resolve or enqueue native Windows Shell thumbnails strictly for the visible viewport range.
    let mut thumbs: Vec<Option<std::sync::Arc<gpui_kit::RenderImage>>> =
        Vec::with_capacity(visible_items.len());
    for (_, entry, _) in &visible_items {
        let thumb = if !entry.is_dir()
            && let Some(local) = entry.location.as_local_path()
        {
            ws.get_or_request_thumbnail(local, entry.modified_secs, 96)
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
                .h(px(122.))
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
                    let drop_dir_int = if is_dir { local_path.clone() } else { None };
                    let drop_dir_ext = drop_dir_int.clone();

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
                                el.on_drag(payload, move |drag, _, _, cx| {
                                    cx.new(|_| DragBadgeView {
                                        label: drag.label.clone(),
                                        palette: p,
                                    })
                                })
                            })
                            .when_some(drop_dir_int, |el, dest| {
                                el.drag_over::<DraggedEntries>(move |style, _, _, _| {
                                    style.border_color(solid(p.accent_primary))
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
                                    .text_size(px(10.))
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
    let pane_dir_int = pane_dir.clone();
    let pane_dir_ext = pane_dir;

    v_flex()
        .size_full()
        .bg(solid(p.bg_base))
        .overflow_hidden()
        .drag_over::<DraggedEntries>(move |style, _, _, _| {
            style.border_1().border_color(solid(p.accent_primary))
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_1().border_color(solid(p.accent_primary))
        })
        .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
            if let Some(ref dest) = pane_dir_int {
                this.handle_dropped_paths(drag.paths.clone(), dest.clone(), false, window, cx);
            }
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
            if let Some(ref dest) = pane_dir_ext {
                this.handle_dropped_paths(ext.paths().to_vec(), dest.clone(), true, window, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                if !window.default_prevented() {
                    this.open_pane_bg_context_menu(side, ev.position, cx);
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
    let p = ws.palette();
    let unit = ws.settings.size_unit;

    let Some(tab) = ws.active_tab() else {
        return Vec::new();
    };
    let model = tab.model.pane(side);

    range
        .filter_map(|vis_idx| {
            let entry = model.visible_entry(vis_idx)?;
            let selected = model.selection.contains(vis_idx);
            let (icon, icon_hex) = file_kind_visual(entry.kind, p);
            let local_path = entry.location.as_local_path().map(|path| path.to_path_buf());
            let is_cut = local_path.as_ref().is_some_and(|path| ws.is_path_cut(path));
            let name: SharedString = entry.name.clone().into();
            let size: SharedString = entry
                .size
                .map(|b| format_size(b, unit))
                .unwrap_or_default()
                .into();
            let drag_payload = build_drag_payload(ws, side, local_path.as_ref(), &entry.name);

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
                        el.on_drag(payload, move |drag, _, _, cx| {
                            cx.new(|_| DragBadgeView {
                                label: drag.label.clone(),
                                palette: p,
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
                    .child(Icon::new(icon).xsmall().text_color(if is_cut {
                        solid(p.text_muted)
                    } else {
                        solid(icon_hex)
                    }))
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
                            .text_size(px(11.))
                            .text_color(solid(p.text_muted))
                            .child(size),
                    )
                    .into_any_element(),
            )
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
            solid(p.text_primary)
        } else {
            solid(p.text_secondary)
        })
        .hover(move |s| s.text_color(solid(p.text_primary)))
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
