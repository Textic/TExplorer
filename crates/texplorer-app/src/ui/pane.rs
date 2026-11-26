use gpui_kit::{
    Context, MouseButton, MouseDownEvent, SharedString,
    assets::IconName as LucideIcon,
    component::{
        Disableable as _, Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::Input,
        v_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::{Location, PaneSide, ViewMode};

use crate::theme::{
    BG_BASE, BG_ELEVATED, BG_HOVER, BG_SURFACE, BORDER, BORDER_SUBTLE, RADIUS_CONTROL, RADIUS_ROW,
    STATUS_ERROR, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY, format_size, full_hit_input, soft_tint,
    solid,
};
use crate::ui::views::{
    render_compact_view, render_details_view, render_grid_view, render_home_view,
};
use crate::ui::workspace::Workspace;

/// Renders one complete explorer pane (navigation bar, breadcrumbs/address input,
/// active view mode, and compact inline status footer).
pub fn render_pane(
    ws: &Workspace,
    side: PaneSide,
    is_split: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let Some(tab) = ws.active_tab() else {
        return v_flex().size_full().bg(solid(BG_BASE)).into_any_element();
    };
    let pane_ui = tab.pane_ui(side);
    let model = tab.model.pane(side);
    let is_active = !is_split || tab.model.active_pane == side;
    let accent = ws.settings.accent;
    let unit = ws.settings.size_unit;

    let can_back = model.can_go_back();
    let can_forward = model.can_go_forward();
    let can_up = model.can_go_up();
    let view_mode = model.view_mode;
    let show_hidden = model.show_hidden;
    let is_home = model.location == Location::Home;
    let total_visible = model.filtered_indices.len();
    let selected_count = model.selection.len();
    let selected_bytes: u64 = model
        .selected_entries()
        .into_iter()
        .filter_map(|e| e.size)
        .sum();
    let pane_error = model.error.clone();
    let is_loading = model.loading;
    let filter_non_empty = !model.quick_filter.trim().is_empty();
    let editing_address = pane_ui.editing_address;

    let side_tag = match side {
        PaneSide::Left => "left",
        PaneSide::Right => "right",
    };

    let address_bar = if editing_address {
        h_flex()
            .id((side_tag, 100usize))
            .flex_1()
            .min_w(px(140.))
            .h(px(28.))
            .px_2()
            .items_center()
            .rounded(px(RADIUS_ROW))
            .bg(solid(BG_BASE))
            .border_1()
            .border_color(solid(accent.primary_hex()))
            .child(
                full_hit_input(
                    (side_tag, 101usize),
                    &pane_ui.address_input,
                    Input::new(&pane_ui.address_input)
                        .with_size(Size::XSmall)
                        .appearance(false),
                )
                .flex_1()
                .min_w_0(),
            )
            .into_any_element()
    } else {
        let crumbs = model.location.breadcrumbs();
        let last_idx = crumbs.len().saturating_sub(1);

        h_flex()
            .id((side_tag, 102usize))
            .flex_1()
            .min_w(px(140.))
            .h(px(28.))
            .px_1p5()
            .gap_0p5()
            .items_center()
            .overflow_x_scroll()
            .rounded(px(RADIUS_ROW))
            .bg(solid(BG_BASE))
            .border_1()
            .border_color(solid(BORDER_SUBTLE))
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    if !window.default_prevented() {
                        this.begin_edit_address(side, window, cx);
                    }
                }),
            )
            .children(crumbs.into_iter().enumerate().map(|(i, seg)| {
                let is_last = i == last_idx;
                let target = seg.target;
                let label: SharedString = seg.label.into();

                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_0p5()
                    .when(i > 0, |el| {
                        el.child(
                            Icon::new(LucideIcon::ChevronRight)
                                .xsmall()
                                .text_color(solid(TEXT_MUTED)),
                        )
                    })
                    .child(
                        div()
                            .id((
                                match side {
                                    PaneSide::Left => "crumb-l",
                                    PaneSide::Right => "crumb-r",
                                },
                                i,
                            ))
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .text_xs()
                            .font_medium()
                            .text_color(if is_last {
                                solid(TEXT_PRIMARY)
                            } else {
                                solid(TEXT_SECONDARY)
                            })
                            .hover(|s| s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    window.prevent_default();
                                    this.navigate_pane(side, target.clone(), window, cx);
                                }),
                            )
                            .child(label),
                    )
            }))
            .into_any_element()
    };

    let body = if is_home {
        render_home_view(ws, side, cx).into_any_element()
    } else {
        match view_mode {
            ViewMode::Details => render_details_view(ws, side, cx).into_any_element(),
            ViewMode::Grid => render_grid_view(ws, side, cx).into_any_element(),
            ViewMode::Compact => render_compact_view(ws, side, cx).into_any_element(),
        }
    };

    v_flex()
        .id(match side {
            PaneSide::Left => "pane-root-left",
            PaneSide::Right => "pane-root-right",
        })
        .size_full()
        .bg(solid(BG_BASE))
        .overflow_hidden()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.set_active_pane(side, cx);
            }),
        )
        // Subtle 2px top accent indicator when Dual-Pane split is active
        .when(is_split, |el| {
            el.child(div().w_full().h(px(2.)).bg(if is_active {
                solid(accent.primary_hex())
            } else {
                solid(BORDER_SUBTLE)
            }))
        })
        // Pane Navigation Bar (38px flat strip)
        .child(
            h_flex()
                .w_full()
                .h(px(38.))
                .px_2()
                .gap_1p5()
                .flex_shrink_0()
                .items_center()
                .bg(solid(BG_SURFACE))
                .border_b_1()
                .border_color(solid(BORDER_SUBTLE))
                // History & Up navigation buttons
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_0p5()
                        .child(
                            Button::new((side_tag, 1usize))
                                .with_size(Size::XSmall)
                                .ghost()
                                .disabled(!can_back)
                                .icon(LucideIcon::ArrowLeft)
                                .tooltip("Back (Alt+Left)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.go_back(side, window, cx);
                                })),
                        )
                        .child(
                            Button::new((side_tag, 2usize))
                                .with_size(Size::XSmall)
                                .ghost()
                                .disabled(!can_forward)
                                .icon(LucideIcon::ArrowRight)
                                .tooltip("Forward (Alt+Right)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.go_forward(side, window, cx);
                                })),
                        )
                        .child(
                            Button::new((side_tag, 3usize))
                                .with_size(Size::XSmall)
                                .ghost()
                                .disabled(!can_up)
                                .icon(LucideIcon::ArrowUp)
                                .tooltip("Up (Alt+Up)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.go_up(side, window, cx);
                                })),
                        )
                        .child(
                            Button::new((side_tag, 4usize))
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::RefreshCw)
                                .tooltip("Refresh (F5)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.refresh_pane(side, window, cx);
                                })),
                        ),
                )
                // Breadcrumbs / Editable Address Input
                .child(address_bar)
                // Inline quick filter input (borderless inside single outer container)
                .when(!is_home, |el| {
                    el.child(
                        h_flex()
                            .w(px(164.))
                            .h(px(28.))
                            .px_2()
                            .gap_1()
                            .flex_shrink_0()
                            .items_center()
                            .rounded(px(RADIUS_ROW))
                            .bg(solid(BG_BASE))
                            .border_1()
                            .border_color(if filter_non_empty {
                                solid(accent.primary_hex())
                            } else {
                                solid(BORDER_SUBTLE)
                            })
                            .child(
                                Icon::new(LucideIcon::Search)
                                    .xsmall()
                                    .text_color(solid(TEXT_MUTED)),
                            )
                            .child(
                                full_hit_input(
                                    (side_tag, 103usize),
                                    &pane_ui.filter_input,
                                    Input::new(&pane_ui.filter_input)
                                        .with_size(Size::XSmall)
                                        .appearance(false),
                                )
                                .flex_1()
                                .min_w_0(),
                            )
                            .when(filter_non_empty, |inner| {
                                inner.child(
                                    div()
                                        .id((side_tag, 104usize))
                                        .w(px(16.))
                                        .h(px(16.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.))
                                        .cursor_pointer()
                                        .text_color(solid(TEXT_MUTED))
                                        .hover(|s| {
                                            s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                this.clear_pane_filter(side, window, cx);
                                            }),
                                        )
                                        .child(Icon::new(LucideIcon::X).xsmall()),
                                )
                            }),
                    )
                })
                // View mode segmented buttons & Hidden files toggle
                .when(!is_home, |el| {
                    el.child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_0p5()
                            .p_0p5()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(solid(BG_BASE))
                            .border_1()
                            .border_color(solid(BORDER_SUBTLE))
                            .child(view_mode_btn(
                                side,
                                ViewMode::Details,
                                view_mode == ViewMode::Details,
                                LucideIcon::LayoutList,
                                accent.primary_hex(),
                                cx,
                            ))
                            .child(view_mode_btn(
                                side,
                                ViewMode::Grid,
                                view_mode == ViewMode::Grid,
                                LucideIcon::LayoutGrid,
                                accent.primary_hex(),
                                cx,
                            ))
                            .child(view_mode_btn(
                                side,
                                ViewMode::Compact,
                                view_mode == ViewMode::Compact,
                                LucideIcon::List,
                                accent.primary_hex(),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .id((side_tag, 105usize))
                            .w(px(26.))
                            .h(px(26.))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(if show_hidden {
                                soft_tint(accent.primary_hex(), 0.14)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .text_color(if show_hidden {
                                solid(accent.primary_hex())
                            } else {
                                solid(TEXT_MUTED)
                            })
                            .hover(|s| s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_hidden_for_pane(side, cx);
                            }))
                            .child(
                                Icon::new(if show_hidden {
                                    LucideIcon::Eye
                                } else {
                                    LucideIcon::EyeOff
                                })
                                .xsmall(),
                            ),
                    )
                }),
        )
        // Main scrollable / virtualized body
        .child(div().flex_1().min_h_0().w_full().overflow_hidden().child(body))
        // Compact inline pane footer (only for directory views or error states)
        .when(!is_home || pane_error.is_some(), |el| {
            let summary: SharedString = if let Some(err) = pane_error {
                err.into()
            } else if is_loading {
                "Loading directory...".into()
            } else if selected_count > 0 {
                if selected_bytes > 0 {
                    format!(
                        "{total_visible} items  •  {selected_count} selected ({})",
                        format_size(selected_bytes, unit)
                    )
                    .into()
                } else {
                    format!("{total_visible} items  •  {selected_count} selected").into()
                }
            } else {
                format!("{total_visible} items").into()
            };

            let has_err = model.error.is_some();
            el.child(
                h_flex()
                    .w_full()
                    .h(px(24.))
                    .px_3()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .bg(solid(BG_SURFACE))
                    .border_t_1()
                    .border_color(solid(BORDER_SUBTLE))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(if has_err {
                                solid(STATUS_ERROR)
                            } else {
                                solid(TEXT_MUTED)
                            })
                            .child(summary),
                    )
                    .when(filter_non_empty, |inner| {
                        inner.child(
                            div()
                                .px_1p5()
                                .rounded(px(3.))
                                .bg(solid(BG_ELEVATED))
                                .border_1()
                                .border_color(solid(BORDER))
                                .text_size(px(10.))
                                .text_color(solid(TEXT_SECONDARY))
                                .child("Filtered"),
                        )
                    }),
            )
        })
        .into_any_element()
}

fn view_mode_btn(
    side: PaneSide,
    mode: ViewMode,
    active: bool,
    icon: LucideIcon,
    accent_hex: u32,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let id = match (side, mode) {
        (PaneSide::Left, ViewMode::Details) => "vm-l-details",
        (PaneSide::Left, ViewMode::Grid) => "vm-l-grid",
        (PaneSide::Left, ViewMode::Compact) => "vm-l-compact",
        (PaneSide::Right, ViewMode::Details) => "vm-r-details",
        (PaneSide::Right, ViewMode::Grid) => "vm-r-grid",
        (PaneSide::Right, ViewMode::Compact) => "vm-r-compact",
    };

    div()
        .id(id)
        .w(px(24.))
        .h(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .cursor_pointer()
        .bg(if active {
            soft_tint(accent_hex, 0.16)
        } else {
            gpui_kit::transparent_black()
        })
        .text_color(if active {
            solid(accent_hex)
        } else {
            solid(TEXT_MUTED)
        })
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY))
            }
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.set_pane_view_mode(side, mode, cx);
        }))
        .child(Icon::new(icon).xsmall())
}
