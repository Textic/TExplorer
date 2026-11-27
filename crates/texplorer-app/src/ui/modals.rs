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
use texplorer_core::{
    AccentPreset, IconStyle, PaneSide, SidebarSection, SizeUnit, ThemePreset, ViewMode,
};
use texplorer_mft::IndexServiceInstallState;
use texplorer_platform::DefaultExplorerStatus;
use uuid::Uuid;

use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, full_hit_input, known_folder_icon, soft_tint, solid,
};
use crate::ui::network::{
    NetworkConnectModalState, P2pModalState, render_network_connect_modal, render_p2p_modal,
};
use crate::ui::sidebar::{
    DraggedPinnedFolder, DraggedQuickAccessItem, DraggedSidebarSection, SidebarDragGhost,
};
use crate::ui::workspace::Workspace;
use crate::updater::{CURRENT_VERSION, UpdateState, format_download_bytes};

/// Active navigation section in the left rail of the Settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsCategory {
    #[default]
    Appearance,
    Interface,
    Search,
    Windows,
    Updates,
}

impl SettingsCategory {
    pub const ALL: [Self; 5] = [
        Self::Appearance,
        Self::Interface,
        Self::Search,
        Self::Windows,
        Self::Updates,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Appearance => "set-cat-appearance",
            Self::Interface => "set-cat-interface",
            Self::Search => "set-cat-search",
            Self::Windows => "set-cat-windows",
            Self::Updates => "set-cat-updates",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Interface => "Interface & Sidebar",
            Self::Search => "Search & Indexing",
            Self::Windows => "Windows Integration",
            Self::Updates => "Updates & About",
        }
    }

    pub fn icon(self) -> LucideIcon {
        match self {
            Self::Appearance => LucideIcon::Eye,
            Self::Interface => LucideIcon::LayoutList,
            Self::Search => LucideIcon::Search,
            Self::Windows => LucideIcon::Monitor,
            Self::Updates => LucideIcon::Download,
        }
    }
}

/// Modal dialog state for Settings, Network connections, P2P transfers, and quick file prompts.
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
    NetworkConnect(NetworkConnectModalState),
    P2p(P2pModalState),
}

#[derive(Clone)]
pub enum TextPromptKind {
    NewFolder { side: PaneSide, parent: PathBuf },
    NewFile { side: PaneSide, parent: PathBuf },
    Rename { side: PaneSide, target: PathBuf },
    RemoteNewFolder {
        side: PaneSide,
        connection_id: Uuid,
        parent_remote_path: String,
    },
    RemoteRename {
        side: PaneSide,
        connection_id: Uuid,
        remote_path: String,
    },
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
        ActiveModal::NetworkConnect(form) => {
            render_network_connect_modal(ws, form, p, cx).into_any_element()
        }
        ActiveModal::P2p(state) => render_p2p_modal(ws, state, p, cx).into_any_element(),
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

    let active_category = ws.settings_category;
    let has_update_badge = matches!(ws.update_state, UpdateState::Available(_));

