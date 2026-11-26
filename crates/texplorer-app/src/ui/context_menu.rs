use std::path::PathBuf;

use gpui_kit::{
    Context, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Window,
    assets::IconName as LucideIcon,
    component::{Icon, IconName, Sizable as _, h_flex, v_flex},
    div, prelude::*, px,
};
use texplorer_core::{
    FileEntry, KnownFolder, KnownFolderKind, Location, PaneSide, SidebarSection, VolumeInfo,
};

use crate::theme::{RADIUS_CONTROL, RADIUS_ROW, soft_tint, solid};
use crate::ui::modals::SettingsCategory;
use crate::ui::workspace::Workspace;

#[derive(Debug, Clone)]
pub enum ContextMenuTarget {
    Entry {
        side: PaneSide,
        entry: FileEntry,
    },
    PaneBackground {
        side: PaneSide,
        directory: PathBuf,
    },
    KnownFolder(KnownFolder),
    Volume(VolumeInfo),
    SidebarSection(SidebarSection),
    SidebarBackground,
}

#[derive(Debug, Clone)]
pub struct ContextMenuState {
    pub position: Point<Pixels>,
    pub target: ContextMenuTarget,
}

enum MenuRow {
    Separator,
    Item {
        id: &'static str,
        label: SharedString,
        shortcut: Option<&'static str>,
        icon: MenuIcon,
        danger: bool,
        action: MenuAction,
    },
}

enum MenuIcon {
    Lucide(LucideIcon),
    Kit(IconName),
}

#[derive(Clone)]
enum MenuAction {
    OpenEntry(Location, bool),
    QuickLook,
    OpenNewTab(Location),
    OpenOppositePane(Location),
    CutSelected(PaneSide),
    CopySelected(PaneSide),
    PasteInto(PathBuf),
    RenameEntry(PaneSide, PathBuf, String),
    BulkRename(PaneSide),
    CompressToZip(PaneSide),
    ExtractArchiveHere(PathBuf),
    ExtractArchiveSelection(PaneSide, PathBuf, String),
    ComputeChecksums,
    CheckFileLocks,
    NewFolder(PaneSide, PathBuf),
    NewFile(PaneSide, PathBuf),
    CopyPath(String),
    OpenTerminal(PathBuf),
    OpenEditor(PathBuf),
    PinFolder(PathBuf),
    ShowWindowsMenu(Vec<PathBuf>),
    RevealInWindowsExplorer(PathBuf),
    ShowProperties(PathBuf),
    TrashSelected(PaneSide),
    PermanentDeleteSelected(PaneSide),
    RefreshPane(PaneSide),
    HideQuickAccess(KnownFolderKind),
    ToggleSidebarSectionCollapsed(SidebarSection),
    HideSidebarSection(SidebarSection),
    OpenSidebarSettings,
}

