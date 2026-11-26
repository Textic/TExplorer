use gpui_kit::{
    Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, SharedString, Window,
    assets::IconName as LucideIcon,
    component::{
        Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::{AccentPreset, Location, PaneSide, ThemePreset, ViewMode};

use crate::theme::{RADIUS_CONTROL, RADIUS_ROW, full_hit_input, soft_tint, solid};
use crate::ui::workspace::Workspace;

/// Action dispatched when an entry is chosen from the `Ctrl+Shift+P` Command Palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    GlobalSearch,
    BulkRename,
    QuickLook,
    ComputeChecksums,
    CheckFileLocks,
    CompressZip,
    ExtractArchive,
    NewTab,
    CloseTab,
    GoHome,
    ToggleSplit,
    ToggleSidebar,
    ToggleInspector,
    ToggleHidden,
    ToggleTabs,
    ViewMode(ViewMode),
    NewFolder,
    NewFile,
    Undo,
    Redo,
    OpenTerminal,
    OpenEditor,
    OpenSettings,
    RebuildMftIndex,
    CheckUpdates,
    SetTheme(ThemePreset),
    SetAccent(AccentPreset),
}

#[derive(Clone, Copy)]
pub struct PaletteItem {
    pub id: &'static str,
    pub category: &'static str,
    pub title: &'static str,
    pub shortcut: Option<&'static str>,
    pub icon: LucideIcon,
    pub cmd: PaletteCommand,
}

/// Interactive state for the `Ctrl+Shift+P` Command Palette overlay.
pub struct CommandPaletteState {
    pub input: Entity<InputState>,
    pub query: String,
    pub selected_idx: usize,
}

pub fn all_palette_items() -> Vec<PaletteItem> {
    let mut items = vec![
        PaletteItem {
            id: "cp-global-search",
            category: "Search",
            title: "Global File Search (MFT / Parallel)",
            shortcut: Some("Ctrl+K"),
            icon: LucideIcon::Search,
            cmd: PaletteCommand::GlobalSearch,
        },
        PaletteItem {
            id: "cp-bulk-rename",
            category: "Tools",
            title: "Bulk Rename Selected Items",
            shortcut: Some("Ctrl+Shift+R"),
            icon: LucideIcon::Pencil,
            cmd: PaletteCommand::BulkRename,
        },
        PaletteItem {
            id: "cp-quicklook",
            category: "Preview",
            title: "Toggle QuickLook Preview",
            shortcut: Some("Space"),
            icon: LucideIcon::Eye,
            cmd: PaletteCommand::QuickLook,
        },
        PaletteItem {
            id: "cp-hashes",
            category: "Tools",
            title: "Compute SHA-256 & MD5 Checksums",
            shortcut: None,
            icon: LucideIcon::ShieldCheck,
            cmd: PaletteCommand::ComputeChecksums,
        },
        PaletteItem {
            id: "cp-locks",
            category: "Tools",
            title: "Check File Locks (Restart Manager)",
            shortcut: None,
            icon: LucideIcon::TriangleAlert,
            cmd: PaletteCommand::CheckFileLocks,
        },
        PaletteItem {
            id: "cp-zip",
            category: "Archive",
            title: "Compress Selected Items to ZIP",
            shortcut: None,
            icon: LucideIcon::FileArchive,
            cmd: PaletteCommand::CompressZip,
        },
        PaletteItem {
            id: "cp-extract",
            category: "Archive",
            title: "Extract Selected Archive Here",
            shortcut: None,
            icon: LucideIcon::FolderOpen,
            cmd: PaletteCommand::ExtractArchive,
        },
        PaletteItem {
            id: "cp-new-tab",
            category: "Navigation",
            title: "Open New Tab",
            shortcut: Some("Ctrl+T"),
            icon: LucideIcon::Plus,
            cmd: PaletteCommand::NewTab,
        },
        PaletteItem {
            id: "cp-close-tab",
            category: "Navigation",
            title: "Close Current Tab",
            shortcut: Some("Ctrl+W"),
            icon: LucideIcon::X,
            cmd: PaletteCommand::CloseTab,
        },
        PaletteItem {
            id: "cp-home",
            category: "Navigation",
            title: "Go to Home / This PC",
            shortcut: None,
            icon: LucideIcon::House,
            cmd: PaletteCommand::GoHome,
        },
        PaletteItem {
            id: "cp-split",
            category: "Layout",
            title: "Toggle Dual-Pane Split View",
            shortcut: Some("Ctrl+\\"),
            icon: LucideIcon::Columns2,
            cmd: PaletteCommand::ToggleSplit,
        },
        PaletteItem {
            id: "cp-sidebar",
            category: "Layout",
            title: "Toggle Left Sidebar",
            shortcut: Some("Ctrl+B"),
            icon: LucideIcon::List,
            cmd: PaletteCommand::ToggleSidebar,
        },
        PaletteItem {
            id: "cp-inspector",
            category: "Layout",
            title: "Toggle Preview & Inspector Panel",
            shortcut: Some("Alt+P"),
            icon: LucideIcon::Eye,
            cmd: PaletteCommand::ToggleInspector,
        },
        PaletteItem {
            id: "cp-hidden",
            category: "View",
            title: "Toggle Hidden Files",
            shortcut: Some("Ctrl+H"),
            icon: LucideIcon::EyeOff,
            cmd: PaletteCommand::ToggleHidden,
        },
        PaletteItem {
            id: "cp-tabs",
            category: "Layout",
            title: "Toggle Top Tab Bar",
            shortcut: None,
            icon: LucideIcon::LayoutList,
            cmd: PaletteCommand::ToggleTabs,
        },
        PaletteItem {
            id: "cp-vm-details",
            category: "View",
            title: "Switch View: Details Table",
            shortcut: None,
            icon: LucideIcon::LayoutList,
            cmd: PaletteCommand::ViewMode(ViewMode::Details),
        },
        PaletteItem {
            id: "cp-vm-grid",
            category: "View",
            title: "Switch View: Thumbnail Grid",
            shortcut: None,
            icon: LucideIcon::LayoutGrid,
            cmd: PaletteCommand::ViewMode(ViewMode::Grid),
        },
        PaletteItem {
            id: "cp-vm-compact",
            category: "View",
            title: "Switch View: Compact List",
            shortcut: None,
            icon: LucideIcon::List,
            cmd: PaletteCommand::ViewMode(ViewMode::Compact),
        },
        PaletteItem {
            id: "cp-new-folder",
            category: "File",
            title: "Create New Folder",
            shortcut: Some("Ctrl+Shift+N"),
            icon: LucideIcon::FolderPlus,
            cmd: PaletteCommand::NewFolder,
        },
        PaletteItem {
            id: "cp-new-file",
            category: "File",
            title: "Create New Empty File",
            shortcut: None,
            icon: LucideIcon::FilePlus,
            cmd: PaletteCommand::NewFile,
        },
        PaletteItem {
            id: "cp-undo",
            category: "Edit",
            title: "Undo Last File Operation",
            shortcut: Some("Ctrl+Z"),
            icon: LucideIcon::Undo2,
            cmd: PaletteCommand::Undo,
        },
        PaletteItem {
            id: "cp-redo",
            category: "Edit",
            title: "Redo Last File Operation",
            shortcut: Some("Ctrl+Y"),
            icon: LucideIcon::Redo2,
            cmd: PaletteCommand::Redo,
        },
        PaletteItem {
            id: "cp-terminal",
            category: "Developer",
            title: "Open Terminal in Active Folder",
            shortcut: None,
            icon: LucideIcon::Terminal,
            cmd: PaletteCommand::OpenTerminal,
        },
        PaletteItem {
            id: "cp-editor",
            category: "Developer",
            title: "Open Active Folder in Code Editor",
            shortcut: None,
            icon: LucideIcon::Code,
            cmd: PaletteCommand::OpenEditor,
        },
        PaletteItem {
            id: "cp-settings",
            category: "App",
            title: "Open Settings",
            shortcut: Some("Ctrl+,"),
            icon: LucideIcon::Settings,
            cmd: PaletteCommand::OpenSettings,
        },
        PaletteItem {
            id: "cp-mft-rebuild",
            category: "Search",
            title: "Rebuild NTFS $MFT Index Service",
            shortcut: None,
            icon: LucideIcon::RefreshCw,
            cmd: PaletteCommand::RebuildMftIndex,
        },
        PaletteItem {
            id: "cp-updates",
            category: "App",
            title: "Check for TExplorer Updates",
            shortcut: None,
            icon: LucideIcon::Download,
            cmd: PaletteCommand::CheckUpdates,
        },
    ];

    for theme in ThemePreset::ALL {
        let (id, title) = match theme {
            ThemePreset::FilesSystem => ("cp-th-sys", "Theme: System (Files Auto)"),
            ThemePreset::FilesDark => ("cp-th-fdark", "Theme: Files Dark"),
            ThemePreset::FilesLight => ("cp-th-flight", "Theme: Files Light"),
            ThemePreset::ObsidianDark => ("cp-th-obs", "Theme: Obsidian Dark"),
        };
        items.push(PaletteItem {
            id,
            category: "Theme",
            title,
            shortcut: None,
            icon: LucideIcon::Settings,
            cmd: PaletteCommand::SetTheme(theme),
        });
    }

    for accent in AccentPreset::ALL {
        let (id, title) = match accent {
            AccentPreset::FilesBlue => ("cp-ac-fblue", "Accent: Files Blue"),
            AccentPreset::FilesCobalt => ("cp-ac-fcob", "Accent: Files Cobalt"),
            AccentPreset::FilesTeal => ("cp-ac-fmint", "Accent: Files Mint"),
            AccentPreset::Emerald => ("cp-ac-emer", "Accent: Obsidian Emerald"),
            AccentPreset::Violet => ("cp-ac-viol", "Accent: Obsidian Violet"),
            AccentPreset::Monochrome => ("cp-ac-mono", "Accent: Obsidian Mono"),
        };
        items.push(PaletteItem {
            id,
            category: "Accent",
            title,
            shortcut: None,
            icon: LucideIcon::Settings,
            cmd: PaletteCommand::SetAccent(accent),
        });
    }

    items
}

pub fn filtered_palette_items(query: &str) -> Vec<PaletteItem> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    let all = all_palette_items();
    if terms.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|item| {
            let hay = format!("{} {}", item.category, item.title).to_ascii_lowercase();
            terms.iter().all(|t| hay.contains(t.as_str()))
        })
        .collect()
}

