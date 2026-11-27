use std::path::PathBuf;

use gpui_kit::{
    Context, Entity, SharedString,
    assets::IconName as LucideIcon,
    component::{
        Disableable as _, Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, prelude::*, px, relative,
};
use texplorer_core::{NetworkProtocol, SshAuthMode};
use texplorer_net::{IncomingP2pOffer, P2pDirection, P2pTicket, decode_ticket};
use uuid::Uuid;

use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, format_size, full_hit_input, soft_tint, solid,
};
use crate::ui::workspace::Workspace;

/// Modal form state for creating or editing a Network connection (SMB / SFTP / FTP / FTPS).
#[derive(Clone)]
pub struct NetworkConnectModalState {
    pub editing_id: Option<Uuid>,
    pub protocol: NetworkProtocol,
    pub auth_mode: SshAuthMode,
    pub sftp_sudo: bool,
    pub name_input: Entity<InputState>,
    pub host_input: Entity<InputState>,
    pub port_input: Entity<InputState>,
    pub remote_path_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub password_input: Entity<InputState>,
    pub sudo_password_input: Entity<InputState>,
    pub key_path_input: Entity<InputState>,
    pub busy: bool,
    pub status_message: Option<String>,
    pub status_is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pModalTab {
    Send,
    Receive,
}

/// Modal state for Internet + LAN encrypted P2P file & folder transfer.
#[derive(Clone)]
pub struct P2pModalState {
    pub tab: P2pModalTab,
    pub selected_paths: Vec<PathBuf>,
    pub ticket_input: Entity<InputState>,
    pub ticket_text: String,
    pub dest_dir: PathBuf,
    pub generating_share: bool,
    pub error: Option<String>,
}

/// Live UI progress record for an in-flight P2P transfer.
#[derive(Debug, Clone)]
pub struct ActiveP2pTransferUi {
    pub transfer_id: Uuid,
    pub direction: P2pDirection,
    pub peer_name: String,
    pub summary: String,
    pub current_file: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
}

/// Renders the Connect to Server / Add Network Location modal (`SMB`, `SSH/SFTP`, `FTP`, `FTPS`).
pub fn render_network_connect_modal(
    _ws: &Workspace,
    form: &NetworkConnectModalState,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let title = if form.editing_id.is_some() {
        "Edit Network Connection"
    } else {
        "Connect to Server / Shared Folder"
    };
    let is_smb = form.protocol == NetworkProtocol::Smb;
    let is_sftp = form.protocol == NetworkProtocol::Sftp;
    let is_key_auth = is_sftp && form.auth_mode == SshAuthMode::PrivateKey;

    v_flex()
        .id("network-connect-modal-card")
        .w(px(548.))
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
                .items_center()
                .justify_between()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(LucideIcon::Network)
                                .small()
                                .text_color(solid(p.accent_primary)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
                                .child(title),
                        ),
                )
                .child(
                    Button::new("net-modal-close")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(LucideIcon::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modal(cx);
                        })),
                ),
        )
        // Form body
        .child(
            v_flex()
                .p_4()
                .gap_3()
                // Protocol segmented pills
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(field_label("Protocol", p))
                        .child(
                            h_flex()
                                .w_full()
                                .gap_1p5()
                                .children(NetworkProtocol::ALL.into_iter().map(|proto| {
                                    let selected = form.protocol == proto;
                                    let btn_id = match proto {
                                        NetworkProtocol::Smb => "net-proto-smb",
                                        NetworkProtocol::Sftp => "net-proto-sftp",
                                        NetworkProtocol::Ftp => "net-proto-ftp",
                                        NetworkProtocol::Ftps => "net-proto-ftps",
                                    };
                                    div()
                                        .id(btn_id)
                                        .flex_1()
                                        .h(px(30.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(RADIUS_CONTROL))
                                        .cursor_pointer()
                                        .bg(if selected {
                                            soft_tint(p.accent_primary, 0.16)
                                        } else {
                                            solid(p.bg_base)
                                        })
                                        .border_1()
                                        .border_color(if selected {
                                            solid(p.accent_primary)
                                        } else {
                                            solid(p.border_subtle)
                                        })
                                        .text_xs()
                                        .font_medium()
                                        .text_color(if selected {
                                            solid(p.accent_primary)
                                        } else {
                                            solid(p.text_secondary)
                                        })
                                        .hover(move |s| s.bg(solid(p.bg_hover)))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.set_network_modal_protocol(proto, window, cx);
                                        }))
                                        .child(proto.short_badge())
                                })),
                        ),
                )
                // Display Name
                .child(labeled_input_row(
                    "Display Name",
                    "net-in-name",
                    &form.name_input,
                    false,
                    p,
                ))
                // Host / Share Path (+ Port for SFTP/FTP/FTPS)
                .child(if is_smb {
                    labeled_input_row(
                        "Shared Folder Path (UNC)",
                        "net-in-host",
                        &form.host_input,
                        false,
                        p,
                    )
                    .into_any_element()
                } else {
                    h_flex()
                        .w_full()
                        .gap_2p5()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(labeled_input_row(
                                    "Host / IP Address",
                                    "net-in-host",
                                    &form.host_input,
                                    false,
                                    p,
                                )),
                        )
                        .child(
                            div()
                                .w(px(96.))
                                .flex_shrink_0()
                                .child(labeled_input_row(
                                    "Port",
                                    "net-in-port",
                                    &form.port_input,
                                    false,
                                    p,
                                )),
                        )
                        .into_any_element()
                })
                // Remote Starting Directory (non-SMB)
                .when(!is_smb, |el| {
                    el.child(labeled_input_row(
                        "Remote Directory",
                        "net-in-rpath",
                        &form.remote_path_input,
                        false,
                        p,
                    ))
                })
                // Auth Mode selector for SSH / SFTP
                .when(is_sftp, |el| {
                    el.child(
                        v_flex()
                            .gap_1p5()
                            .child(field_label("SSH Authentication", p))
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .children([SshAuthMode::Password, SshAuthMode::PrivateKey].into_iter().map(
                                        |mode| {
                                            let active = form.auth_mode == mode;
                                            let id = match mode {
                                                SshAuthMode::Password => "net-auth-pass",
                                                SshAuthMode::PrivateKey => "net-auth-key",
                                            };
                                            div()
                                                .id(id)
                                                .flex_1()
                                                .h(px(28.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .gap_1p5()
                                                .rounded(px(RADIUS_CONTROL))
                                                .cursor_pointer()
                                                .bg(if active {
                                                    soft_tint(p.accent_primary, 0.14)
                                                } else {
                                                    solid(p.bg_base)
                                                })
                                                .border_1()
                                                .border_color(if active {
                                                    solid(p.accent_primary)
                                                } else {
                                                    solid(p.border_subtle)
                                                })
                                                .text_xs()
                                                .text_color(if active {
                                                    solid(p.accent_primary)
                                                } else {
                                                    solid(p.text_secondary)
                                                })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.set_network_modal_auth_mode(mode, window, cx);
                                                }))
                                                .child(
                                                    Icon::new(match mode {
                                                        SshAuthMode::Password => LucideIcon::Lock,
                                                        SshAuthMode::PrivateKey => LucideIcon::Key,
                                                    })
                                                    .xsmall(),
                                                )
                                                .child(mode.label())
                                        },
                                    )),
                            ),
                    )
                })
                // Username & Password / Passphrase
                .child(
                    h_flex()
                        .w_full()
                        .gap_2p5()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(labeled_input_row(
                                    if is_smb {
                                        "Username (optional)"
                                    } else {
                                        "Username"
                                    },
                                    "net-in-user",
                                    &form.username_input,
                                    false,
                                    p,
                                )),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(labeled_input_row(
                                    if is_key_auth {
                                        "Key Passphrase (optional)"
                                    } else if is_smb {
                                        "Password (optional)"
                                    } else {
                                        "Password"
                                    },
                                    "net-in-pass",
                                    &form.password_input,
                                    true,
                                    p,
                                )),
                        ),
                )
                // Private Key Path (SSH key auth only — auto-detects system SSH key + native picker)
                .when(is_key_auth, |el| {
                    el.child(
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(field_label("Private Key File (auto-detected from system SSH)", p))
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .h(px(32.))
                                            .px_2p5()
                                            .rounded(px(RADIUS_CONTROL))
                                            .bg(solid(p.bg_base))
                                            .border_1()
                                            .border_color(solid(p.border))
                                            .child(full_hit_input(
                                                "net-in-keypath",
                                                &form.key_path_input,
                                                Input::new(&form.key_path_input)
                                                    .with_size(Size::Small)
                                                    .appearance(false),
                                            )),
                                    )
                                    .child(
                                        Button::new("net-pick-key")
                                            .with_size(Size::Small)
                                            .icon(LucideIcon::FolderOpen)
                                            .label("Select Key")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.network_pick_ssh_key_file(cx);
                                            })),
                                    ),
                            ),
                    )
                })
                // SFTP root elevation via sudo (`sudo sftp-server`)
                .when(is_sftp, |el| {
                    let sudo_on = form.sftp_sudo;
                    el.child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                h_flex()
                                    .w_full()
                                    .px_2p5()
                                    .py_2()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(solid(p.bg_base))
                                    .border_1()
                                    .border_color(if sudo_on {
                                        solid(p.accent_primary)
                                    } else {
                                        solid(p.border_subtle)
                                    })
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap(px(1.))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(solid(p.text_primary))
                                                    .child("Elevate with sudo (root SFTP)"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.5))
                                                    .text_color(solid(p.text_muted))
                                                    .child("Access /root, /etc, and protected folders via sudo sftp-server."),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("net-sftp-sudo-toggle")
                                            .flex_shrink_0()
                                            .px_2p5()
                                            .py_1()
                                            .rounded(px(RADIUS_CONTROL))
                                            .cursor_pointer()
                                            .bg(if sudo_on {
                                                soft_tint(p.accent_primary, 0.16)
                                            } else {
                                                solid(p.bg_elevated)
                                            })
                                            .border_1()
                                            .border_color(if sudo_on {
                                                solid(p.accent_primary)
                                            } else {
                                                solid(p.border)
                                            })
                                            .text_xs()
                                            .font_medium()
                                            .text_color(if sudo_on {
                                                solid(p.accent_primary)
                                            } else {
                                                solid(p.text_secondary)
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.toggle_network_modal_sftp_sudo(cx);
                                            }))
                                            .child(if sudo_on { "Enabled" } else { "Off" }),
                                    ),
                            )
                            .when(sudo_on && is_key_auth, |inner| {
                                inner.child(labeled_input_row(
                                    "Sudo Password (optional — leave empty if NOPASSWD)",
                                    "net-in-sudopass",
                                    &form.sudo_password_input,
                                    true,
                                    p,
                                ))
                            }),
                    )
                })
                // Status / error feedback banner
                .when_some(form.status_message.clone(), |el, msg| {
                    let color = if form.status_is_error {
                        p.status_error
                    } else {
                        p.status_success
                    };
                    el.child(
                        div()
                            .w_full()
                            .px_2p5()
                            .py_1p5()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(soft_tint(color, 0.12))
                            .border_1()
                            .border_color(soft_tint(color, 0.35))
                            .text_xs()
                            .text_color(solid(color))
                            .child(msg),
                    )
                })
                // Action buttons
                .child(
                    h_flex()
                        .w_full()
                        .pt_1()
                        .items_center()
                        .justify_between()
                        .child(
                            Button::new("net-btn-test")
                                .with_size(Size::Small)
                                .disabled(form.busy)
                                .label(if form.busy {
                                    "Connecting..."
                                } else {
                                    "Test Connection"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.test_network_modal_connection(cx);
                                })),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("net-btn-cancel")
                                        .with_size(Size::Small)
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_modal(cx);
                                        })),
                                )
                                .child(
                                    Button::new("net-btn-save")
                                        .with_size(Size::Small)
                                        .primary()
                                        .disabled(form.busy)
                                        .label("Save & Connect")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_and_connect_network_modal(window, cx);
                                        })),
                                ),
                        ),
                ),
        )
}