/// Renders the flat floating context menu clamped cleanly inside the viewport.
pub fn render_context_menu(
    ws: &Workspace,
    menu: &ContextMenuState,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let rows = build_rows(ws, &menu.target);
    if rows.is_empty() {
        return div().into_any_element();
    }

    const MENU_W: f32 = 286.0;
    const ITEM_H: f32 = 31.0;
    const SEP_H: f32 = 9.0;
    let est_h: f32 = 10.0
        + rows
            .iter()
            .map(|r| match r {
                MenuRow::Separator => SEP_H,
                MenuRow::Item { .. } => ITEM_H,
            })
            .sum::<f32>();

    let vp = window.viewport_size();
    let max_x = (f32::from(vp.width) - MENU_W - 8.0).max(8.0);
    let max_y = (f32::from(vp.height) - est_h - 8.0).max(8.0);
    let left = f32::from(menu.position.x).clamp(8.0, max_x);
    let top = f32::from(menu.position.y).clamp(8.0, max_y);

    div()
        .id("ctx-menu-overlay")
        .absolute()
        .inset_0()
        .occlude()
        .child(
            div()
                .id("ctx-menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_scroll(|_, _, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.dismiss_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.dismiss_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.dismiss_context_menu(cx);
                    }),
                ),
        )
        .child(
            v_flex()
                .id("ctx-menu-card")
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(MENU_W))
                .p_1()
                .bg(solid(p.bg_elevated))
                .border_1()
                .border_color(solid(p.border))
                .rounded(px(RADIUS_ROW))
                .occlude()
                .children(rows.into_iter().map(|row| match row {
                    MenuRow::Separator => div()
                        .w_full()
                        .h(px(1.))
                        .my_1()
                        .bg(solid(p.border_subtle))
                        .into_any_element(),
                    MenuRow::Item {
                        id,
                        label,
                        shortcut,
                        icon,
                        danger,
                        action,
                    } => {
                        let text_hex = if danger {
                            p.status_error
                        } else {
                            p.text_primary
                        };
                        let icon_hex = if danger {
                            p.status_error
                        } else {
                            p.text_secondary
                        };
                        let icon_el = match icon {
                            MenuIcon::Lucide(i) => {
                                Icon::new(i).xsmall().text_color(solid(icon_hex))
                            }
                            MenuIcon::Kit(i) => Icon::new(i).xsmall().text_color(solid(icon_hex)),
                        };

                        h_flex()
                            .id(id)
                            .w_full()
                            .h(px(ITEM_H))
                            .px_2()
                            .gap_2()
                            .items_center()
                            .justify_between()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .hover(move |s| {
                                if danger {
                                    s.bg(soft_tint(p.status_error, 0.14))
                                } else {
                                    s.bg(solid(p.bg_hover))
                                }
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    this.dismiss_context_menu(cx);
                                    execute_action(this, action.clone(), window, cx);
                                }),
                            )
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .items_center()
                                    .child(icon_el)
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(solid(text_hex))
                                            .child(label),
                                    ),
                            )
                            .when_some(shortcut, |el, sc| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(12.))
                                        .text_color(solid(p.text_muted))
                                        .child(sc),
                                )
                            })
                            .into_any_element()
                    }
                })),
        )
        .into_any_element()
}

