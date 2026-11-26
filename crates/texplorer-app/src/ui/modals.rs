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
use texplorer_core::{AccentPreset, PaneSide, SizeUnit, ThemePreset, ViewMode};
use texplorer_mft::IndexServiceInstallState;
use texplorer_platform::DefaultExplorerStatus;

use crate::theme::{Palette, RADIUS_CONTROL, RADIUS_ROW, full_hit_input, soft_tint, solid};
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
    let p = ws.palette();
    let card = match modal {
        ActiveModal::Settings => render_settings_card(ws, p, cx).into_any_element(),
        ActiveModal::TextPrompt {
            title,
            confirm_label,
            input,
            ..
        } => render_text_prompt_card(title, confirm_label, input, p, cx).into_any_element(),
        ActiveModal::ConfirmPermanentDelete { paths, .. } => {
            render_delete_confirm_card(paths, p, cx).into_any_element()
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
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, p.backdrop_alpha()))
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
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    v_flex()
        .id("prompt-modal-card")
        .w(px(420.))
        .bg(solid(p.bg_surface))
        .border_1()
        .border_color(solid(p.border))
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
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.text_primary))
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
                        .bg(solid(p.bg_base))
                        .border_1()
                        .border_color(solid(p.border))
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

fn render_delete_confirm_card(
    paths: &[PathBuf],
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
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
        .bg(solid(p.bg_surface))
        .border_1()
        .border_color(solid(p.border))
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
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.status_error))
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
                        .text_color(solid(p.text_secondary))
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

