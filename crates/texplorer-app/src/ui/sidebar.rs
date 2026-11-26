use gpui_kit::{
    AnyElement, Context, MouseButton, MouseDownEvent, SharedString,
    assets::IconName as LucideIcon,
    component::{
        Icon, IconName, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, prelude::*, px, relative,
};
use texplorer_core::{Location, SidebarSection};

use crate::theme::{
    Palette, RADIUS_CONTROL, drive_kind_icon, known_folder_icon, solid, usage_bar_color,
};
use crate::ui::context_menu::ContextMenuTarget;
use crate::ui::workspace::Workspace;

/// Renders the flat left navigation rail containing Home and customizable, collapsible,
/// reorderable sections (Quick Access, Pinned folders, Drives, and Linux/WSL).
pub fn render_sidebar(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let p = ws.palette();
    let current_loc = ws
        .active_tab()
        .map(|t| t.model.active_pane_ref().location.clone())
        .unwrap_or(Location::Home);

    let is_home = current_loc == Location::Home;
    let visible_quick_access: Vec<_> = ws
        .settings
        .ordered_quick_access(&ws.known_folders)
        .into_iter()
        .filter(|f| ws.settings.is_quick_access_visible(f.kind))
        .collect();

    let ordered_sections: Vec<AnyElement> = ws
        .settings
        .normalized_sidebar_order()
        .into_iter()
        .filter(|&section| ws.settings.is_sidebar_section_visible(section))
        .filter_map(|section| {
            let collapsed = ws.settings.is_sidebar_section_collapsed(section);
            match section {
                SidebarSection::QuickAccess => {
                    if visible_quick_access.is_empty() {
                        return None;
                    }
                    Some(
                        render_quick_access_section(
                            &visible_quick_access,
                            &current_loc,
                            collapsed,
                            p,
                            cx,
                        )
                        .into_any_element(),
                    )
                }
                SidebarSection::Pinned => {
                    if ws.settings.pinned_folders.is_empty() {
                        return None;
                    }
                    Some(render_pinned_section(ws, &current_loc, collapsed, p, cx).into_any_element())
                }
                SidebarSection::Drives => {
                    Some(render_drives_section(ws, &current_loc, collapsed, p, cx).into_any_element())
                }
                SidebarSection::LinuxWsl => {
                    if ws.wsl_distros.is_empty() {
                        return None;
                    }
                    Some(render_wsl_section(ws, &current_loc, collapsed, p, cx).into_any_element())
                }
            }
        })
        .collect();

    v_flex()
        .id("explorer-sidebar")
        .size_full()
        .bg(solid(p.bg_surface))
        .overflow_hidden()
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .p_2()
                .gap_3()
                // Primary Home / This PC item
                .child(
                    div()
                        .id("sb-home")
                        .w_full()
                        .h(px(34.))
                        .px_2p5()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(RADIUS_CONTROL))
                        .cursor_pointer()
                        .bg(if is_home {
                            solid(p.bg_elevated)
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .border_1()
                        .border_color(if is_home {
                            solid(p.border_subtle)
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .hover(move |s| s.bg(solid(p.bg_elevated)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate_active_pane(Location::Home, window, cx);
                        }))
                        .child(Icon::new(LucideIcon::House).xsmall().text_color(
                            if is_home {
                                solid(p.accent_primary)
                            } else {
                                solid(p.text_secondary)
                            },
                        ))
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
                                .child("Home"),
                        ),
                )
                .children(ordered_sections),
        )
}

fn render_quick_access_section(
    visible_quick_access: &[texplorer_core::KnownFolder],
    current_loc: &Location,
    collapsed: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(collapsible_section_header(
            "sb-sec-quick-access",
            SidebarSection::QuickAccess,
            collapsed,
            p,
            None,
            cx,
        ))
        .when(!collapsed, |el| {
            el.children(visible_quick_access.iter().enumerate().map(|(i, folder)| {
                let target = Location::local(&folder.path);
                let active = *current_loc == target;
                let path_click = folder.path.clone();
                let path_mid = folder.path.clone();
                let folder_ctx = folder.clone();
                let icon = known_folder_icon(folder.kind);
                let label: SharedString = folder.name.clone().into();

                h_flex()
                    .id(("sb-known", i))
                    .w_full()
                    .h(px(32.))
                    .px_2p5()
                    .gap_2()
                    .items_center()
                    .rounded(px(RADIUS_CONTROL))
                    .cursor_pointer()
                    .bg(if active {
                        solid(p.bg_elevated)
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .hover(move |s| s.bg(solid(p.bg_elevated)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.navigate_active_pane(Location::local(&path_click), window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            let loc = Location::local(&path_mid);
                            if this.settings.show_tabs {
                                this.open_new_tab(loc, window, cx);
                            } else {
                                this.navigate_active_pane(loc, window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.open_context_menu(
                                ev.position,
                                ContextMenuTarget::KnownFolder(folder_ctx.clone()),
                                cx,
                            );
                        }),
                    )
                    .child(Icon::new(icon).xsmall().text_color(if active {
                        solid(p.accent_primary)
                    } else {
                        solid(p.text_secondary)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(if active {
                                solid(p.text_primary)
                            } else {
                                solid(p.text_secondary)
                            })
                            .child(label),
                    )
            }))
        })
}

fn render_pinned_section(
    ws: &Workspace,
    current_loc: &Location,
    collapsed: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(collapsible_section_header(
            "sb-sec-pinned",
            SidebarSection::Pinned,
            collapsed,
            p,
            None,
            cx,
        ))
        .when(!collapsed, |el| {
            el.children(
                ws.settings
                    .pinned_folders
                    .iter()
                    .enumerate()
                    .map(|(i, path)| {
                        let target = Location::local(path);
                        let active = *current_loc == target;
                        let name: SharedString = target.tab_title().into();
                        let path_open = path.clone();
                        let path_mid = path.clone();
                        let path_unpin = path.clone();

                        h_flex()
                            .id(("sb-pinned", i))
                            .w_full()
                            .h(px(32.))
                            .px_2p5()
                            .gap_2()
                            .items_center()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(if active {
                                solid(p.bg_elevated)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .hover(move |s| s.bg(solid(p.bg_elevated)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.navigate_active_pane(
                                        Location::local(&path_open),
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    let loc = Location::local(&path_mid);
                                    if this.settings.show_tabs {
                                        this.open_new_tab(loc, window, cx);
                                    } else {
                                        this.navigate_active_pane(loc, window, cx);
                                    }
                                }),
                            )
                            .child(Icon::new(LucideIcon::Pin).xsmall().text_color(if active {
                                solid(p.accent_primary)
                            } else {
                                solid(p.text_muted)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(if active {
                                        solid(p.text_primary)
                                    } else {
                                        solid(p.text_secondary)
                                    })
                                    .child(name),
                            )
                            .child(
                                div()
                                    .id(("sb-unpin", i))
                                    .w(px(18.))
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .text_color(solid(p.text_muted))
                                    .hover(move |s| {
                                        s.bg(solid(p.bg_hover))
                                            .text_color(solid(p.text_primary))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, _: &MouseDownEvent, window, cx| {
                                                window.prevent_default();
                                                this.unpin_folder(&path_unpin, cx);
                                            },
                                        ),
                                    )
                                    .child(Icon::new(LucideIcon::X).xsmall()),
                            )
                    }),
            )
        })
}

fn render_drives_section(
    ws: &Workspace,
    current_loc: &Location,
    collapsed: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let refresh_btn = Button::new("sb-refresh-drives")
        .with_size(Size::XSmall)
        .ghost()
        .icon(IconName::RefreshCw)
        .tooltip("Refresh Drives")
        .on_click(cx.listener(|this, _, _, cx| {
            this.refresh_volumes(cx);
        }))
        .into_any_element();

    v_flex()
        .gap_0p5()
        .child(collapsible_section_header(
            "sb-sec-drives",
            SidebarSection::Drives,
            collapsed,
            p,
            Some(refresh_btn),
            cx,
        ))
        .when(!collapsed, |el| {
            el.children(ws.volumes.iter().enumerate().map(|(i, vol)| {
                let target = Location::local(&vol.root_path);
                let active = match current_loc {
                    Location::Local(path) => path
                        .to_string_lossy()
                        .to_ascii_uppercase()
                        .starts_with(&vol.short_name.to_ascii_uppercase()),
                    _ => false,
                };
                let frac = vol.used_fraction();
                let bar_hex = usage_bar_color(frac, p);
                let title: SharedString = vol.display_name().into();
                let vol_mid = vol.root_path.clone();
                let vol_ctx = vol.clone();

                h_flex()
                    .id(("sb-drive", i))
                    .w_full()
                    .h(px(36.))
                    .px_2p5()
                    .gap_2()
                    .items_center()
                    .rounded(px(RADIUS_CONTROL))
                    .cursor_pointer()
                    .bg(if active {
                        solid(p.bg_elevated)
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .hover(move |s| s.bg(solid(p.bg_elevated)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.navigate_active_pane(target.clone(), window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            let loc = Location::local(&vol_mid);
                            if this.settings.show_tabs {
                                this.open_new_tab(loc, window, cx);
                            } else {
                                this.navigate_active_pane(loc, window, cx);
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
                        Icon::new(drive_kind_icon(vol.kind))
                            .xsmall()
                            .text_color(if active {
                                solid(p.accent_primary)
                            } else {
                                solid(p.text_secondary)
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_xs()
                                    .text_color(if active {
                                        solid(p.text_primary)
                                    } else {
                                        solid(p.text_secondary)
                                    })
                                    .child(title),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(2.))
                                    .rounded(px(1.))
                                    .bg(solid(p.border_subtle))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(frac))
                                            .bg(solid(bar_hex)),
                                    ),
                            ),
                    )
            }))
        })
}

fn render_wsl_section(
    ws: &Workspace,
    current_loc: &Location,
    collapsed: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(collapsible_section_header(
            "sb-sec-wsl",
            SidebarSection::LinuxWsl,
            collapsed,
            p,
            None,
            cx,
        ))
        .when(!collapsed, |el| {
            el.children(
                ws.wsl_distros
                    .iter()
                    .enumerate()
                    .map(|(i, (name, path))| {
                        let target = Location::local(path);
                        let active = *current_loc == target;
                        let label: SharedString = name.clone().into();
                        let open_loc = target.clone();
                        let mid_loc = target;

                        h_flex()
                            .id(("sb-wsl", i))
                            .w_full()
                            .h(px(32.))
                            .px_2p5()
                            .gap_2()
                            .items_center()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .bg(if active {
                                solid(p.bg_elevated)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .hover(move |s| s.bg(solid(p.bg_elevated)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.navigate_active_pane(open_loc.clone(), window, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    if this.settings.show_tabs {
                                        this.open_new_tab(mid_loc.clone(), window, cx);
                                    } else {
                                        this.navigate_active_pane(mid_loc.clone(), window, cx);
                                    }
                                }),
                            )
                            .child(
                                Icon::new(LucideIcon::Terminal)
                                    .xsmall()
                                    .text_color(if active {
                                        solid(p.accent_primary)
                                    } else {
                                        solid(p.text_secondary)
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(if active {
                                        solid(p.text_primary)
                                    } else {
                                        solid(p.text_secondary)
                                    })
                                    .child(label),
                            )
                    }),
            )
        })
}

fn collapsible_section_header(
    id: &'static str,
    section: SidebarSection,
    collapsed: bool,
    p: Palette,
    trailing: Option<AnyElement>,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let chevron = if collapsed {
        LucideIcon::ChevronRight
    } else {
        LucideIcon::ChevronDown
    };

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_1()
        .child(
            h_flex()
                .id(id)
                .flex_1()
                .min_w_0()
                .h(px(26.))
                .px_1p5()
                .gap_1()
                .items_center()
                .rounded(px(RADIUS_CONTROL))
                .cursor_pointer()
                .text_color(solid(p.text_muted))
                .hover(move |s| s.bg(solid(p.bg_elevated)).text_color(solid(p.text_primary)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_sidebar_section_collapsed(section, cx);
                }))
                .child(Icon::new(chevron).xsmall())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .font_semibold()
                        .child(section.header_label()),
                ),
        )
        .when_some(trailing, |el, action| el.child(action))
}