fn build_rows(ws: &Workspace, target: &ContextMenuTarget) -> Vec<MenuRow> {
    match target {
        ContextMenuTarget::Entry { side, entry } => {
            let mut rows = Vec::new();
            let is_dir = entry.is_dir();
            let is_archive_file = !is_dir
                && entry
                    .location
                    .as_local_path()
                    .is_some_and(texplorer_vfs::archive::is_supported_archive);
            rows.push(MenuRow::Item {
                id: "ctx-open",
                label: if is_archive_file {
                    "Browse Archive".into()
                } else {
                    "Open".into()
                },
                shortcut: Some("Enter"),
                icon: MenuIcon::Lucide(if is_archive_file {
                    LucideIcon::FileArchive
                } else if is_dir {
                    LucideIcon::FolderOpen
                } else {
                    LucideIcon::ExternalLink
                }),
                danger: false,
                action: MenuAction::OpenEntry(entry.location.clone(), is_dir),
            });
            rows.push(MenuRow::Item {
                id: "ctx-quicklook",
                label: "QuickLook Preview".into(),
                shortcut: Some("Space"),
                icon: MenuIcon::Lucide(LucideIcon::Eye),
                danger: false,
                action: MenuAction::QuickLook,
            });

            if is_dir || is_archive_file {
                let nav_target = if is_archive_file
                    && let Some(path) = entry.location.as_local_path()
                {
                    Location::archive(path, "")
                } else {
                    entry.location.clone()
                };
                if ws.settings.show_tabs {
                    rows.push(MenuRow::Item {
                        id: "ctx-open-tab",
                        label: "Open in New Tab".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::Plus),
                        danger: false,
                        action: MenuAction::OpenNewTab(nav_target.clone()),
                    });
                }
                rows.push(MenuRow::Item {
                    id: "ctx-open-split",
                    label: "Open in Split Pane".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Columns2),
                    danger: false,
                    action: MenuAction::OpenOppositePane(nav_target),
                });
            }

            if let Some(path) = entry.location.as_local_path() {
                let path_buf = path.to_path_buf();
                let mut selected_paths = ws.selected_local_paths(*side);
                if selected_paths.is_empty() {
                    selected_paths.push(path_buf.clone());
                }

                if is_archive_file {
                    rows.push(MenuRow::Item {
                        id: "ctx-extract-here",
                        label: "Extract Archive Here".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::Download),
                        danger: false,
                        action: MenuAction::ExtractArchiveHere(path_buf.clone()),
                    });
                }

                rows.push(MenuRow::Separator);
                rows.push(MenuRow::Item {
                    id: "ctx-cut",
                    label: "Cut".into(),
                    shortcut: Some("Ctrl+X"),
                    icon: MenuIcon::Lucide(LucideIcon::Scissors),
                    danger: false,
                    action: MenuAction::CutSelected(*side),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-copy",
                    label: "Copy".into(),
                    shortcut: Some("Ctrl+C"),
                    icon: MenuIcon::Lucide(LucideIcon::Copy),
                    danger: false,
                    action: MenuAction::CopySelected(*side),
                });
                if is_dir {
                    rows.push(MenuRow::Item {
                        id: "ctx-paste-into",
                        label: "Paste Into Folder".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::Clipboard),
                        danger: false,
                        action: MenuAction::PasteInto(path_buf.clone()),
                    });
                }
                rows.push(MenuRow::Item {
                    id: "ctx-rename",
                    label: "Rename".into(),
                    shortcut: Some("F2"),
                    icon: MenuIcon::Lucide(LucideIcon::Pencil),
                    danger: false,
                    action: MenuAction::RenameEntry(*side, path_buf.clone(), entry.name.clone()),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-bulk-rename",
                    label: "Bulk Rename...".into(),
                    shortcut: Some("Ctrl+Shift+R"),
                    icon: MenuIcon::Lucide(LucideIcon::Pencil),
                    danger: false,
                    action: MenuAction::BulkRename(*side),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-compress-zip",
                    label: "Compress to ZIP".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::FileArchive),
                    danger: false,
                    action: MenuAction::CompressToZip(*side),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-copy-path",
                    label: "Copy Path".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Link),
                    danger: false,
                    action: MenuAction::CopyPath(entry.location.display_path()),
                });

                if !is_dir {
                    rows.push(MenuRow::Separator);
                    rows.push(MenuRow::Item {
                        id: "ctx-checksums",
                        label: "Compute Checksums".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::ShieldCheck),
                        danger: false,
                        action: MenuAction::ComputeChecksums,
                    });
                    rows.push(MenuRow::Item {
                        id: "ctx-file-locks",
                        label: "Check File Locks".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::TriangleAlert),
                        danger: false,
                        action: MenuAction::CheckFileLocks,
                    });
                }

                rows.push(MenuRow::Separator);
                rows.push(MenuRow::Item {
                    id: "ctx-terminal",
                    label: "Open Terminal Here".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Terminal),
                    danger: false,
                    action: MenuAction::OpenTerminal(path_buf.clone()),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-editor",
                    label: "Open in Editor".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Code),
                    danger: false,
                    action: MenuAction::OpenEditor(path_buf.clone()),
                });
                if is_dir {
                    rows.push(MenuRow::Item {
                        id: "ctx-pin",
                        label: "Pin to Sidebar".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::Pin),
                        danger: false,
                        action: MenuAction::PinFolder(path_buf.clone()),
                    });
                }

                rows.push(MenuRow::Separator);
                rows.push(MenuRow::Item {
                    id: "ctx-win-shell-menu",
                    label: "Show Windows Menu".into(),
                    shortcut: Some("Shift+RClick"),
                    icon: MenuIcon::Lucide(LucideIcon::ExternalLink),
                    danger: false,
                    action: MenuAction::ShowWindowsMenu(selected_paths),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-win-explorer",
                    label: "Reveal in Windows Explorer".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Folder),
                    danger: false,
                    action: MenuAction::RevealInWindowsExplorer(path_buf.clone()),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-props",
                    label: "Properties".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Settings),
                    danger: false,
                    action: MenuAction::ShowProperties(path_buf),
                });

                rows.push(MenuRow::Separator);
                rows.push(MenuRow::Item {
                    id: "ctx-trash",
                    label: "Move to Recycle Bin".into(),
                    shortcut: Some("Del"),
                    icon: MenuIcon::Kit(IconName::Delete),
                    danger: true,
                    action: MenuAction::TrashSelected(*side),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-perm-del",
                    label: "Delete Permanently...".into(),
                    shortcut: Some("Shift+Del"),
                    icon: MenuIcon::Kit(IconName::Delete),
                    danger: true,
                    action: MenuAction::PermanentDeleteSelected(*side),
                });
            } else if let Location::Archive {
                archive_path,
                inner_path,
            } = &entry.location
            {
                rows.push(MenuRow::Separator);
                rows.push(MenuRow::Item {
                    id: "ctx-arch-extract",
                    label: "Extract Item(s)".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Download),
                    danger: false,
                    action: MenuAction::ExtractArchiveSelection(
                        *side,
                        archive_path.clone(),
                        inner_path.clone(),
                    ),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-arch-copy-path",
                    label: "Copy Path".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Link),
                    danger: false,
                    action: MenuAction::CopyPath(entry.location.display_path()),
                });
            }
            rows
        }
        ContextMenuTarget::PaneBackground { side, directory } => vec![
            MenuRow::Item {
                id: "ctx-bg-paste",
                label: "Paste".into(),
                shortcut: Some("Ctrl+V"),
                icon: MenuIcon::Lucide(LucideIcon::Clipboard),
                danger: false,
                action: MenuAction::PasteInto(directory.clone()),
            },
            MenuRow::Separator,
            MenuRow::Item {
                id: "ctx-bg-new-folder",
                label: "New Folder".into(),
                shortcut: Some("Ctrl+Shift+N"),
                icon: MenuIcon::Lucide(LucideIcon::FolderPlus),
                danger: false,
                action: MenuAction::NewFolder(*side, directory.clone()),
            },
            MenuRow::Item {
                id: "ctx-bg-new-file",
                label: "New File".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::FilePlus),
                danger: false,
                action: MenuAction::NewFile(*side, directory.clone()),
            },
            MenuRow::Separator,
            MenuRow::Item {
                id: "ctx-bg-refresh",
                label: "Refresh".into(),
                shortcut: Some("F5"),
                icon: MenuIcon::Lucide(LucideIcon::RefreshCw),
                danger: false,
                action: MenuAction::RefreshPane(*side),
            },
            MenuRow::Item {
                id: "ctx-bg-copy-path",
                label: "Copy Folder Path".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::Copy),
                danger: false,
                action: MenuAction::CopyPath(directory.display().to_string()),
            },
            MenuRow::Item {
                id: "ctx-bg-terminal",
                label: "Open Terminal Here".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::Terminal),
                danger: false,
                action: MenuAction::OpenTerminal(directory.clone()),
            },
            MenuRow::Item {
                id: "ctx-bg-pin",
                label: "Pin Current Folder".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::Pin),
                danger: false,
                action: MenuAction::PinFolder(directory.clone()),
            },
            MenuRow::Separator,
            MenuRow::Item {
                id: "ctx-bg-win-menu",
                label: "Show Windows Menu".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::ExternalLink),
                danger: false,
                action: MenuAction::ShowWindowsMenu(vec![directory.clone()]),
            },
            MenuRow::Item {
                id: "ctx-bg-props",
                label: "Folder Properties".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::Settings),
                danger: false,
                action: MenuAction::ShowProperties(directory.clone()),
            },
        ],
        ContextMenuTarget::KnownFolder(folder) => {
            let loc = Location::local(&folder.path);
            let mut rows = vec![MenuRow::Item {
                id: "ctx-qa-open",
                label: "Open".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::FolderOpen),
                danger: false,
                action: MenuAction::OpenEntry(loc.clone(), true),
            }];
            if ws.settings.show_tabs {
                rows.push(MenuRow::Item {
                    id: "ctx-qa-tab",
                    label: "Open in New Tab".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Plus),
                    danger: false,
                    action: MenuAction::OpenNewTab(loc.clone()),
                });
            }
            rows.extend([
                MenuRow::Item {
                    id: "ctx-qa-split",
                    label: "Open in Split Pane".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Columns2),
                    danger: false,
                    action: MenuAction::OpenOppositePane(loc),
                },
                MenuRow::Separator,
                MenuRow::Item {
                    id: "ctx-qa-copy-path",
                    label: "Copy Path".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Copy),
                    danger: false,
                    action: MenuAction::CopyPath(folder.path.display().to_string()),
                },
                MenuRow::Item {
                    id: "ctx-qa-terminal",
                    label: "Open Terminal Here".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Terminal),
                    danger: false,
                    action: MenuAction::OpenTerminal(folder.path.clone()),
                },
                MenuRow::Separator,
                MenuRow::Item {
                    id: "ctx-qa-hide",
                    label: "Remove from Quick Access".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::EyeOff),
                    danger: true,
                    action: MenuAction::HideQuickAccess(folder.kind),
                },
                MenuRow::Item {
                    id: "ctx-qa-customize",
                    label: "Customize Sidebar...".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Settings),
                    danger: false,
                    action: MenuAction::OpenSidebarSettings,
                },
            ]);
            rows
        }
        ContextMenuTarget::Volume(vol) => {
            let loc = Location::local(&vol.root_path);
            let mut rows = vec![MenuRow::Item {
                id: "ctx-vol-open",
                label: "Open Drive".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::HardDrive),
                danger: false,
                action: MenuAction::OpenEntry(loc.clone(), true),
            }];
            if ws.settings.show_tabs {
                rows.push(MenuRow::Item {
                    id: "ctx-vol-tab",
                    label: "Open in New Tab".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Plus),
                    danger: false,
                    action: MenuAction::OpenNewTab(loc.clone()),
                });
            }
            rows.extend([
                MenuRow::Item {
                    id: "ctx-vol-split",
                    label: "Open in Split Pane".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Columns2),
                    danger: false,
                    action: MenuAction::OpenOppositePane(loc),
                },
                MenuRow::Separator,
                MenuRow::Item {
                    id: "ctx-vol-terminal",
                    label: "Open Terminal Here".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Terminal),
                    danger: false,
                    action: MenuAction::OpenTerminal(vol.root_path.clone()),
                },
                MenuRow::Item {
                    id: "ctx-vol-props",
                    label: "Drive Properties".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Settings),
                    danger: false,
                    action: MenuAction::ShowProperties(vol.root_path.clone()),
                },
                MenuRow::Separator,
                MenuRow::Item {
                    id: "ctx-vol-customize",
                    label: "Customize Sidebar...".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Settings),
                    danger: false,
                    action: MenuAction::OpenSidebarSettings,
                },
            ]);
            rows
        }
        ContextMenuTarget::SidebarSection(section) => {
            let collapsed = ws.settings.is_sidebar_section_collapsed(*section);
            vec![
                MenuRow::Item {
                    id: "ctx-sb-toggle-collapse",
                    label: if collapsed {
                        format!("Expand {}", section.label()).into()
                    } else {
                        format!("Collapse {}", section.label()).into()
                    },
                    shortcut: None,
                    icon: MenuIcon::Lucide(if collapsed {
                        LucideIcon::ChevronDown
                    } else {
                        LucideIcon::ChevronRight
                    }),
                    danger: false,
                    action: MenuAction::ToggleSidebarSectionCollapsed(*section),
                },
                MenuRow::Item {
                    id: "ctx-sb-hide-sec",
                    label: format!("Hide {}", section.label()).into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::EyeOff),
                    danger: false,
                    action: MenuAction::HideSidebarSection(*section),
                },
                MenuRow::Separator,
                MenuRow::Item {
                    id: "ctx-sb-customize",
                    label: "Customize Sidebar...".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Settings),
                    danger: false,
                    action: MenuAction::OpenSidebarSettings,
                },
            ]
        }
        ContextMenuTarget::SidebarBackground => vec![MenuRow::Item {
            id: "ctx-sb-bg-customize",
            label: "Customize Sidebar...".into(),
            shortcut: None,
            icon: MenuIcon::Lucide(LucideIcon::Settings),
            danger: false,
            action: MenuAction::OpenSidebarSettings,
        }],
    }
}