/// Renders the encrypted Internet + LAN P2P File Transfer modal (Send / Receive).
pub fn render_p2p_modal(
    ws: &Workspace,
    modal: &P2pModalState,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let unit = ws.settings.size_unit;
    let parsed_ticket: Option<Result<P2pTicket, String>> = if modal.ticket_text.trim().is_empty() {
        None
    } else {
        Some(decode_ticket(&modal.ticket_text))
    };

    v_flex()
        .id("p2p-modal-card")
        .w(px(560.))
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
                .items_center()
                .justify_between()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(LucideIcon::Radio)
                                .small()
                                .text_color(solid(p.accent_primary)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
                                .child("P2P File Transfer"),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .py(px(1.))
                                .rounded(px(3.))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .text_size(px(11.))
                                .text_color(solid(p.text_muted))
                                .child("End-to-End Encrypted • LAN & Internet"),
                        ),
                )
                .child(
                    Button::new("p2p-modal-close")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(LucideIcon::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_modal(cx);
                        })),
                ),
        )
        // Mode tabs (Send / Receive)
        .child(
            h_flex()
                .w_full()
                .px_4()
                .pt_3()
                .gap_2()
                .children([
                    (P2pModalTab::Send, "p2p-tab-send", LucideIcon::Upload, "Send Files"),
                    (
                        P2pModalTab::Receive,
                        "p2p-tab-recv",
                        LucideIcon::Download,
                        "Receive by Code",
                    ),
                ]
                .into_iter()
                .map(|(tab, id, icon, label)| {
                    let active = modal.tab == tab;
                    div()
                        .id(id)
                        .flex_1()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .rounded(px(RADIUS_CONTROL))
                        .cursor_pointer()
                        .bg(if active {
                            soft_tint(p.accent_primary, 0.16)
                        } else {
                            solid(p.bg_base)
                        })
                        .border_1()
                        .border_color(if active {
                            solid(p.accent_primary)
                        } else {
                            solid(p.border_subtle)
                        })
                        .text_xs()
                        .font_semibold()
                        .text_color(if active {
                            solid(p.accent_primary)
                        } else {
                            solid(p.text_secondary)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_p2p_modal_tab(tab, window, cx);
                        }))
                        .child(Icon::new(icon).xsmall())
                        .child(label)
                })),
        )
        // Tab Body
        .child(match modal.tab {
            P2pModalTab::Send => v_flex()
                .p_4()
                .gap_3()
                // Selected items card
                .child(
                    v_flex()
                        .w_full()
                        .p_3()
                        .gap_2()
                        .rounded(px(RADIUS_ROW))
                        .bg(solid(p.bg_base))
                        .border_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(solid(p.text_primary))
                                        .child(if modal.selected_paths.is_empty() {
                                            SharedString::from("No files or folder selected")
                                        } else if modal.selected_paths.len() == 1 {
                                            SharedString::from(format!(
                                                "Ready to send: {}",
                                                modal.selected_paths[0]
                                                    .file_name()
                                                    .map(|n| n.to_string_lossy().into_owned())
                                                    .unwrap_or_else(|| {
                                                        modal.selected_paths[0]
                                                            .display()
                                                            .to_string()
                                                    })
                                            ))
                                        } else {
                                            SharedString::from(format!(
                                                "Ready to send {} selected items",
                                                modal.selected_paths.len()
                                            ))
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Button::new("p2p-pick-files")
                                                .with_size(Size::XSmall)
                                                .icon(LucideIcon::FilePlus)
                                                .label("Select Files")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.p2p_pick_send_files(cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("p2p-pick-folder")
                                                .with_size(Size::XSmall)
                                                .icon(LucideIcon::FolderOpen)
                                                .label("Select Folder")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.p2p_pick_send_folder(cx);
                                                })),
                                        ),
                                ),
                        )
                        .when(modal.selected_paths.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(solid(p.text_muted))
                                    .child("Choose files or a folder to send, or right-click any item in a pane → \"Send via P2P...\"."),
                            )
                        })
                        .when(!modal.selected_paths.is_empty(), |el| {
                            el.child(
                                v_flex()
                                    .max_h(px(72.))
                                    .overflow_y_scrollbar()
                                    .gap_0p5()
                                    .children(modal.selected_paths.iter().take(6).map(|path| {
                                        let name = path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| path.display().to_string());
                                        div()
                                            .truncate()
                                            .text_size(px(12.))
                                            .text_color(solid(p.text_secondary))
                                            .child(format!("• {name}"))
                                    })),
                            )
                        }),
                )
                // Internet & LAN Share Code section
                .child(
                    v_flex()
                        .w_full()
                        .p_3()
                        .gap_2()
                        .rounded(px(RADIUS_ROW))
                        .bg(solid(p.bg_base))
                        .border_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(solid(p.text_primary))
                                        .child("Share via Encrypted Code (Internet & LAN)"),
                                )
                                .when(ws.active_p2p_share.is_some(), |el| {
                                    el.child(
                                        Button::new("p2p-stop-share")
                                            .with_size(Size::XSmall)
                                            .ghost()
                                            .label("Stop Sharing")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.stop_active_p2p_share(cx);
                                            })),
                                    )
                                }),
                        )
                        .child(if let Some(share) = ws.active_p2p_share.as_ref() {
                            let code_copy = share.ticket_code.clone();
                            let short_preview: SharedString = if share.ticket_code.len() > 54 {
                                format!(
                                    "{}…{}",
                                    &share.ticket_code[..32],
                                    &share.ticket_code[share.ticket_code.len() - 16..]
                                )
                                .into()
                            } else {
                                share.ticket_code.clone().into()
                            };
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(solid(p.status_success))
                                        .child(format!(
                                            "Sharing {} ({}) — send this code to your friend:",
                                            share.summary,
                                            format_size(share.total_bytes, unit)
                                        )),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .px_2p5()
                                        .py_1p5()
                                        .gap_2()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(solid(p.bg_elevated))
                                        .border_1()
                                        .border_color(solid(p.border))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family("Consolas")
                                                .text_size(px(12.))
                                                .text_color(solid(p.text_primary))
                                                .child(short_preview),
                                        )
                                        .child(
                                            Button::new("p2p-copy-code")
                                                .with_size(Size::XSmall)
                                                .primary()
                                                .icon(LucideIcon::Copy)
                                                .label("Copy Code")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.copy_text_to_clipboard(
                                                        &code_copy,
                                                        "P2P share code copied!",
                                                        cx,
                                                    );
                                                })),
                                        ),
                                )
                                .into_any_element()
                        } else {
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.))
                                        .text_color(solid(p.text_muted))
                                        .child("Generates a private one-time code that traverses NAT/CGNAT securely."),
                                )
                                .child(
                                    Button::new("p2p-gen-code")
                                        .with_size(Size::Small)
                                        .primary()
                                        .disabled(
                                            modal.selected_paths.is_empty()
                                                || modal.generating_share,
                                        )
                                        .label(if modal.generating_share {
                                            "Preparing..."
                                        } else {
                                            "Generate Code"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.generate_p2p_share_code(cx);
                                        })),
                                )
                                .into_any_element()
                        }),
                )
                // Nearby LAN peers section
                .child(
                    v_flex()
                        .w_full()
                        .p_3()
                        .gap_2()
                        .rounded(px(RADIUS_ROW))
                        .bg(solid(p.bg_base))
                        .border_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(solid(p.text_primary))
                                        .child("Nearby Peers (Local Network)"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(solid(p.text_muted))
                                        .child(format!("This PC: {}", ws.p2p_device_name)),
                                ),
                        )
                        .child(if ws.p2p_nearby_peers.is_empty() {
                            div()
                                .text_size(px(12.))
                                .text_color(solid(p.text_muted))
                                .child("Scanning LAN for other PCs running TExplorer...")
                                .into_any_element()
                        } else {
                            v_flex()
                                .gap_1()
                                .children(ws.p2p_nearby_peers.iter().enumerate().map(|(i, peer)| {
                                    let peer_send = peer.clone();
                                    let disabled = modal.selected_paths.is_empty();
                                    h_flex()
                                        .w_full()
                                        .px_2p5()
                                        .py_1p5()
                                        .items_center()
                                        .justify_between()
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(solid(p.bg_elevated))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .w(px(6.))
                                                        .h(px(6.))
                                                        .rounded(px(999.))
                                                        .bg(solid(p.status_success)),
                                                )
                                                .child(
                                                    Icon::new(LucideIcon::Laptop)
                                                        .xsmall()
                                                        .text_color(solid(p.text_secondary)),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_medium()
                                                        .text_color(solid(p.text_primary))
                                                        .child(peer.device_name.clone()),
                                                ),
                                        )
                                        .child(
                                            Button::new(("p2p-send-peer", i))
                                                .with_size(Size::XSmall)
                                                .primary()
                                                .disabled(disabled)
                                                .label("Send")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.send_p2p_to_peer(peer_send.clone(), cx);
                                                })),
                                        )
                                }))
                                .into_any_element()
                        }),
                )
                .when_some(modal.error.clone(), |el, err| {
                    el.child(
                        div()
                            .px_2p5()
                            .py_1p5()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(soft_tint(p.status_error, 0.12))
                            .text_xs()
                            .text_color(solid(p.status_error))
                            .child(err),
                    )
                })
                .into_any_element(),
            P2pModalTab::Receive => v_flex()
                .p_4()
                .gap_3()
                .child(labeled_input_row(
                    "Paste P2P Share Code (texp1-...)",
                    "p2p-recv-code",
                    &modal.ticket_input,
                    false,
                    p,
                ))
                // Live decoded ticket confirmation preview
                .when_some(parsed_ticket, |el, res| match res {
                    Ok(ticket) => el.child(
                        v_flex()
                            .w_full()
                            .p_3()
                            .gap_1p5()
                            .rounded(px(RADIUS_ROW))
                            .bg(soft_tint(p.accent_primary, 0.08))
                            .border_1()
                            .border_color(soft_tint(p.accent_primary, 0.35))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(solid(p.text_primary))
                                    .child(format!("From: {}", ticket.sender_name)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(solid(p.text_secondary))
                                    .child(format!(
                                        "{}  •  {} file(s)  •  {}",
                                        ticket.summary,
                                        ticket.item_count,
                                        format_size(ticket.total_bytes, unit)
                                    )),
                            ),
                    ),
                    Err(err) => el.child(
                        div()
                            .px_2p5()
                            .py_1p5()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(soft_tint(p.status_error, 0.12))
                            .text_xs()
                            .text_color(solid(p.status_error))
                            .child(err),
                    ),
                })
                .child(
                    h_flex()
                        .w_full()
                        .px_2p5()
                        .py_1p5()
                        .gap_2()
                        .items_center()
                        .justify_between()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(solid(p.bg_base))
                        .border_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(solid(p.text_muted))
                                        .child("Save destination"),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(solid(p.text_primary))
                                        .child(modal.dest_dir.display().to_string()),
                                ),
                        )
                        .child(
                            Button::new("p2p-pick-dest")
                                .with_size(Size::XSmall)
                                .icon(LucideIcon::FolderOpen)
                                .label("Choose Folder")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.p2p_pick_dest_folder(cx);
                                })),
                        ),
                )
                .when_some(modal.error.clone(), |el, err| {
                    el.child(
                        div()
                            .px_2p5()
                            .py_1p5()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(soft_tint(p.status_error, 0.12))
                            .text_xs()
                            .text_color(solid(p.status_error))
                            .child(err),
                    )
                })
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("p2p-recv-cancel")
                                .with_size(Size::Small)
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_modal(cx);
                                })),
                        )
                        .child(
                            Button::new("p2p-recv-start")
                                .with_size(Size::Small)
                                .primary()
                                .disabled(decode_ticket(&modal.ticket_text).is_err())
                                .icon(LucideIcon::Download)
                                .label("Accept & Download")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_p2p_receive_from_modal(cx);
                                })),
                        ),
                )
                .into_any_element(),
        })
        // Inline incoming LAN offers & live transfer progress inside the P2P window
        .when(
            !ws.incoming_p2p_offers.is_empty() || !ws.active_p2p_transfers.is_empty(),
            |el| {
                el.child(
                    v_flex()
                        .w_full()
                        .px_4()
                        .pb_4()
                        .gap_2()
                        .border_t_1()
                        .border_color(solid(p.border_subtle))
                        .pt_3()
                        .children(ws.incoming_p2p_offers.iter().enumerate().map(|(i, offer)| {
                            render_incoming_offer_card(i, offer, unit, p, cx)
                        }))
                        .children(ws.active_p2p_transfers.iter().enumerate().map(|(i, xfer)| {
                            render_p2p_transfer_card(i, xfer, unit, p)
                        })),
                )
            },
        )
}