/// Executes a chosen Command Palette action on `Workspace`.
pub fn execute_palette_command(
    ws: &mut Workspace,
    cmd: PaletteCommand,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let side = ws
        .active_tab()
        .map(|t| t.model.active_pane)
        .unwrap_or(PaneSide::Left);

    match cmd {
        PaletteCommand::GlobalSearch => ws.open_global_search(window, cx),
        PaletteCommand::BulkRename => ws.open_bulk_rename_for_pane(side, window, cx),
        PaletteCommand::QuickLook => ws.toggle_quicklook(cx),
        PaletteCommand::ComputeChecksums => ws.compute_checksums_for_selection(cx),
        PaletteCommand::CheckFileLocks => ws.check_locks_for_selection(cx),
        PaletteCommand::CompressZip => ws.compress_selected_to_zip(side, window, cx),
        PaletteCommand::ExtractArchive => ws.extract_selected_archive_here(side, window, cx),
        PaletteCommand::NewTab => ws.open_new_tab(Location::Home, window, cx),
        PaletteCommand::CloseTab => {
            let idx = ws.active_tab_index;
            ws.close_tab_at(idx, window, cx);
        }
        PaletteCommand::GoHome => ws.navigate_active_pane(Location::Home, window, cx),
        PaletteCommand::ToggleSplit => ws.toggle_split_active_tab(window, cx),
        PaletteCommand::ToggleSidebar => ws.toggle_sidebar(cx),
        PaletteCommand::ToggleInspector => ws.toggle_inspector(cx),
        PaletteCommand::ToggleHidden => ws.toggle_hidden_for_pane(side, cx),
        PaletteCommand::ToggleTabs => ws.toggle_show_tabs(cx),
        PaletteCommand::ViewMode(m) => ws.set_pane_view_mode(side, m, cx),
        PaletteCommand::NewFolder => {
            if let Some(dir) = ws
                .active_tab()
                .and_then(|t| t.model.pane(side).location.as_local_path())
                .map(|p| p.to_path_buf())
            {
                ws.open_new_folder_prompt(side, dir, window, cx);
            }
        }
        PaletteCommand::NewFile => {
            if let Some(dir) = ws
                .active_tab()
                .and_then(|t| t.model.pane(side).location.as_local_path())
                .map(|p| p.to_path_buf())
            {
                ws.open_new_file_prompt(side, dir, window, cx);
            }
        }
        PaletteCommand::Undo => ws.undo_last_action(window, cx),
        PaletteCommand::Redo => ws.redo_last_action(window, cx),
        PaletteCommand::OpenTerminal => {
            if let Some(dir) = ws
                .active_tab()
                .and_then(|t| t.model.pane(side).location.as_local_path())
                .map(|p| p.to_path_buf())
            {
                ws.open_terminal_at(&dir, cx);
            }
        }
        PaletteCommand::OpenEditor => {
            if let Some(dir) = ws
                .active_tab()
                .and_then(|t| t.model.pane(side).location.as_local_path())
                .map(|p| p.to_path_buf())
            {
                ws.open_editor_at(&dir, cx);
            }
        }
        PaletteCommand::OpenSettings => ws.open_settings_modal(window, cx),
        PaletteCommand::RebuildMftIndex => ws.rebuild_mft_index(cx),
        PaletteCommand::CheckUpdates => {
            ws.open_settings_modal(window, cx);
            ws.check_for_updates(true, cx);
        }
        PaletteCommand::SetTheme(theme) => ws.set_theme(theme, cx),
        PaletteCommand::SetAccent(accent) => ws.set_accent(accent, cx),
    }
}