    let appearance_section = v_flex()
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
                        .child(setting_label_shared("Theme Palette", theme_subtitle, p))
                        .child(h_flex().flex_wrap().gap_1p5().children(
                            ThemePreset::ALL.into_iter().map(|preset| {
                                let active = ws.settings.theme == preset;
                                let (swatch_bg, swatch_border) = match preset {
                                    ThemePreset::FilesSystem => (
                                        if ws.system_is_dark { 0x111111 } else { 0xfafafa },
                                        p.accent_primary,
                                    ),
                                    ThemePreset::FilesDark => (0x111111, 0x50c0ff),
                                    ThemePreset::FilesLight => (0xfafafa, 0x0070cb),
                                    ThemePreset::ObsidianDark => (0x09090b, 0x34d399),
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_theme(preset, cx);
                                    }))
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
                        )),
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
                        .child(h_flex().flex_wrap().gap_1p5().children(
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
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_accent(preset, cx);
                                    }))
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
                        )),
                )
                // File & Folder Icon Style selector
                .child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(setting_label(
                            "File & Folder Icons",
                            match ws.settings.icon_style {
                                IconStyle::SystemFiles => {
                                    "Windows system icons for files (DLL, EXE, PDF, etc.) while keeping TExplorer vector folders."
                                }
                                IconStyle::SystemAll => {
                                    "Native Windows icons for both files and folders (including custom folder icons)."
                                }
                                IconStyle::Minimal => {
                                    "Built-in minimalist vector icons for all files and folders."
                                }
                            },
                            p,
                        ))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_1p5()
                                .child(choice_pill(
                                    "icon-style-sys-files",
                                    "System Files",
                                    ws.settings.icon_style == IconStyle::SystemFiles,
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_icon_style(IconStyle::SystemFiles, cx);
                                    }),
                                ))
                                .child(choice_pill(
                                    "icon-style-sys-all",
                                    "System (+ Windows Folders)",
                                    ws.settings.icon_style == IconStyle::SystemAll,
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_icon_style(IconStyle::SystemAll, cx);
                                    }),
                                ))
                                .child(choice_pill(
                                    "icon-style-minimal",
                                    "Minimal",
                                    ws.settings.icon_style == IconStyle::Minimal,
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_icon_style(IconStyle::Minimal, cx);
                                    }),
                                )),
                        ),
                )
                // Size unit
                .child(
                    h_flex()
                        .p_3()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(setting_label(
                            "Size Units",
                            "Choose binary (KiB/MiB) or decimal (KB/MB) formatting.",
                            p,
                        ))
                        .child(
                            h_flex()
                                .flex_shrink_0()
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
                        .gap_3()
                        .child(setting_label(
                            "Default View Mode",
                            "Layout used when opening new tabs or split panes.",
                            p,
                        ))
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .gap_1()
                                .child(choice_pill(
                                    "dvm-details",
                                    "Details",
                                    ws.settings.default_view_mode == ViewMode::Details,
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_default_view_mode(ViewMode::Details, cx);
                                    }),
                                ))
                                .child(choice_pill(
                                    "dvm-grid",
                                    "Grid",
                                    ws.settings.default_view_mode == ViewMode::Grid,
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_default_view_mode(ViewMode::Grid, cx);
                                    }),
                                ))
                                .child(choice_pill(
                                    "dvm-compact",
                                    "Compact",
                                    ws.settings.default_view_mode == ViewMode::Compact,
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_default_view_mode(ViewMode::Compact, cx);
                                    }),
                                )),
                        ),
                ),
        );

    let interface_section = v_flex()
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
                        .gap_3()
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(setting_label(
                            "Top Tab Bar",
                            "Show or hide browser tabs in the window header.",
                            p,
                        ))
                        .child(choice_pill(
                            "ui-show-tabs",
                            if ws.settings.show_tabs { "Visible" } else { "Hidden" },
                            ws.settings.show_tabs,
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_show_tabs(cx);
                            }),
                        )),
                )
                .when(ws.settings.show_tabs, |el| {
                    el.child(
                        h_flex()
                            .p_3()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .border_b_1()
                            .border_color(solid(p.border_subtle))
                            .child(setting_label(
                                "Win+E When Already Open",
                                "Open a separate TExplorer window or add a new tab when pressing Win+E.",
                                p,
                            ))
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(choice_pill(
                                        "ui-win-e-new-window",
                                        "New Window",
                                        ws.settings.win_e_new_window,
                                        p,
                                        cx.listener(|this, _, _, cx| {
                                            this.set_win_e_new_window(true, cx);
                                        }),
                                    ))
                                    .child(choice_pill(
                                        "ui-win-e-new-tab",
                                        "New Tab",
                                        !ws.settings.win_e_new_window,
                                        p,
                                        cx.listener(|this, _, _, cx| {
                                            this.set_win_e_new_window(false, cx);
                                        }),
                                    )),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .p_3()
                        .items_center()
                        .justify_between()
                        .gap_3()
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
                    h_flex()
                        .p_3()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
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
                )
                .child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(setting_label(
                                    "Sidebar Categories",
                                    "Drag categories to reorder them, or toggle visibility. You can also drag category headers directly in the sidebar.",
                                    p,
                                ))
                                .when(ws.settings.has_custom_sidebar_sections(), |el| {
                                    el.child(
                                        Button::new("sb-sec-reset")
                                            .with_size(Size::XSmall)
                                            .ghost()
                                            .label("Reset")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.reset_sidebar_sections(cx);
                                            })),
                                    )
                                }),
                        )
                        .child({
                            let ordered = ws.settings.normalized_sidebar_order();
                            v_flex().gap_1p5().children(ordered.into_iter().enumerate().map(
                                |(idx, section)| {
                                    let visible = ws.settings.is_sidebar_section_visible(section);
                                    let collapsed =
                                        ws.settings.is_sidebar_section_collapsed(section);
                                    let icon = match section {
                                        SidebarSection::QuickAccess => LucideIcon::Folder,
                                        SidebarSection::Pinned => LucideIcon::Pin,
                                        SidebarSection::Drives => LucideIcon::HardDrive,
                                        SidebarSection::Network => LucideIcon::Network,
                                        SidebarSection::LinuxWsl => LucideIcon::Terminal,
                                    };
                                    let drag_payload = DraggedSidebarSection {
                                        section,
                                        label: section.label().into(),
                                    };

                                    h_flex()
                                        .id(("sb-sec-row", idx))
                                        .w_full()
                                        .px_2p5()
                                        .py_1p5()
                                        .gap_2()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(RADIUS_CONTROL))
                                        .cursor_grab()
                                        .bg(solid(p.bg_surface))
                                        .border_1()
                                        .border_color(solid(p.border_subtle))
                                        .hover(move |s| s.border_color(solid(p.border)))
                                        .on_drag(drag_payload, move |drag, cursor_offset, _, cx| {
                                            cx.new(|_| SidebarDragGhost {
                                                label: drag.label.clone(),
                                                palette: p,
                                                cursor_offset,
                                            })
                                        })
                                        .drag_over::<DraggedSidebarSection>(
                                            move |style, drag, _, _| {
                                                if drag.section != section {
                                                    style
                                                        .bg(soft_tint(p.accent_primary, 0.14))
                                                        .border_color(solid(p.accent_primary))
                                                } else {
                                                    style
                                                }
                                            },
                                        )
                                        .on_drop(cx.listener(
                                            move |this, drag: &DraggedSidebarSection, _, cx| {
                                                cx.stop_propagation();
                                                this.reorder_sidebar_section(
                                                    drag.section,
                                                    section,
                                                    cx,
                                                );
                                            },
                                        ))
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .items_center()
                                                .gap_2()
                                                .child(drag_grip_handle(p))
                                                .child(Icon::new(icon).xsmall().text_color(
                                                    if visible {
                                                        solid(p.accent_primary)
                                                    } else {
                                                        solid(p.text_muted)
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(if visible {
                                                            solid(p.text_primary)
                                                        } else {
                                                            solid(p.text_muted)
                                                        })
                                                        .child(section.label()),
                                                )
                                                .when(visible && collapsed, |el| {
                                                    el.child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .text_color(solid(p.text_muted))
                                                            .child("(Collapsed)"),
                                                    )
                                                }),
                                        )
                                        .child(choice_pill(
                                            ("sb-sec-vis", idx),
                                            if visible { "Visible" } else { "Hidden" },
                                            visible,
                                            p,
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle_sidebar_section_visible(section, cx);
                                            }),
                                        ))
                                },
                            ))
                        }),
                )
                .child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(setting_label(
                                    "Quick Access Folders",
                                    "Drag folders to reorder them, or toggle which default folders appear in Quick Access.",
                                    p,
                                ))
                                .when(
                                    ws.settings.has_custom_quick_access(&ws.known_folders),
                                    |el| {
                                        el.child(
                                            Button::new("qa-reset")
                                                .with_size(Size::XSmall)
                                                .ghost()
                                                .label("Reset")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.reset_quick_access(cx);
                                                })),
                                        )
                                    },
                                ),
                        )
                        .child({
                            let ordered_qa = ws.settings.ordered_quick_access(&ws.known_folders);
                            v_flex().gap_1p5().children(ordered_qa.into_iter().enumerate().map(
                                |(idx, folder)| {
                                    let kind = folder.kind;
                                    let visible = ws.settings.is_quick_access_visible(kind);
                                    let label: SharedString = folder.name.clone().into();
                                    let icon = known_folder_icon(kind);
                                    let drag_payload = DraggedQuickAccessItem {
                                        kind,
                                        label: label.clone(),
                                    };

                                    h_flex()
                                        .id(("qa-row", idx))
                                        .w_full()
                                        .px_2p5()
                                        .py_1p5()
                                        .gap_2()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(RADIUS_CONTROL))
                                        .cursor_grab()
                                        .bg(solid(p.bg_surface))
                                        .border_1()
                                        .border_color(solid(p.border_subtle))
                                        .hover(move |s| s.border_color(solid(p.border)))
                                        .on_drag(drag_payload, move |drag, cursor_offset, _, cx| {
                                            cx.new(|_| SidebarDragGhost {
                                                label: drag.label.clone(),
                                                palette: p,
                                                cursor_offset,
                                            })
                                        })
                                        .drag_over::<DraggedQuickAccessItem>(
                                            move |style, drag, _, _| {
                                                if drag.kind != kind {
                                                    style
                                                        .bg(soft_tint(p.accent_primary, 0.14))
                                                        .border_color(solid(p.accent_primary))
                                                } else {
                                                    style
                                                }
                                            },
                                        )
                                        .on_drop(cx.listener(
                                            move |this, drag: &DraggedQuickAccessItem, _, cx| {
                                                cx.stop_propagation();
                                                this.reorder_quick_access(drag.kind, kind, cx);
                                            },
                                        ))
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .items_center()
                                                .gap_2()
                                                .child(drag_grip_handle(p))
                                                .child(Icon::new(icon).xsmall().text_color(
                                                    if visible {
                                                        solid(p.accent_primary)
                                                    } else {
                                                        solid(p.text_muted)
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(if visible {
                                                            solid(p.text_primary)
                                                        } else {
                                                            solid(p.text_muted)
                                                        })
                                                        .child(label),
                                                ),
                                        )
                                        .child(choice_pill(
                                            ("qa-vis", idx),
                                            if visible { "Visible" } else { "Hidden" },
                                            visible,
                                            p,
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle_quick_access(kind, cx);
                                            }),
                                        ))
                                },
                            ))
                        }),
                )
                .child(
                    h_flex()
                        .p_3()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .border_t_1()
                        .border_color(solid(p.border_subtle))
                        .child(setting_label(
                            "P2P Transfer Row in Network",
                            "Show the text \"P2P Transfer\" entry inside the Network sidebar section (disabled by default; the Network header button & Home Tools card stay available).",
                            p,
                        ))
                        .child(choice_pill(
                            "sb-p2p-row-vis",
                            if ws.settings.show_sidebar_p2p_row {
                                "Visible"
                            } else {
                                "Hidden"
                            },
                            ws.settings.show_sidebar_p2p_row,
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_sidebar_p2p_row(cx);
                            }),
                        )),
                )
                .when(!ws.settings.pinned_folders.is_empty(), |el| {
                    el.child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .border_t_1()
                            .border_color(solid(p.border_subtle))
                            .child(setting_label(
                                "Pinned Folders",
                                "Drag pinned folders to reorder them (you can also drag them directly inside the left sidebar).",
                                p,
                            ))
                            .child(v_flex().gap_1p5().children(
                                ws.settings.pinned_folders.iter().enumerate().map(|(idx, path)| {
                                    let label: SharedString = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| path.display().to_string())
                                        .into();
                                    let path_over = path.clone();
                                    let path_drop = path.clone();
                                    let path_unpin = path.clone();
                                    let drag_payload = DraggedPinnedFolder {
                                        path: path.clone(),
                                        label: label.clone(),
                                    };
                                    h_flex()
                                        .id(("pin-set-row", idx))
                                        .w_full()
                                        .px_2p5()
                                        .py_1p5()
                                        .gap_2()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(RADIUS_CONTROL))
                                        .cursor_grab()
                                        .bg(solid(p.bg_surface))
                                        .border_1()
                                        .border_color(solid(p.border_subtle))
                                        .hover(move |s| s.border_color(solid(p.border)))
                                        .on_drag(drag_payload, move |drag, cursor_offset, _, cx| {
                                            cx.new(|_| SidebarDragGhost {
                                                label: drag.label.clone(),
                                                palette: p,
                                                cursor_offset,
                                            })
                                        })
                                        .drag_over::<DraggedPinnedFolder>(
                                            move |style, drag, _, _| {
                                                if drag.path != path_over {
                                                    style
                                                        .bg(soft_tint(p.accent_primary, 0.14))
                                                        .border_color(solid(p.accent_primary))
                                                } else {
                                                    style
                                                }
                                            },
                                        )
                                        .on_drop(cx.listener(
                                            move |this, drag: &DraggedPinnedFolder, _, cx| {
                                                cx.stop_propagation();
                                                this.reorder_pinned_folder(
                                                    drag.path.clone(),
                                                    path_drop.clone(),
                                                    cx,
                                                );
                                            },
                                        ))
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .items_center()
                                                .gap_2()
                                                .child(drag_grip_handle(p))
                                                .child(
                                                    Icon::new(LucideIcon::Pin)
                                                        .xsmall()
                                                        .text_color(solid(p.accent_primary)),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(solid(p.text_primary))
                                                        .child(label),
                                                ),
                                        )
                                        .child(
                                            Button::new(("pin-set-unpin", idx))
                                                .with_size(Size::XSmall)
                                                .ghost()
                                                .icon(LucideIcon::X)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.unpin_folder(&path_unpin, cx);
                                                })),
                                        )
                                }),
                            )),
                    )
                }),
        );

    let search_section = v_flex()
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
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.install_mft_service(cx);
                                            })),
                                    )
                                })
                                .when(svc_supported && svc_stopped, |el| {
                                    el.child(
                                        Button::new("svc-start")
                                            .with_size(Size::XSmall)
                                            .primary()
                                            .disabled(svc_busy)
                                            .label("Start")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.start_mft_service(cx);
                                            })),
                                    )
                                })
                                .when(svc_supported && svc_running, |el| {
                                    el.child(
                                        Button::new("svc-rebuild")
                                            .with_size(Size::XSmall)
                                            .disabled(svc_busy)
                                            .label("Rebuild")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.rebuild_mft_index(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("svc-uninstall")
                                            .with_size(Size::XSmall)
                                            .danger()
                                            .disabled(svc_busy)
                                            .label("Uninstall")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.uninstall_mft_service(cx);
                                            })),
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
        );

    let windows_section = v_flex()
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
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.enable_default_file_manager(cx);
                                            })),
                                    )
                                })
                                .when(is_windows && is_enabled, |el| {
                                    el.child(
                                        Button::new("win-restore-default")
                                            .with_size(Size::XSmall)
                                            .danger()
                                            .label("Restore Windows Explorer")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.restore_default_file_manager(cx);
                                            })),
                                    )
                                }),
                        ),
                )
                .when(is_windows, |el| {
                    let daemon_running = texplorer_platform::is_win_e_daemon_running();
                    el.child(
                        h_flex()
                            .p_3()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(setting_label(
                                "Intercept Win+E Shortcut",
                                "Opens TExplorer on Win+E across all Windows 10 & Windows 11 versions (including 26H2) via HKCU Shell routing and a tiny background shortcut listener (no reboot or UAC).",
                                p,
                            ))
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap_2()
                                    .when(daemon_running, |row| {
                                        row.child(
                                            div()
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(RADIUS_CONTROL))
                                                .bg(soft_tint(p.status_success, 0.14))
                                                .text_size(px(13.))
                                                .font_medium()
                                                .text_color(solid(p.status_success))
                                                .child("Active"),
                                        )
                                    })
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
                            ),
                    )
                }),
        );

    let updates_section = v_flex()
        .gap_2()
        .child(section_heading("UPDATES & ABOUT", p))
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
                                        .child(format!("TExplorer v{CURRENT_VERSION}")),
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
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.start_update_install(cx);
                                            })),
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
        );

    let active_content = match active_category {
        SettingsCategory::Appearance => appearance_section.into_any_element(),
        SettingsCategory::Interface => interface_section.into_any_element(),
        SettingsCategory::Search => search_section.into_any_element(),
        SettingsCategory::Windows => windows_section.into_any_element(),
        SettingsCategory::Updates => updates_section.into_any_element(),
    };

    v_flex()
        .id("settings-modal-card")
        .w(px(820.))
        .h(px(560.))
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
                .h(px(44.))
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
        // Two-column split body: left category rail + right scrollable section content
        .child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                // Left Category Sidebar
                .child(
                    v_flex()
                        .id("settings-category-rail")
                        .w(px(214.))
                        .h_full()
                        .flex_shrink_0()
                        .p_2p5()
                        .gap_1()
                        .bg(solid(p.bg_base))
                        .border_r_1()
                        .border_color(solid(p.border_subtle))
                        .children(SettingsCategory::ALL.into_iter().map(|cat| {
                            let selected = active_category == cat;
                            let dot_color: Option<u32> = match cat {
                                SettingsCategory::Updates if has_update_badge => {
                                    Some(p.accent_primary)
                                }
                                _ => None,
                            };

                            h_flex()
                                .id(cat.id())
                                .w_full()
                                .h(px(36.))
                                .px_2p5()
                                .gap_2()
                                .items_center()
                                .justify_between()
                                .rounded(px(RADIUS_CONTROL))
                                .cursor_pointer()
                                .bg(if selected {
                                    soft_tint(p.accent_primary, if p.is_dark { 0.16 } else { 0.12 })
                                } else {
                                    gpui_kit::transparent_black()
                                })
                                .border_1()
                                .border_color(if selected {
                                    soft_tint(p.accent_primary, 0.35)
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
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_settings_category(cat, cx);
                                }))
                                .child(
                                    h_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Icon::new(cat.icon()).xsmall().text_color(if selected {
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
                                                .font_medium()
                                                .text_color(if selected {
                                                    solid(p.text_primary)
                                                } else {
                                                    solid(p.text_secondary)
                                                })
                                                .child(cat.label()),
                                        ),
                                )
                                .when_some(dot_color, |el, hex| {
                                    el.child(
                                        div()
                                            .w(px(6.))
                                            .h(px(6.))
                                            .flex_shrink_0()
                                            .rounded(px(999.))
                                            .bg(solid(hex)),
                                    )
                                })
                        })),
                )
                // Right Scrollable Settings Content Pane
                .child(
                    v_flex()
                        .id("settings-content-pane")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scrollbar()
                        .p_4()
                        .gap_4()
                        .bg(solid(p.bg_surface))
                        .child(active_content),
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

/// Minimalist 6-dot drag handle grip rendered inside drag-reorderable Settings rows.
fn drag_grip_handle(p: Palette) -> impl IntoElement {
    let dot = || {
        div()
            .w(px(2.5))
            .h(px(2.5))
            .rounded(px(999.))
            .bg(solid(p.text_muted))
    };
    h_flex()
        .flex_shrink_0()
        .gap(px(2.5))
        .px_0p5()
        .items_center()
        .child(v_flex().gap(px(2.5)).child(dot()).child(dot()).child(dot()))
        .child(v_flex().gap(px(2.5)).child(dot()).child(dot()).child(dot()))
}

fn choice_pill(
    id: impl Into<gpui_kit::ElementId>,
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
