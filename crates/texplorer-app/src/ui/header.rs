use gpui_kit::{
    Context, MouseButton, MouseDownEvent, SharedString, Window, WindowControlArea,
    assets::IconName as LucideIcon,
    component::{
        Icon, IconName, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::Location;

use crate::theme::{
    BG_BASE, BG_ELEVATED, BG_HOVER, BORDER_SUBTLE, RADIUS_CONTROL, STATUS_ERROR, TEXT_MUTED,
    TEXT_PRIMARY, TEXT_SECONDARY, soft_tint, solid,
};
use crate::ui::workspace::Workspace;
use crate::updater::UpdateState;

/// Renders the 38px integrated frameless titlebar with optional brand text (`TExplorer`),
/// optional scrollable tabs, a dedicated `WindowControlArea::Drag` spacer, update indicator,
/// layout controls, and native window controls.
pub fn render_header(
    ws: &Workspace,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let accent = ws.settings.accent;
    let active_tab = ws.active_tab_index;
    let split_active = ws.active_tab().map(|t| t.model.split_enabled).unwrap_or(false);
    let show_tabs = ws.settings.show_tabs;
    let show_brand = ws.settings.show_brand_title;

    let update_pill: Option<SharedString> = match &ws.update_state {
        UpdateState::Available(info) => Some(format!("Update {}", info.tag_name).into()),
        UpdateState::Downloading { info, .. } => Some(format!("Downloading {}...", info.tag_name).into()),
        UpdateState::Installing(info) => Some(format!("Installing {}...", info.tag_name).into()),
        _ => None,
    };

    h_flex()
        .id("app-header-bar")
        .w_full()
        .h(px(38.))
        .flex_shrink_0()
        .items_center()
        .bg(solid(BG_ELEVATED))
        .border_b_1()
        .border_color(solid(BORDER_SUBTLE))
        // Minimal text-only brand area without emblem icon (toggleable in Settings)
        .when(show_brand, |el| {
            el.child(
                h_flex()
                    .h_full()
                    .pl_3()
                    .pr_2()
                    .items_center()
                    .flex_shrink_0()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(solid(TEXT_PRIMARY))
                            .child("TExplorer"),
                    ),
            )
        })
        // Sidebar toggle (sibling outside Drag area so clicks always register)
        .child(
            div()
                .flex_shrink_0()
                .map(|d| if show_brand { d.pr_1p5() } else { d.pl_2().pr_1p5() })
                .child(
                    Button::new("hdr-toggle-sidebar")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(IconName::PanelLeft)
                        .tooltip("Toggle Sidebar (Ctrl+B)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_sidebar(cx);
                        })),
                ),
        )
        // Optional scrollable tab strip + new tab button
        .when(show_tabs, |el| {
            el.child(
                h_flex()
                    .id("header-tab-strip")
                    .flex_initial()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap_1()
                    .overflow_x_scroll()
                    .children(ws.tabs.iter().enumerate().map(|(idx, tab)| {
                        let is_active = idx == active_tab;
                        let title: SharedString = tab.model.title().into();
                        let is_split = tab.model.split_enabled;
                        let tab_icon = match tab.model.active_pane_ref().location {
                            Location::Home => LucideIcon::House,
                            _ => LucideIcon::Folder,
                        };

                        h_flex()
                            .id(("tab-item", idx))
                            .h(px(28.))
                            .max_w(px(200.))
                            .min_w(px(96.))
                            .px_2()
                            .gap_1p5()
                            .items_center()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(if is_active {
                                solid(BG_BASE)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .border_1()
                            .border_color(if is_active {
                                solid(BORDER_SUBTLE)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .hover(move |s| {
                                if is_active {
                                    s
                                } else {
                                    s.bg(solid(BG_HOVER))
                                }
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.select_tab(idx, window, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.close_tab_at(idx, window, cx);
                                }),
                            )
                            .child(
                                Icon::new(tab_icon).xsmall().text_color(if is_active {
                                    solid(accent.primary_hex())
                                } else {
                                    solid(TEXT_MUTED)
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(if is_active {
                                        solid(TEXT_PRIMARY)
                                    } else {
                                        solid(TEXT_SECONDARY)
                                    })
                                    .child(title),
                            )
                            .when(is_split, |inner| {
                                inner.child(
                                    Icon::new(LucideIcon::Columns2)
                                        .xsmall()
                                        .text_color(solid(TEXT_MUTED)),
                                )
                            })
                            .child(
                                div()
                                    .id(("tab-close", idx))
                                    .w(px(16.))
                                    .h(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .text_color(solid(TEXT_MUTED))
                                    .hover(|s| {
                                        s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            window.prevent_default();
                                            this.close_tab_at(idx, window, cx);
                                        }),
                                    )
                                    .child(Icon::new(LucideIcon::X).xsmall()),
                            )
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .pl_1()
                    .child(
                        Button::new("hdr-new-tab")
                            .with_size(Size::XSmall)
                            .ghost()
                            .icon(LucideIcon::Plus)
                            .tooltip("New Tab (Ctrl+T)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_tab(Location::Home, window, cx);
                            })),
                    ),
            )
        })
        // Flexible draggable empty titlebar spacer
        .child(
            div()
                .flex_1()
                .min_w(px(36.))
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        // Right utility controls (outside Drag area)
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .pr_1p5()
                .when_some(update_pill, |el, label| {
                    el.child(
                        div()
                            .id("hdr-update-pill")
                            .h(px(24.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(soft_tint(accent.primary_hex(), 0.16))
                            .border_1()
                            .border_color(soft_tint(accent.primary_hex(), 0.35))
                            .text_size(px(11.))
                            .font_medium()
                            .text_color(solid(accent.primary_hex()))
                            .hover(move |s| s.bg(soft_tint(accent.primary_hex(), 0.24)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings_modal(window, cx);
                            }))
                            .child(Icon::new(LucideIcon::Download).xsmall())
                            .child(label),
                    )
                })
                .child(
                    div()
                        .id("hdr-toggle-split")
                        .h(px(26.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded(px(RADIUS_CONTROL))
                        .cursor_pointer()
                        .bg(if split_active {
                            soft_tint(accent.primary_hex(), 0.14)
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .text_color(if split_active {
                            solid(accent.primary_hex())
                        } else {
                            solid(TEXT_SECONDARY)
                        })
                        .hover(move |s| {
                            if split_active {
                                s
                            } else {
                                s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY))
                            }
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_split_active_tab(window, cx);
                        }))
                        .child(Icon::new(LucideIcon::Columns2).xsmall())
                        .child(div().text_xs().font_medium().child("Split")),
                )
                .child(
                    Button::new("hdr-toggle-inspector")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(IconName::PanelRight)
                        .tooltip("Toggle Preview & Inspector (Alt+P)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_inspector(cx);
                        })),
                )
                .child(
                    Button::new("hdr-open-settings")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(IconName::Settings)
                        .tooltip("Settings (Ctrl+,)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_settings_modal(window, cx);
                        })),
                ),
        )
        // Native Windows window controls flush at the right edge
        .child(render_window_controls(window))
}

fn render_window_controls(window: &Window) -> impl IntoElement {
    let max_icon = if window.is_maximized() {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };

    h_flex()
        .h_full()
        .flex_shrink_0()
        .items_center()
        .child(window_control_btn(
            "win-min",
            IconName::WindowMinimize,
            WindowControlArea::Min,
            false,
        ))
        .child(window_control_btn(
            "win-max",
            max_icon,
            WindowControlArea::Max,
            false,
        ))
        .child(window_control_btn(
            "win-close",
            IconName::WindowClose,
            WindowControlArea::Close,
            true,
        ))
}

fn window_control_btn(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    is_close: bool,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(36.))
        .h_full()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(solid(TEXT_MUTED))
        .hover(move |s| {
            if is_close {
                s.bg(solid(STATUS_ERROR)).text_color(solid(BG_BASE))
            } else {
                s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY))
            }
        })
        .window_control_area(area)
        .child(Icon::new(icon).small())
}