fn execute_action(
    ws: &mut Workspace,
    action: MenuAction,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    match action {
        MenuAction::OpenEntry(loc, is_dir) => ws.open_location_or_file(loc, is_dir, window, cx),
        MenuAction::QuickLook => ws.toggle_quicklook(cx),
        MenuAction::OpenNewTab(loc) => ws.open_new_tab(loc, window, cx),
        MenuAction::OpenOppositePane(loc) => ws.open_in_opposite_pane(loc, window, cx),
        MenuAction::CutSelected(side) => ws.copy_or_cut_selected(side, true, window, cx),
        MenuAction::CopySelected(side) => ws.copy_or_cut_selected(side, false, window, cx),
        MenuAction::PasteInto(dir) => ws.paste_clipboard_into(dir, window, cx),
        MenuAction::RenameEntry(side, path, current) => {
            ws.open_rename_prompt(side, path, current, window, cx);
        }
        MenuAction::BulkRename(side) => ws.open_bulk_rename_for_pane(side, window, cx),
        MenuAction::CompressToZip(side) => ws.compress_selected_to_zip(side, window, cx),
        MenuAction::ExtractArchiveHere(archive_path) => {
            ws.extract_archive_here(archive_path, window, cx);
        }
        MenuAction::ExtractArchiveSelection(side, archive_path, inner_path) => {
            let mut inner_paths = ws.selected_archive_inner_paths(side, &archive_path);
            if !inner_paths.iter().any(|p| p == &inner_path) {
                inner_paths = vec![inner_path];
            }
            ws.extract_archive_item_to_target(archive_path, inner_paths, window, cx);
        }
        MenuAction::ComputeChecksums => ws.compute_checksums_for_selection(cx),
        MenuAction::CheckFileLocks => ws.check_locks_for_selection(cx),
        MenuAction::NewFolder(side, dir) => ws.open_new_folder_prompt(side, dir, window, cx),
        MenuAction::NewFile(side, dir) => ws.open_new_file_prompt(side, dir, window, cx),
        MenuAction::CopyPath(text) => ws.copy_text_to_clipboard(&text, "Path copied", cx),
        MenuAction::OpenTerminal(path) => ws.open_terminal_at(&path, cx),
        MenuAction::OpenEditor(path) => ws.open_editor_at(&path, cx),
        MenuAction::PinFolder(path) => ws.pin_folder(path, cx),
        MenuAction::ShowWindowsMenu(paths) => ws.show_native_windows_menu(paths, cx),
        MenuAction::RevealInWindowsExplorer(path) => ws.reveal_in_windows_explorer(&path, cx),
        MenuAction::ShowProperties(path) => ws.show_properties_for(&path, cx),
        MenuAction::TrashSelected(side) => ws.trash_selected(side, window, cx),
        MenuAction::PermanentDeleteSelected(side) => ws.prompt_permanent_delete(side, cx),
        MenuAction::RefreshPane(side) => ws.refresh_pane(side, window, cx),
        MenuAction::HideQuickAccess(kind) => ws.hide_quick_access(kind, cx),
        MenuAction::ToggleSidebarSectionCollapsed(sec) => {
            ws.toggle_sidebar_section_collapsed(sec, cx);
        }
        MenuAction::HideSidebarSection(sec) => ws.toggle_sidebar_section_visible(sec, cx),
        MenuAction::OpenSidebarSettings => {
            ws.open_settings_category(SettingsCategory::Interface, window, cx);
        }
    }
}