/// Renders floating incoming P2P LAN confirmation cards when the P2P modal is closed.
/// Active transfer progress bars are shown directly inside the P2P window instead.
pub fn render_p2p_overlays(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let has_transfers = !ws.transfers.is_empty();

    v_flex()
        .absolute()
        .right(px(16.))
        .bottom(if has_transfers { px(190.) } else { px(72.) })
        .w(px(340.))
        .gap_2()
        .children(ws.incoming_p2p_offers.iter().enumerate().map(|(i, offer)| {
            render_incoming_offer_card(i, offer, unit, p, cx)
        }))
}

fn render_p2p_transfer_card(
    idx: usize,
    xfer: &ActiveP2pTransferUi,
    unit: texplorer_core::SizeUnit,
    p: Palette,
) -> impl IntoElement + use<> {
    let frac = if xfer.total_bytes > 0 {
        (xfer.bytes_transferred as f32 / xfer.total_bytes as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let pct = (frac * 100.0).round() as u8;
    let verb = match xfer.direction {
        P2pDirection::Send => "Sending to",
        P2pDirection::Receive => "Receiving from",
    };
    let file_label = if xfer.current_file.is_empty() {
        xfer.summary.clone()
    } else {
        xfer.current_file.clone()
    };
    let bytes_label = format!(
        "{} / {}",
        format_size(xfer.bytes_transferred, unit),
        format_size(xfer.total_bytes, unit)
    );

    v_flex()
        .id(("p2p-xfer-card", idx))
        .w_full()
        .p_3()
        .gap_1p5()
        .rounded(px(RADIUS_ROW))
        .bg(solid(p.bg_base))
        .border_1()
        .border_color(solid(p.border))
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .items_center()
                        .child(
                            Icon::new(match xfer.direction {
                                P2pDirection::Send => LucideIcon::Upload,
                                P2pDirection::Receive => LucideIcon::Download,
                            })
                            .xsmall()
                            .text_color(solid(p.accent_primary)),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
                                .child(format!("{verb} {}", xfer.peer_name)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(solid(p.text_secondary))
                        .child(format!("{pct}%")),
                ),
        )
        .child(
            div()
                .truncate()
                .text_size(px(12.))
                .text_color(solid(p.text_muted))
                .child(file_label),
        )
        .child(
            div()
                .w_full()
                .h(px(4.))
                .rounded(px(999.))
                .bg(solid(p.border_subtle))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(frac))
                        .bg(solid(p.accent_primary)),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(solid(p.text_muted))
                .child(bytes_label),
        )
}

fn render_incoming_offer_card(
    idx: usize,
    offer: &IncomingP2pOffer,
    unit: texplorer_core::SizeUnit,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement + use<> {
    let accept_id = offer.transfer_id;
    let decline_id = offer.transfer_id;

    v_flex()
        .id(("p2p-offer-card", idx))
        .w_full()
        .p_3()
        .gap_2()
        .rounded(px(RADIUS_ROW))
        .bg(solid(p.bg_elevated))
        .border_1()
        .border_color(solid(p.accent_primary))
        .occlude()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(LucideIcon::Radio)
                        .xsmall()
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
                        .child(format!("Incoming P2P from {}", offer.sender_name)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(solid(p.text_secondary))
                .child(format!(
                    "{}  •  {}",
                    offer.summary,
                    format_size(offer.total_bytes, unit)
                )),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_1p5()
                .child(
                    Button::new(("p2p-decline", idx))
                        .with_size(Size::XSmall)
                        .label("Decline")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.respond_incoming_p2p(decline_id, false, window, cx);
                        })),
                )
                .child(
                    Button::new(("p2p-accept", idx))
                        .with_size(Size::XSmall)
                        .primary()
                        .icon(LucideIcon::Download)
                        .label("Accept")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.respond_incoming_p2p(accept_id, true, window, cx);
                        })),
                ),
        )
}

fn field_label(label: &'static str, p: Palette) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .font_medium()
        .text_color(solid(p.text_secondary))
        .child(label)
}

fn labeled_input_row(
    label: &'static str,
    hit_id: &'static str,
    state: &Entity<InputState>,
    is_password: bool,
    p: Palette,
) -> impl IntoElement {
    let mut input = Input::new(state).with_size(Size::Small).appearance(false);
    if is_password {
        input = input.mask_toggle();
    }
    v_flex()
        .w_full()
        .gap_1()
        .child(field_label(label, p))
        .child(
            div()
                .w_full()
                .h(px(32.))
                .px_2p5()
                .rounded(px(RADIUS_CONTROL))
                .bg(solid(p.bg_base))
                .border_1()
                .border_color(solid(p.border))
                .child(full_hit_input(hit_id, state, input)),
        )
}
