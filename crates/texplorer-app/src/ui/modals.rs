use std::path::PathBuf;

use gpui_kit::{
    Context, Entity, MouseButton, MouseDownEvent, SharedString,
    assets::IconName as LucideIcon,
    component::{
        Disableable as _, Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::{AccentPreset, PaneSide, SizeUnit, ViewMode};
use texplorer_platform::DefaultExplorerStatus;

use crate::theme::{
    BG_BASE, BG_ELEVATED, BG_HOVER, BG_SURFACE, BORDER, BORDER_SUBTLE, RADIUS_CONTROL, RADIUS_ROW,
    STATUS_ERROR, STATUS_INFO, STATUS_SUCCESS, STATUS_WARNING, TEXT_MUTED, TEXT_PRIMARY,
    TEXT_SECONDARY, full_hit_input, soft_tint, solid,
};
use crate::ui::workspace::Workspace;
use crate::updater::{CURRENT_VERSION, UpdateState, format_download_bytes};

/// Modal dialog state for Settings and quick file prompts.
#[derive(Clone)]
pub enum ActiveModal {
    Settings,
    TextPrompt {
        title: &'static str,
        confirm_label: &'static str,
        kind: TextPromptKind,
        input: Entity<InputState>,
    },
    ConfirmPermanentDelete {
        side: PaneSide,
        paths: Vec<PathBuf>,
    },
}

#[derive(Clone)]
pub enum TextPromptKind {
    NewFolder { side: PaneSide, parent: PathBuf },
    NewFile { side: PaneSide, parent: PathBuf },
    Rename { side: PaneSide, target: PathBuf },
}

pub fn render_active_modal(
    ws: &Workspace,
    modal: &ActiveModal,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let card = match modal {
        ActiveModal::Settings => render_settings_card(ws, cx).into_any_element(),
        ActiveModal::TextPrompt {
            title,
            confirm_label,
            input,
            ..
        } => render_text_prompt_card(title, confirm_label, input, cx).into_any_element(),
        ActiveModal::ConfirmPermanentDelete { paths, .. } => {
            render_delete_confirm_card(paths, cx).into_any_element()
        }
    };

    div()
        .id("modal-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .child(
            div()
                .id("modal-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.65))
                .occlude()
                .on_scroll(|_, _, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_modal(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_modal(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_modal(cx);
                    }),
                ),
        )
        .child(div().relative().occlude().child(card))
}

fn render_text_prompt_card(
    title: &'static str,
    confirm_label: &'static str,
    input: &Entity<InputState>,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    v_flex()
        .id("prompt-modal-card")
        .w(px(420.))
        .bg(solid(BG_SURFACE))
        .border_1()
        .border_color(solid(BORDER))
        .rounded(px(RADIUS_ROW))
        .overflow_hidden()
        .occlude()
        .child(
            h_flex()
                .w_full()
                .h(px(40.))
                .px_3p5()
                .items_center()
                .justify_between()
                .bg(solid(BG_ELEVATED))
                .border_b_1()
                .border_color(solid(BORDER_SUBTLE))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(TEXT_PRIMARY))
                        .child(title),
                )
                .child(
                    Button::new("prompt-close")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(LucideIcon::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modal(cx);
                        })),
                ),
        )
        .child(
            v_flex()
                .p_4()
                .gap_3()
                .child(
                    div()
                        .w_full()
                        .h(px(34.))
                        .px_2p5()
                        .rounded(px(RADIUS_ROW))
                        .bg(solid(BG_BASE))
                        .border_1()
                        .border_color(solid(BORDER))
                        .child(full_hit_input(
                            "prompt-text-input",
                            input,
                            Input::new(input).with_size(Size::Small).appearance(false),
                        )),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("prompt-cancel")
                                .with_size(Size::Small)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_modal(cx);
                                })),
                        )
                        .child(
                            Button::new("prompt-confirm")
                                .with_size(Size::Small)
                                .primary()
                                .label(confirm_label)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_text_prompt(window, cx);
                                })),
                        ),
                ),
        )
}

