use std::path::PathBuf;

use gpui_kit::{
    Context, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Window,
    assets::IconName as LucideIcon,
    component::{Icon, IconName, Sizable as _, h_flex, v_flex},
    div, prelude::*, px,
};
use texplorer_core::{FileEntry, KnownFolder, KnownFolderKind, Location, PaneSide, VolumeInfo};

use crate::theme::{
    BG_ELEVATED, BG_HOVER, BORDER, BORDER_SUBTLE, RADIUS_CONTROL, RADIUS_ROW, STATUS_ERROR,
    TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY, soft_tint, solid,
};
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
    OpenNewTab(Location),
    OpenOppositePane(Location),
    RenameEntry(PaneSide, PathBuf, String),
    NewFolder(PaneSide, PathBuf),
    NewFile(PaneSide, PathBuf),
    CopyPath(String),
    OpenTerminal(PathBuf),
    OpenEditor(PathBuf),
    OpenTexTree(PathBuf),
    PinFolder(PathBuf),
    RevealInWindowsExplorer(PathBuf),
    ShowProperties(PathBuf),
    TrashSelected(PaneSide),
    PermanentDeleteSelected(PaneSide),
    RefreshPane(PaneSide),
    HideQuickAccess(KnownFolderKind),
}

/// Renders the flat Obsidian Dark floating context menu clamped cleanly inside the viewport.
pub fn render_context_menu(
    ws: &Workspace,
    menu: &ContextMenuState,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let rows = build_rows(ws, &menu.target);
    if rows.is_empty() {
        return div().into_any_element();
    }

    const MENU_W: f32 = 248.0;
    const ITEM_H: f32 = 28.0;
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
        .child(
            div()
                .id("ctx-menu-backdrop")
                .absolute()
                .inset_0()
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
                .bg(solid(BG_ELEVATED))
                .border_1()
                .border_color(solid(BORDER))
                .rounded(px(RADIUS_ROW))
                .children(rows.into_iter().map(|row| match row {
                    MenuRow::Separator => div()
                        .w_full()
                        .h(px(1.))
                        .my_1()
                        .bg(solid(BORDER_SUBTLE))
                        .into_any_element(),
                    MenuRow::Item {
                        id,
                        label,
                        shortcut,
                        icon,
                        danger,
                        action,
                    } => {
                        let text_hex = if danger { STATUS_ERROR } else { TEXT_PRIMARY };
                        let icon_hex = if danger { STATUS_ERROR } else { TEXT_SECONDARY };
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
                                    s.bg(soft_tint(STATUS_ERROR, 0.14))
                                } else {
                                    s.bg(solid(BG_HOVER))
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
                                        .text_size(px(10.))
                                        .text_color(solid(TEXT_MUTED))
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
    let has_textree = texplorer_platform::find_textree_exe(ws.settings.textree_path.as_deref()).is_some();

    match target {
        ContextMenuTarget::Entry { side, entry } => {
            let mut rows = Vec::new();
            let is_dir = entry.is_dir();
            rows.push(MenuRow::Item {
                id: "ctx-open",
                label: "Open".into(),
                shortcut: Some("Enter"),
                icon: MenuIcon::Lucide(if is_dir {
                    LucideIcon::FolderOpen
                } else {
                    LucideIcon::ExternalLink
                }),
                danger: false,
                action: MenuAction::OpenEntry(entry.location.clone(), is_dir),
            });

            if is_dir {
                rows.push(MenuRow::Item {
                    id: "ctx-open-tab",
                    label: "Open in New Tab".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Plus),
                    danger: false,
                    action: MenuAction::OpenNewTab(entry.location.clone()),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-open-split",
                    label: "Open in Split Pane".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Columns2),
                    danger: false,
                    action: MenuAction::OpenOppositePane(entry.location.clone()),
                });
            }

            if let Some(path) = entry.location.as_local_path() {
                let path_buf = path.to_path_buf();
                rows.push(MenuRow::Separator);
                rows.push(MenuRow::Item {
                    id: "ctx-rename",
                    label: "Rename".into(),
                    shortcut: Some("F2"),
                    icon: MenuIcon::Lucide(LucideIcon::Pencil),
                    danger: false,
                    action: MenuAction::RenameEntry(*side, path_buf.clone(), entry.name.clone()),
                });
                rows.push(MenuRow::Item {
                    id: "ctx-copy-path",
                    label: "Copy Path".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Copy),
                    danger: false,
                    action: MenuAction::CopyPath(entry.location.display_path()),
                });

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
                if is_dir && has_textree {
                    rows.push(MenuRow::Item {
                        id: "ctx-textree",
                        label: "Analyze in TexTree".into(),
                        shortcut: None,
                        icon: MenuIcon::Lucide(LucideIcon::HardDrive),
                        danger: false,
                        action: MenuAction::OpenTexTree(path_buf.clone()),
                    });
                }
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
                    id: "ctx-win-explorer",
                    label: "Reveal in Windows Explorer".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::ExternalLink),
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
            }
            rows
        }
        ContextMenuTarget::PaneBackground { side, directory } => vec![
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
            vec![
                MenuRow::Item {
                    id: "ctx-qa-open",
                    label: "Open".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::FolderOpen),
                    danger: false,
                    action: MenuAction::OpenEntry(loc.clone(), true),
                },
                MenuRow::Item {
                    id: "ctx-qa-tab",
                    label: "Open in New Tab".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Plus),
                    danger: false,
                    action: MenuAction::OpenNewTab(loc.clone()),
                },
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
            ]
        }
        ContextMenuTarget::Volume(vol) => {
            let loc = Location::local(&vol.root_path);
            let mut rows = vec![
                MenuRow::Item {
                    id: "ctx-vol-open",
                    label: "Open Drive".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::HardDrive),
                    danger: false,
                    action: MenuAction::OpenEntry(loc.clone(), true),
                },
                MenuRow::Item {
                    id: "ctx-vol-tab",
                    label: "Open in New Tab".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::Plus),
                    danger: false,
                    action: MenuAction::OpenNewTab(loc.clone()),
                },
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
            ];
            if has_textree {
                rows.push(MenuRow::Item {
                    id: "ctx-vol-textree",
                    label: "Analyze in TexTree".into(),
                    shortcut: None,
                    icon: MenuIcon::Lucide(LucideIcon::HardDrive),
                    danger: false,
                    action: MenuAction::OpenTexTree(vol.root_path.clone()),
                });
            }
            rows.push(MenuRow::Item {
                id: "ctx-vol-props",
                label: "Drive Properties".into(),
                shortcut: None,
                icon: MenuIcon::Lucide(LucideIcon::Settings),
                danger: false,
                action: MenuAction::ShowProperties(vol.root_path.clone()),
            });
            rows
        }
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
        MenuAction::OpenNewTab(loc) => ws.open_new_tab(loc, window, cx),
        MenuAction::OpenOppositePane(loc) => ws.open_in_opposite_pane(loc, window, cx),
        MenuAction::RenameEntry(side, path, current) => {
            ws.open_rename_prompt(side, path, current, window, cx);
        }
        MenuAction::NewFolder(side, dir) => ws.open_new_folder_prompt(side, dir, window, cx),
        MenuAction::NewFile(side, dir) => ws.open_new_file_prompt(side, dir, window, cx),
        MenuAction::CopyPath(text) => ws.copy_text_to_clipboard(&text, "Path copied", cx),
        MenuAction::OpenTerminal(path) => ws.open_terminal_at(&path, cx),
        MenuAction::OpenEditor(path) => ws.open_editor_at(&path, cx),
        MenuAction::OpenTexTree(path) => ws.open_in_textree(&path, cx),
        MenuAction::PinFolder(path) => ws.pin_folder(path, cx),
        MenuAction::RevealInWindowsExplorer(path) => ws.reveal_in_windows_explorer(&path, cx),
        MenuAction::ShowProperties(path) => ws.show_properties_for(&path, cx),
        MenuAction::TrashSelected(side) => ws.trash_selected(side, window, cx),
        MenuAction::PermanentDeleteSelected(side) => ws.prompt_permanent_delete(side, cx),
        MenuAction::RefreshPane(side) => ws.refresh_pane(side, window, cx),
        MenuAction::HideQuickAccess(kind) => ws.hide_quick_access(kind, cx),
    }
}