/// Renders the `Ctrl+Shift+P` Command Palette floating modal.
pub fn render_command_palette_modal(
    ws: &Workspace,
    state: &CommandPaletteState,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let items = filtered_palette_items(&state.query);
    let selected_idx = state.selected_idx.min(items.len().saturating_sub(1));

    div()
        .id("cmd-palette-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(86.))
        .occlude()
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
            match ev.keystroke.key.as_str() {
                "up" => {
                    this.move_command_palette_selection(-1, cx);
                    cx.stop_propagation();
                }
                "down" => {
                    this.move_command_palette_selection(1, cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    this.close_command_palette(window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
        .child(
            div()
                .id("cmd-palette-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, p.backdrop_alpha()))
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_command_palette(window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_command_palette(window, cx);
                    }),
                ),
        )
        .child(
            v_flex()
                .id("cmd-palette-card")
                .relative()
                .w(px(620.))
                .max_h(px(480.))
                .bg(solid(p.bg_surface))
                .border_1()
                .border_color(solid(p.border))
                .rounded(px(RADIUS_ROW))
                .overflow_hidden()
                .occlude()
                .child(
                    h_flex()
                        .w_full()
                        .h(px(44.))
                        .px_3p5()
                        .gap_2()
                        .flex_shrink_0()
                        .items_center()
                        .bg(solid(p.bg_elevated))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            Icon::new(LucideIcon::Terminal)
                                .small()
                                .text_color(solid(p.accent_primary)),
                        )
                        .child(
                            full_hit_input(
                                "cmd-palette-input-hit",
                                &state.input,
                                Input::new(&state.input)
                                    .with_size(Size::Small)
                                    .appearance(false),
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(
                            Button::new("cp-close")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::X)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_command_palette(window, cx);
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .id("cmd-palette-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .p_1p5()
                        .gap_0p5()
                        .bg(solid(p.bg_base))
                        .when(items.is_empty(), |el| {
                            el.child(
                                div()
                                    .py_6()
                                    .text_center()
                                    .text_xs()
                                    .text_color(solid(p.text_muted))
                                    .child("No matching commands"),
                            )
                        })
                        .children(items.into_iter().enumerate().map(|(idx, item)| {
                            let selected = idx == selected_idx;
                            let cmd = item.cmd;
                            let cat: SharedString = item.category.into();
                            let title: SharedString = item.title.into();

                            h_flex()
                                .id(item.id)
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
                                .hover(move |s| {
                                    if selected {
                                        s
                                    } else {
                                        s.bg(solid(p.bg_hover))
                                    }
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        this.close_command_palette(window, cx);
                                        execute_palette_command(this, cmd, window, cx);
                                    }),
                                )
                                .child(
                                    h_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_2()
                                        .items_center()
                                        .child(Icon::new(item.icon).xsmall().text_color(
                                            solid(if selected {
                                                p.accent_primary
                                            } else {
                                                p.text_secondary
                                            }),
                                        ))
                                        .child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded(px(3.))
                                                .bg(solid(p.bg_elevated))
                                                .text_size(px(12.))
                                                .text_color(solid(p.text_muted))
                                                .child(cat),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(solid(p.text_primary))
                                                .child(title),
                                        ),
                                )
                                .when_some(item.shortcut, |el, sc| {
                                    el.child(
                                        div()
                                            .flex_shrink_0()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(3.))
                                            .bg(solid(p.bg_surface))
                                            .border_1()
                                            .border_color(solid(p.border_subtle))
                                            .text_size(px(12.))
                                            .text_color(solid(p.text_muted))
                                            .child(sc),
                                    )
                                })
                        })),
                ),
        )
}