fn render_delete_confirm_card(paths: &[PathBuf], cx: &mut Context<Workspace>) -> impl IntoElement {
    let summary: SharedString = if paths.len() == 1 {
        format!(
            "Permanently delete \"{}\"? This cannot be undone.",
            paths[0]
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| paths[0].display().to_string())
        )
        .into()
    } else {
        format!(
            "Permanently delete {} selected items? This cannot be undone.",
            paths.len()
        )
        .into()
    };

    v_flex()
        .id("confirm-del-card")
        .w(px(440.))
        .bg(solid(BG_SURFACE))
        .border_1()
        .border_color(solid(BORDER))
        .rounded(px(RADIUS_ROW))
        .overflow_hidden()
        .occlude()
        .child(
            h_flex()
                .w_full()
                .h(px(40.))
                .px_3p5()
                .items_center()
                .justify_between()
                .bg(solid(BG_ELEVATED))
                .border_b_1()
                .border_color(solid(BORDER_SUBTLE))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(STATUS_ERROR))
                        .child("Delete Permanently"),
                ),
        )
        .child(
            v_flex()
                .p_4()
                .gap_4()
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(TEXT_SECONDARY))
                        .child(summary),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("confirm-del-cancel")
                                .with_size(Size::Small)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_modal(cx);
                                })),
                        )
                        .child(
                            Button::new("confirm-del-ok")
                                .with_size(Size::Small)
                                .danger()
                                .label("Delete Forever")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_permanent_delete(window, cx);
                                })),
                        ),
                ),
        )
}