fn render_settings_card(
    ws: &Workspace,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let status = &ws.default_explorer_status;
    let (status_label, status_hex): (&'static str, u32) = match status {
        DefaultExplorerStatus::Enabled => ("Active", p.status_success),
        DefaultExplorerStatus::OutdatedPath(_) => ("Needs Update", p.status_warning),
        DefaultExplorerStatus::Disabled => ("Windows Default", p.text_secondary),
        DefaultExplorerStatus::UnsupportedPlatform => ("Windows Only", p.text_muted),
    };
    let is_windows = !matches!(status, DefaultExplorerStatus::UnsupportedPlatform);
    let is_enabled = matches!(status, DefaultExplorerStatus::Enabled);

    let svc_state = ws.service_install_state;
    let svc_busy = ws.service_busy;
    let (svc_badge_label, svc_badge_hex): (SharedString, u32) = match svc_state {
        IndexServiceInstallState::Running => {
            if let Some(live) = &ws.service_live_status {
                if live.indexing {
                    ("Indexing MFT...".into(), p.status_info)
                } else if live.total_entries > 0 {
                    (
                        format!("Active ({} items)", live.total_entries).into(),
                        p.status_success,
                    )
                } else {
                    ("Active".into(), p.status_success)
                }
            } else {
                ("Active".into(), p.status_success)
            }
        }
        IndexServiceInstallState::Stopped => ("Stopped".into(), p.status_warning),
        IndexServiceInstallState::NotInstalled => ("Not Installed".into(), p.text_secondary),
        IndexServiceInstallState::Unavailable => ("Windows Only".into(), p.text_muted),
    };
    let svc_supported = svc_state != IndexServiceInstallState::Unavailable;
    let svc_running = svc_state == IndexServiceInstallState::Running;
    let svc_stopped = svc_state == IndexServiceInstallState::Stopped;
    let svc_not_installed = svc_state == IndexServiceInstallState::NotInstalled;

    let theme_subtitle: SharedString = if ws.settings.theme == ThemePreset::FilesSystem {
        let mode_label = if ws.system_is_dark { "Dark" } else { "Light" };
        format!(
            "Follows Windows system mode using the Files palette (currently {mode_label})."
        )
        .into()
    } else {
        "Files palettes appear first, followed by Obsidian Dark.".into()
    };

    let update_busy = ws.update_state.is_busy();
    let (update_summary, update_hex): (SharedString, u32) = match &ws.update_state {
        UpdateState::Idle => (
            format!("Installed v{CURRENT_VERSION}").into(),
            p.text_secondary,
        ),
        UpdateState::Checking => ("Checking GitHub Releases...".into(), p.status_info),
        UpdateState::UpToDate => (
            format!("v{CURRENT_VERSION} is up to date").into(),
            p.status_success,
        ),
        UpdateState::Available(info) => {
            let date_part = if info.published_at.is_empty() {
                String::new()
            } else {
                format!(" ({})", info.published_at)
            };
            (
                format!("Update {} available{date_part}", info.tag_name).into(),
                p.status_success,
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
            (progress.into(), p.status_info)
        }
        UpdateState::Installing(info) => (
            format!("Installing {} and restarting...", info.tag_name).into(),
            p.status_info,
        ),
        UpdateState::Failed(err) => (err.clone().into(), p.status_error),
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
        .bg(solid(p.bg_surface))
        .border_1()
        .border_color(solid(p.border))
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
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(LucideIcon::Settings)
                                .small()
                                .text_color(solid(p.accent_primary)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
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
                        .child(section_heading("INTERFACE & SIDEBAR", p))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Top Tab Bar",
                                            "Show or hide browser tabs in the window header.",
                                            p,
                                        ))
                                        .child(choice_pill(
                                            "ui-show-tabs",
                                            if ws.settings.show_tabs {
                                                "Visible"
                                            } else {
                                                "Hidden"
                                            },
                                            ws.settings.show_tabs,
                                            p,
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
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Header Title (TExplorer)",
                                            "Show or hide the TExplorer title text at the top-left.",
                                            p,
                                        ))
                                        .child(choice_pill(
                                            "ui-show-brand",
                                            if ws.settings.show_brand_title {
                                                "Visible"
                                            } else {
                                                "Hidden"
                                            },
                                            ws.settings.show_brand_title,
                                            p,
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
                                            p,
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

                                                        div()
                                                            .id(("qa-toggle", idx))
                                                            .px_2p5()
                                                            .py_1()
                                                            .rounded(px(RADIUS_CONTROL))
                                                            .cursor_pointer()
                                                            .bg(if visible {
                                                                soft_tint(p.accent_primary, 0.16)
                                                            } else {
                                                                solid(p.bg_surface)
                                                            })
                                                            .border_1()
                                                            .border_color(if visible {
                                                                solid(p.accent_primary)
                                                            } else {
                                                                solid(p.border_subtle)
                                                            })
                                                            .text_xs()
                                                            .font_medium()
                                                            .text_color(if visible {
                                                                solid(p.accent_primary)
                                                            } else {
                                                                solid(p.text_muted)
                                                            })
                                                            .hover(move |s| {
                                                                if visible {
                                                                    s
                                                                } else {
                                                                    s.bg(solid(p.bg_hover))
                                                                        .text_color(solid(
                                                                            p.text_primary,
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
                // Appearance section (Files palettes & accents first, then Obsidian)
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("APPEARANCE", p))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .overflow_hidden()
                                // Theme Palette selector (Files System / Files Dark / Files Light / Obsidian)
                                .child(
                                    v_flex()
                                        .p_3()
                                        .gap_2()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label_shared(
                                            "Theme Palette",
                                            theme_subtitle,
                                            p,
                                        ))
                                        .child(
                                            h_flex().flex_wrap().gap_1p5().children(
                                                ThemePreset::ALL.into_iter().map(|preset| {
                                                    let active = ws.settings.theme == preset;
                                                    let (swatch_bg, swatch_border) = match preset {
                                                        ThemePreset::FilesSystem => (
                                                            if ws.system_is_dark {
                                                                0x111111
                                                            } else {
                                                                0xfafafa
                                                            },
                                                            p.accent_primary,
                                                        ),
                                                        ThemePreset::FilesDark => {
                                                            (0x111111, 0x50c0ff)
                                                        }
                                                        ThemePreset::FilesLight => {
                                                            (0xfafafa, 0x0070cb)
                                                        }
                                                        ThemePreset::ObsidianDark => {
                                                            (0x09090b, 0x34d399)
                                                        }
                                                    };

                                                    div()
                                                        .id(preset.label())
                                                        .px_2p5()
                                                        .py_1()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .rounded(px(RADIUS_CONTROL))
                                                        .cursor_pointer()
                                                        .bg(if active {
                                                            soft_tint(p.accent_primary, 0.14)
                                                        } else {
                                                            solid(p.bg_surface)
                                                        })
                                                        .border_1()
                                                        .border_color(if active {
                                                            solid(p.accent_primary)
                                                        } else {
                                                            solid(p.border_subtle)
                                                        })
                                                        .hover(move |s| {
                                                            if active {
                                                                s
                                                            } else {
                                                                s.bg(solid(p.bg_hover))
                                                            }
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.set_theme(preset, cx);
                                                            },
                                                        ))
                                                        .child(
                                                            div()
                                                                .w(px(10.))
                                                                .h(px(10.))
                                                                .rounded(px(999.))
                                                                .bg(solid(swatch_bg))
                                                                .border_1()
                                                                .border_color(solid(swatch_border)),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .font_medium()
                                                                .text_color(if active {
                                                                    solid(p.accent_primary)
                                                                } else {
                                                                    solid(p.text_primary)
                                                                })
                                                                .child(preset.short_label()),
                                                        )
                                                }),
                                            ),
                                        ),
                                )
                                // Accent selector (Files colors first, then Obsidian colors)
                                .child(
                                    v_flex()
                                        .p_3()
                                        .gap_2()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Accent Color",
                                            "Files accents appear first, followed by Obsidian accents.",
                                            p,
                                        ))
                                        .child(
                                            h_flex().flex_wrap().gap_1p5().children(
                                                AccentPreset::ALL.into_iter().map(|preset| {
                                                    let active = ws.settings.accent == preset;
                                                    let dot_hex = preset.primary_hex_for(p.is_dark);

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
                                                            soft_tint(dot_hex, 0.14)
                                                        } else {
                                                            solid(p.bg_surface)
                                                        })
                                                        .border_1()
                                                        .border_color(if active {
                                                            solid(dot_hex)
                                                        } else {
                                                            solid(p.border_subtle)
                                                        })
                                                        .hover(move |s| {
                                                            if active {
                                                                s
                                                            } else {
                                                                s.bg(solid(p.bg_hover))
                                                            }
                                                        })
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
                                                                .bg(solid(dot_hex)),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(solid(p.text_primary))
                                                                .child(preset.short_label()),
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
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Size Units",
                                            "Choose binary (KiB/MiB) or decimal (KB/MB) formatting.",
                                            p,
                                        ))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(choice_pill(
                                                    "unit-bin",
                                                    "Binary",
                                                    ws.settings.size_unit == SizeUnit::Binary,
                                                    p,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_size_unit(SizeUnit::Binary, cx);
                                                    }),
                                                ))
                                                .child(choice_pill(
                                                    "unit-dec",
                                                    "Decimal",
                                                    ws.settings.size_unit == SizeUnit::Decimal,
                                                    p,
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
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Default View Mode",
                                            "Layout used when opening new tabs or split panes.",
                                            p,
                                        ))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(choice_pill(
                                                    "dvm-details",
                                                    "Details",
                                                    ws.settings.default_view_mode
                                                        == ViewMode::Details,
                                                    p,
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
                                                    p,
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
                                                    ws.settings.default_view_mode
                                                        == ViewMode::Compact,
                                                    p,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.set_default_view_mode(
                                                            ViewMode::Compact,
                                                            cx,
                                                        );
                                                    }),
                                                )),
                                        ),
                                )
                                // Session persistence
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .child(setting_label(
                                            "Restore Tabs & Window on Startup",
                                            "Automatically reopen your previous tabs, Dual-Pane splits, and window size.",
                                            p,
                                        ))
                                        .child(choice_pill(
                                            "set-restore-session",
                                            if ws.settings.restore_tabs_on_startup {
                                                "Enabled"
                                            } else {
                                                "Disabled"
                                            },
                                            ws.settings.restore_tabs_on_startup,
                                            p,
                                            cx.listener(|this, _, window, cx| {
                                                this.toggle_restore_tabs_on_startup(window, cx);
                                            }),
                                        )),
                                ),
                        ),
                )
                // Search, MFT Service & Folder Sizes section
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("SEARCH & INDEXING", p))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "NTFS MFT + USN Index Service",
                                            "Read-only background service for <10 ms global search (Ctrl+K) and instant folder sizes without UAC.",
                                            p,
                                        ))
                                        .child(
                                            h_flex()
                                                .flex_shrink_0()
                                                .items_center()
                                                .gap_1p5()
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .py_0p5()
                                                        .rounded(px(RADIUS_CONTROL))
                                                        .bg(soft_tint(svc_badge_hex, 0.14))
                                                        .text_size(px(13.))
                                                        .font_medium()
                                                        .text_color(solid(svc_badge_hex))
                                                        .child(svc_badge_label),
                                                )
                                                .when(svc_supported && svc_not_installed, |el| {
                                                    el.child(
                                                        Button::new("svc-install")
                                                            .with_size(Size::XSmall)
                                                            .primary()
                                                            .disabled(svc_busy)
                                                            .label("Install & Start")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.install_mft_service(cx);
                                                                },
                                                            )),
                                                    )
                                                })
                                                .when(svc_supported && svc_stopped, |el| {
                                                    el.child(
                                                        Button::new("svc-start")
                                                            .with_size(Size::XSmall)
                                                            .primary()
                                                            .disabled(svc_busy)
                                                            .label("Start")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.start_mft_service(cx);
                                                                },
                                                            )),
                                                    )
                                                })
                                                .when(svc_supported && svc_running, |el| {
                                                    el.child(
                                                        Button::new("svc-rebuild")
                                                            .with_size(Size::XSmall)
                                                            .disabled(svc_busy)
                                                            .label("Rebuild")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.rebuild_mft_index(cx);
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("svc-uninstall")
                                                            .with_size(Size::XSmall)
                                                            .danger()
                                                            .disabled(svc_busy)
                                                            .label("Uninstall")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.uninstall_mft_service(cx);
                                                                },
                                                            )),
                                                    )
                                                }),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Use MFT Index Service",
                                            "Prefer the background NTFS index service over multi-threaded fallback walk when available.",
                                            p,
                                        ))
                                        .child(choice_pill(
                                            "set-use-mft",
                                            if ws.settings.use_mft_service {
                                                "Enabled"
                                            } else {
                                                "Disabled"
                                            },
                                            ws.settings.use_mft_service,
                                            p,
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_use_mft_service(cx);
                                            }),
                                        )),
                                )
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .child(setting_label(
                                            "Calculate Folder Sizes",
                                            "Resolve recursive folder byte totals in Details view (instant via MFT Service or background walk).",
                                            p,
                                        ))
                                        .child(choice_pill(
                                            "set-folder-sizes",
                                            if ws.settings.calculate_folder_sizes {
                                                "Enabled"
                                            } else {
                                                "Disabled"
                                            },
                                            ws.settings.calculate_folder_sizes,
                                            p,
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_calculate_folder_sizes(cx);
                                            }),
                                        )),
                                ),
                        ),
                )
                // Windows Default File Manager Integration section
                .child(
                    v_flex()
                        .gap_2()
                        .child(section_heading("WINDOWS INTEGRATION", p))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(setting_label(
                                            "Set TExplorer as Default File Manager",
                                            "Registers per-user HKCU folder handlers (no UAC). Keeps taskbar, desktop, and Control Panel untouched.",
                                            p,
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
                                                        .text_size(px(13.))
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
                                                p,
                                            ))
                                            .child(choice_pill(
                                                "win-e-toggle",
                                                if ws.settings.intercept_win_e {
                                                    "Enabled"
                                                } else {
                                                    "Disabled"
                                                },
                                                ws.settings.intercept_win_e,
                                                p,
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
                        .child(section_heading("UPDATES", p))
                        .child(
                            v_flex()
                                .rounded(px(RADIUS_ROW))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .overflow_hidden()
                                .child(
                                    h_flex()
                                        .p_3()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .border_b_1()
                                        .border_color(solid(p.border_subtle))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_0p5()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .text_color(solid(p.text_primary))
                                                        .child(format!(
                                                            "TExplorer v{CURRENT_VERSION}"
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
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
                                            p,
                                        ))
                                        .child(choice_pill(
                                            "upd-auto-toggle",
                                            if ws.settings.auto_check_updates {
                                                "Enabled"
                                            } else {
                                                "Disabled"
                                            },
                                            ws.settings.auto_check_updates,
                                            p,
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_auto_check_updates(cx);
                                            }),
                                        )),
                                ),
                        ),
                ),
        )
}

fn section_heading(text: &'static str, p: Palette) -> impl IntoElement {
    div()
        .text_size(px(13.))
        .font_semibold()
        .text_color(solid(p.text_muted))
        .child(text)
}

fn setting_label(title: &'static str, subtitle: &'static str, p: Palette) -> impl IntoElement {
    setting_label_shared(title, subtitle.into(), p)
}

fn setting_label_shared(
    title: &'static str,
    subtitle: SharedString,
    p: Palette,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(solid(p.text_primary))
                .child(title),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(solid(p.text_muted))
                .child(subtitle),
        )
}

fn choice_pill(
    id: &'static str,
    label: &'static str,
    active: bool,
    p: Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .bg(if active {
            soft_tint(p.accent_primary, 0.16)
        } else {
            solid(p.bg_surface)
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
                s.bg(solid(p.bg_hover)).text_color(solid(p.text_primary))
            }
        })
        .on_click(on_click)
        .child(label)
}