fn render_settings_card(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let accent = ws.settings.accent;
    let status = &ws.default_explorer_status;
    let (status_label, status_hex): (&'static str, u32) = match status {
        DefaultExplorerStatus::Enabled => ("Active", STATUS_SUCCESS),
        DefaultExplorerStatus::OutdatedPath(_) => ("Needs Update", STATUS_WARNING),
        DefaultExplorerStatus::Disabled => ("Windows Default", TEXT_SECONDARY),
        DefaultExplorerStatus::UnsupportedPlatform => ("Windows Only", TEXT_MUTED),
    };
    let is_windows = !matches!(status, DefaultExplorerStatus::UnsupportedPlatform);
    let is_enabled = matches!(status, DefaultExplorerStatus::Enabled);

    let update_busy = ws.update_state.is_busy();
    let (update_summary, update_hex): (SharedString, u32) = match &ws.update_state {
        UpdateState::Idle => (format!("Installed v{CURRENT_VERSION}").into(), TEXT_SECONDARY),
        UpdateState::Checking => ("Checking GitHub Releases...".into(), STATUS_INFO),
        UpdateState::UpToDate => (
            format!("v{CURRENT_VERSION} is up to date").into(),
            STATUS_SUCCESS,
        ),
        UpdateState::Available(info) => {
            let date_part = if info.published_at.is_empty() {
                String::new()
            } else {
                format!(" ({})", info.published_at)
            };
            (
                format!("Update {} available{date_part}", info.tag_name).into(),
                STATUS_SUCCESS,
            )
        }
        UpdateState::Downloading {
            info,
            downloaded_bytes,
            total_bytes,
        } => {
            let progress = match total_bytes {
                Some(total) if *total > 0 => format!(
                    "Downloading {} — {} / {}",
                    info.tag_name,
                    format_download_bytes(*downloaded_bytes),
                    format_download_bytes(*total)
                ),
                _ => format!(
                    "Downloading {} — {}",
                    info.tag_name,
                    format_download_bytes(*downloaded_bytes)
                ),
            };
            (progress.into(), STATUS_INFO)
        }
        UpdateState::Installing(info) => (
            format!("Installing {} and restarting...", info.tag_name).into(),
            STATUS_INFO,
        ),
        UpdateState::Failed(err) => (err.clone().into(), STATUS_ERROR),
    };

    let install_action_label: Option<&'static str> = match &ws.update_state {
        UpdateState::Available(info) => {
            if info.supports_in_app_install() {
                Some("Download & Install")
            } else {
                Some("Download from GitHub")
            }
        }
        _ => None,
    };

    v_flex()
        .id("settings-modal-card")
        .w(px(640.))
        .max_h(px(680.))
        .bg(solid(BG_SURFACE))
        .border_1()
        .border_color(solid(BORDER))
        .rounded(px(RADIUS_ROW))
        .overflow_hidden()
        .occlude()
        // Header
        .child(
            h_flex()
                .w_full()
                .h(px(42.))
                .px_4()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .bg(solid(BG_ELEVATED))
                .border_b_1()
                .border_color(solid(BORDER_SUBTLE))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(LucideIcon::Settings)
                                .small()
                                .text_color(solid(accent.primary_hex())),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(solid(TEXT_PRIMARY))
                                .child("Settings"),
                        ),
                )
                .child(
                    Button::new("settings-close")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(LucideIcon::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modal(cx);
                        })),
                ),
        )
        // Scrollable settings body
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .p_4()
                .gap_4()
                // Interface & Quick Access section
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("INTERFACE & SIDEBAR"))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(BG_BASE))
                                .border_1()
                                .border_color(solid(BORDER_SUBTLE))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .child(setting_label(
                                            "Top Tab Bar",
                                            "Show or hide browser tabs in the window header.",
                                        ))
                                        .child(choice_pill(
                                            "ui-show-tabs",
                                            if ws.settings.show_tabs {
                                                "Visible"
                                            } else {
                                                "Hidden"
                                            },
                                            ws.settings.show_tabs,
                                            accent.primary_hex(),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_show_tabs(cx);
                                            }),
                                        )),
                                )
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .child(setting_label(
                                            "Header Title (TExplorer)",
                                            "Show or hide the TExplorer title text at the top-left.",
                                        ))
                                        .child(choice_pill(
                                            "ui-show-brand",
                                            if ws.settings.show_brand_title {
                                                "Visible"
                                            } else {
                                                "Hidden"
                                            },
                                            ws.settings.show_brand_title,
                                            accent.primary_hex(),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_show_brand_title(cx);
                                            }),
                                        )),
                                )
                                .child(
                                    v_flex()
                                        .p_3()
                                        .gap_2()
                                        .child(setting_label(
                                            "Quick Access Folders",
                                            "Choose which default folders appear in Quick Access. You can also right-click any item to remove it.",
                                        ))
                                        .child(
                                            h_flex().flex_wrap().gap_1p5().children(
                                                ws.known_folders.iter().enumerate().map(
                                                    |(idx, folder)| {
                                                        let kind = folder.kind;
                                                        let visible = ws
                                                            .settings
                                                            .is_quick_access_visible(kind);
                                                        let label: SharedString =
                                                            folder.name.clone().into();
                                                        let accent_hex = accent.primary_hex();

                                                        div()
                                                            .id(("qa-toggle", idx))
                                                            .px_2p5()
                                                            .py_1()
                                                            .rounded(px(RADIUS_CONTROL))
                                                            .cursor_pointer()
                                                            .bg(if visible {
                                                                soft_tint(accent_hex, 0.16)
                                                            } else {
                                                                solid(BG_SURFACE)
                                                            })
                                                            .border_1()
                                                            .border_color(if visible {
                                                                solid(accent_hex)
                                                            } else {
                                                                solid(BORDER_SUBTLE)
                                                            })
                                                            .text_xs()
                                                            .font_medium()
                                                            .text_color(if visible {
                                                                solid(accent_hex)
                                                            } else {
                                                                solid(TEXT_MUTED)
                                                            })
                                                            .hover(move |s| {
                                                                if visible {
                                                                    s
                                                                } else {
                                                                    s.bg(solid(BG_HOVER))
                                                                        .text_color(solid(
                                                                            TEXT_PRIMARY,
                                                                        ))
                                                                }
                                                            })
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.toggle_quick_access(
                                                                        kind, cx,
                                                                    );
                                                                },
                                                            ))
                                                            .child(label)
                                                    },
                                                ),
                                            ),
                                        ),
                                ),
                        ),
                )
                // Appearance section
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("APPEARANCE"))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(BG_BASE))
                                .border_1()
                                .border_color(solid(BORDER_SUBTLE))
                                .overflow_hidden()
                                // Accent selector
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .child(setting_label(
                                            "Accent Color",
                                            "Obsidian Dark flat accent used for focus and active controls.",
                                        ))
                                        .child(
                                            h_flex().gap_1p5().children(
                                                AccentPreset::ALL.into_iter().map(|preset| {
                                                    let active = ws.settings.accent == preset;
                                                    div()
                                                        .id(preset.label())
                                                        .px_2()
                                                        .py_1()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .rounded(px(RADIUS_CONTROL))
                                                        .cursor_pointer()
                                                        .bg(if active {
                                                            solid(BG_ELEVATED)
                                                        } else {
                                                            solid(BG_SURFACE)
                                                        })
                                                        .border_1()
                                                        .border_color(if active {
                                                            solid(preset.primary_hex())
                                                        } else {
                                                            solid(BORDER_SUBTLE)
                                                        })
                                                        .hover(|s| s.bg(solid(BG_HOVER)))
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.set_accent(preset, cx);
                                                            },
                                                        ))
                                                        .child(
                                                            div()
                                                                .w(px(10.))
                                                                .h(px(10.))
                                                                .rounded(px(999.))
                                                                .bg(solid(preset.primary_hex())),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(solid(TEXT_PRIMARY))
                                                                .child(match preset {
                                                                    AccentPreset::Emerald => "Emerald",
                                                                    AccentPreset::Violet => "Violet",
                                                                    AccentPreset::Monochrome => "Mono",
                                                                }),
                                                        )
                                                }),
                                            ),
                                        ),
                                )
                                // Size unit
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .child(setting_label(
                                            "Size Units",
                                            "Choose binary (KiB/MiB) or decimal (KB/MB) formatting.",
                                        ))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(choice_pill(
                                                    "unit-bin",
                                                    "Binary",
                                                    ws.settings.size_unit == SizeUnit::Binary,
                                                    accent.primary_hex(),
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_size_unit(SizeUnit::Binary, cx);
                                                    }),
                                                ))
                                                .child(choice_pill(
                                                    "unit-dec",
                                                    "Decimal",
                                                    ws.settings.size_unit == SizeUnit::Decimal,
                                                    accent.primary_hex(),
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_size_unit(SizeUnit::Decimal, cx);
                                                    }),
                                                )),
                                        ),
                                )
                                // Default view mode
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .child(setting_label(
                                            "Default View Mode",
                                            "Layout used when opening new tabs or split panes.",
                                        ))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(choice_pill(
                                                    "dvm-details",
                                                    "Details",
                                                    ws.settings.default_view_mode == ViewMode::Details,
                                                    accent.primary_hex(),
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_default_view_mode(
                                                            ViewMode::Details,
                                                            cx,
                                                        );
                                                    }),
                                                ))
                                                .child(choice_pill(
                                                    "dvm-grid",
                                                    "Grid",
                                                    ws.settings.default_view_mode == ViewMode::Grid,
                                                    accent.primary_hex(),
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_default_view_mode(
                                                            ViewMode::Grid,
                                                            cx,
                                                        );
                                                    }),
                                                ))
                                                .child(choice_pill(
                                                    "dvm-compact",
                                                    "Compact",
                                                    ws.settings.default_view_mode == ViewMode::Compact,
                                                    accent.primary_hex(),
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_default_view_mode(
                                                            ViewMode::Compact,
                                                            cx,
                                                        );
                                                    }),
                                                )),
                                        ),
                                ),
                        ),
                )
                // Windows Default File Manager Integration section
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("WINDOWS INTEGRATION"))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(BG_BASE))
                                .border_1()
                                .border_color(solid(BORDER_SUBTLE))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .border_b_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .child(setting_label(
                                            "Set TExplorer as Default File Manager",
                                            "Registers per-user HKCU folder handlers (no UAC). Keeps taskbar, desktop, and Control Panel untouched.",
                                        ))
                                        .child(
                                            h_flex()
                                                .flex_shrink_0()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .py_0p5()
                                                        .rounded(px(RADIUS_CONTROL))
                                                        .bg(soft_tint(status_hex, 0.14))
                                                        .text_size(px(11.))
                                                        .font_medium()
                                                        .text_color(solid(status_hex))
                                                        .child(status_label),
                                                )
                                                .when(is_windows && !is_enabled, |el| {
                                                    el.child(
                                                        Button::new("win-enable-default")
                                                            .with_size(Size::XSmall)
                                                            .primary()
                                                            .label("Enable")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.enable_default_file_manager(
                                                                        cx,
                                                                    );
                                                                },
                                                            )),
                                                    )
                                                })
                                                .when(is_windows && is_enabled, |el| {
                                                    el.child(
                                                        Button::new("win-restore-default")
                                                            .with_size(Size::XSmall)
                                                            .danger()
                                                            .label("Restore Windows Explorer")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.restore_default_file_manager(
                                                                        cx,
                                                                    );
                                                                },
                                                            )),
                                                    )
                                                }),
                                        ),
                                )
                                .when(is_windows, |el| {
                                    el.child(
                                        h_flex()
                                            .p_3()
                                            .items_center()
                                            .justify_between()
                                            .gap_3()
                                            .child(setting_label(
                                                "Intercept Win+E Shortcut",
                                                "Open TExplorer when pressing Win+E via HKCU Shell CLSID routing (no global keyboard hooks).",
                                            ))
                                            .child(choice_pill(
                                                "win-e-toggle",
                                                if ws.settings.intercept_win_e {
                                                    "Enabled"
                                                } else {
                                                    "Disabled"
                                                },
                                                ws.settings.intercept_win_e,
                                                accent.primary_hex(),
                                                cx.listener(|this, _, _, cx| {
                                                    this.toggle_intercept_win_e(cx);
                                                }),
                                            )),
                                    )
                                }),
                        ),
                )
                // Updates & About section
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("UPDATES"))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(BG_BASE))
                                .border_1()
                                .border_color(solid(BORDER_SUBTLE))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .border_b_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_0p5()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .text_color(solid(TEXT_PRIMARY))
                                                        .child(format!(
                                                            "TExplorer v{CURRENT_VERSION}"
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(solid(update_hex))
                                                        .child(update_summary),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .flex_shrink_0()
                                                .items_center()
                                                .gap_1p5()
                                                .when_some(install_action_label, |el, lbl| {
                                                    el.child(
                                                        Button::new("upd-install")
                                                            .with_size(Size::XSmall)
                                                            .primary()
                                                            .label(lbl)
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.start_update_install(cx);
                                                                },
                                                            )),
                                                    )
                                                })
                                                .child(
                                                    Button::new("upd-check")
                                                        .with_size(Size::XSmall)
                                                        .disabled(update_busy)
                                                        .label("Check for Updates")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.check_for_updates(true, cx);
                                                        })),
                                                )
                                                .child(
                                                    Button::new("upd-releases")
                                                        .with_size(Size::XSmall)
                                                        .ghost()
                                                        .icon(LucideIcon::ExternalLink)
                                                        .tooltip("Open GitHub Releases")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.open_releases_page(cx);
                                                        })),
                                                ),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .child(setting_label(
                                            "Automatic Update Check",
                                            "Check GitHub Releases quietly when TExplorer starts.",
                                        ))
                                        .child(choice_pill(
                                            "upd-auto-toggle",
                                            if ws.settings.auto_check_updates {
                                                "Enabled"
                                            } else {
                                                "Disabled"
                                            },
                                            ws.settings.auto_check_updates,
                                            accent.primary_hex(),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_auto_check_updates(cx);
                                            }),
                                        )),
                                ),
                        ),
                ),
        )
}

fn section_heading(text: &'static str) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .font_semibold()
        .text_color(solid(TEXT_MUTED))
        .child(text)
}

fn setting_label(title: &'static str, subtitle: &'static str) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(solid(TEXT_PRIMARY))
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(solid(TEXT_MUTED))
                .child(subtitle),
        )
}

fn choice_pill(
    id: &'static str,
    label: &'static str,
    active: bool,
    accent_hex: u32,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .bg(if active {
            soft_tint(accent_hex, 0.16)
        } else {
            solid(BG_SURFACE)
        })
        .border_1()
        .border_color(if active {
            solid(accent_hex)
        } else {
            solid(BORDER_SUBTLE)
        })
        .text_xs()
        .font_medium()
        .text_color(if active {
            solid(accent_hex)
        } else {
            solid(TEXT_SECONDARY)
        })
        .hover(move |s| {
            if active {
                s
            } else {
                s.bg(solid(BG_HOVER)).text_color(solid(TEXT_PRIMARY))
            }
        })
        .on_click(on_click)
        .child(label)
}
