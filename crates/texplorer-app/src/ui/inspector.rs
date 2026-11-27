use gpui_kit::{
    Context, ObjectFit, SharedString, StyledText,
    assets::IconName as LucideIcon,
    component::{
        Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::Input,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, img, prelude::*, px,
};
use texplorer_core::{EntryFlags, Location};
use texplorer_platform::FileLockStatus;
use texplorer_vfs::hash::{FileHashes, verify_hash_match};

use crate::preview::PreviewPayload;
use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, file_kind_visual, format_size, format_timestamp,
    full_hit_input, soft_tint, solid,
};
use crate::ui::workspace::Workspace;

/// Async checksum state shown inside the right-hand Inspector.
#[derive(Debug, Clone, Default)]
pub enum HashInspectState {
    #[default]
    Idle,
    Computing,
    Ready(FileHashes),
    Error(String),
}

/// Renders the collapsible right-hand Preview & Inspector panel (`Alt+P`) with live inline preview,
/// streaming SHA-256/MD5 checksums + verification, and Windows Restart Manager file-lock detection.
pub fn render_inspector(ws: &mut Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let active_pane = ws.active_tab().map(|t| t.model.active_pane_ref());
    let primary = active_pane.and_then(|pane| pane.primary_selected_entry()).cloned();
    let sel_count = active_pane.map(|pane| pane.selection.len()).unwrap_or(0);

    v_flex()
        .id("inspector-panel")
        .size_full()
        .bg(solid(p.bg_surface))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .h(px(40.))
                .px_3()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(solid(p.text_secondary))
                        .child("INSPECTOR"),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .when(primary.is_some(), |el| {
                            el.child(
                                Button::new("insp-quicklook-btn")
                                    .with_size(Size::XSmall)
                                    .ghost()
                                    .icon(LucideIcon::Eye)
                                    .tooltip("QuickLook (Space)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_quicklook(cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new("insp-close")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::X)
                                .tooltip("Close Inspector (Alt+P)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_inspector(cx);
                                })),
                        ),
                ),
        )
        .child(match primary {
            None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .px_4()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.text_primary))
                        .child("No selection"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child("Select a file or folder to inspect or press Space for QuickLook."),
                )
                .into_any_element(),
            Some(entry) => {
                let (icon, icon_hex) = file_kind_visual(entry.kind, p);
                let sys_icon = ws.get_or_request_entry_icon(&entry, 48);
                let name: SharedString = entry.name.clone().into();
                let type_label: SharedString = entry.type_label.clone().into();
                let path_str: SharedString = entry.location.display_path().into();
                let size_str: SharedString = entry
                    .size
                    .map(|b| format_size(b, unit))
                    .unwrap_or_else(|| "Folder".to_string())
                    .into();
                let modified_str: SharedString = format_timestamp(entry.modified_secs).into();
                let created_str: SharedString = format_timestamp(entry.created_secs).into();
                let local_path = entry.location.as_local_path().map(|path| path.to_path_buf());
                let is_dir = entry.is_dir();
                let is_local_file = local_path.is_some() && !is_dir;
                let archive_entry_info = match &entry.location {
                    Location::Archive {
                        archive_path,
                        inner_path,
                    } => Some((archive_path.clone(), inner_path.clone())),
                    _ => None,
                };

                let matches_preview = ws
                    .preview_location
                    .as_ref()
                    .is_some_and(|pl| pl == &entry.location);
                let preview_visual = if matches_preview {
                    render_inline_preview(
                        &ws.preview_payload,
                        ws.preview_generation,
                        sys_icon,
                        icon,
                        icon_hex,
                        unit,
                        p,
                    )
                    .into_any_element()
                } else {
                    render_icon_box(sys_icon, icon, icon_hex).into_any_element()
                };

                let mut attr_badges: Vec<&'static str> = Vec::new();
                if entry.flags.contains(EntryFlags::IS_ARCHIVE) {
                    attr_badges.push("Inside Archive");
                }
                if entry.flags.contains(EntryFlags::IS_READONLY) {
                    attr_badges.push("Read-Only");
                }
                if entry.flags.contains(EntryFlags::IS_HIDDEN) {
                    attr_badges.push("Hidden");
                }
                if entry.flags.contains(EntryFlags::IS_SYSTEM) {
                    attr_badges.push("System");
                }
                if entry.flags.contains(EntryFlags::IS_SYMLINK) {
                    attr_badges.push("Symlink");
                }
                if entry.flags.contains(EntryFlags::IS_COMPRESSED) {
                    attr_badges.push("NTFS Compressed");
                }
                if entry.flags.contains(EntryFlags::IS_CLOUD_ONLY) {
                    attr_badges.push("Cloud Placeholder");
                }

                let open_loc = entry.location.clone();
                let copy_path = entry.location.display_path();
                let term_path = local_path.clone();
                let props_path = local_path.clone();
                let pin_path = local_path.clone();

                let hash_matches = local_path
                    .as_ref()
                    .is_some_and(|lp| ws.hash_target.as_ref() == Some(lp));
                let lock_matches = local_path
                    .as_ref()
                    .is_some_and(|lp| ws.lock_target.as_ref() == Some(lp));

                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .gap_3()
                    // Live inline preview card (click to launch QuickLook)
                    .child(
                        v_flex()
                            .id("insp-preview-card")
                            .w_full()
                            .p_3()
                            .gap_2()
                            .items_center()
                            .rounded(px(RADIUS_ROW))
                            .cursor_pointer()
                            .bg(solid(p.bg_base))
                            .border_1()
                            .border_color(solid(p.border_subtle))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_quicklook(cx);
                            }))
                            .child(preview_visual)
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(solid(p.text_primary))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(solid(p.text_muted))
                                    .child(if sel_count > 1 {
                                        SharedString::from(format!(
                                            "{} (+{} more selected)",
                                            type_label,
                                            sel_count - 1
                                        ))
                                    } else {
                                        type_label
                                    }),
                            ),
                    )
                    // Metadata rows
                    .child(
                        v_flex()
                            .w_full()
                            .rounded(px(RADIUS_ROW))
                            .bg(solid(p.bg_base))
                            .border_1()
                            .border_color(solid(p.border_subtle))
                            .overflow_hidden()
                            .child(meta_row("Size", size_str, p))
                            .child(meta_row("Modified", modified_str, p))
                            .child(meta_row("Created", created_str, p))
                            .child(
                                v_flex()
                                    .p_2p5()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(solid(p.text_muted))
                                            .child("Location"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(solid(p.text_secondary))
                                            .child(path_str),
                                    ),
                            ),
                    )
                    // Attribute badges
                    .when(!attr_badges.is_empty(), |el| {
                        el.child(
                            h_flex()
                                .flex_wrap()
                                .gap_1()
                                .children(attr_badges.into_iter().map(|badge| {
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(solid(p.bg_elevated))
                                        .border_1()
                                        .border_color(solid(p.border_subtle))
                                        .text_size(px(13.))
                                        .text_color(solid(p.text_secondary))
                                        .child(badge)
                                })),
                        )
                    })
                    // Phase 4: File Lock Detector card (local files only)
                    .when(is_local_file, |el| {
                        el.child(render_lock_section(ws, lock_matches, p, cx))
                    })
                    // Phase 4: SHA-256 & MD5 Checksums + Verifier card (local files only)
                    .when(is_local_file, |el| {
                        el.child(render_hash_section(ws, hash_matches, p, cx))
                    })
                    // Quick Actions
                    .child(
                        v_flex()
                            .gap_1p5()
                            .when_some(archive_entry_info, |el, (arch_path, inner)| {
                                el.child(
                                    Button::new("insp-extract-entry")
                                        .with_size(Size::Small)
                                        .primary()
                                        .label("Extract Item")
                                        .icon(LucideIcon::Download)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.extract_archive_item_to_target(
                                                arch_path.clone(),
                                                vec![inner.clone()],
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                            })
                            .when(local_path.is_some(), |el| {
                                el.child(
                                    Button::new("insp-open")
                                        .with_size(Size::Small)
                                        .primary()
                                        .label(if is_dir { "Open Folder" } else { "Open File" })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_location_or_file(
                                                open_loc.clone(),
                                                is_dir,
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                            })
                            .child(
                                Button::new("insp-copy-path")
                                    .with_size(Size::Small)
                                    .label("Copy Path")
                                    .icon(LucideIcon::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_path_to_clipboard(&copy_path, cx);
                                    })),
                            )
                            .when_some(term_path, |el, path| {
                                el.child(
                                    Button::new("insp-terminal")
                                        .with_size(Size::Small)
                                        .label("Open Terminal Here")
                                        .icon(LucideIcon::Terminal)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_terminal_at(&path, cx);
                                        })),
                                )
                            })
                            .when_some(pin_path.filter(|_| is_dir), |el, path| {
                                el.child(
                                    Button::new("insp-pin")
                                        .with_size(Size::Small)
                                        .label("Pin to Sidebar")
                                        .icon(LucideIcon::Pin)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.pin_folder(path.clone(), cx);
                                        })),
                                )
                            })
                            .when_some(props_path, |el, path| {
                                el.child(
                                    Button::new("insp-props")
                                        .with_size(Size::Small)
                                        .label("Properties")
                                        .icon(LucideIcon::ExternalLink)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.show_properties_for(&path, cx);
                                        })),
                                )
                            }),
                    )
                    .into_any_element()
            }
        })
}

fn render_lock_section(
    ws: &Workspace,
    matches_target: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let status = if matches_target {
        &ws.lock_status
    } else {
        &FileLockStatus::NotChecked
    };

    v_flex()
        .w_full()
        .p_2p5()
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
                        .text_size(px(13.))
                        .font_semibold()
                        .text_color(solid(p.text_secondary))
                        .child("FILE LOCKS"),
                )
                .child(
                    Button::new("insp-check-lock")
                        .with_size(Size::XSmall)
                        .ghost()
                        .label("Check")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.check_locks_for_selection(cx);
                        })),
                ),
        )
        .child(match status {
            FileLockStatus::NotChecked => div()
                .text_size(px(13.))
                .text_color(solid(p.text_muted))
                .child("Inspect which Windows processes hold open handles.")
                .into_any_element(),
            FileLockStatus::Checking => div()
                .text_size(px(13.))
                .text_color(solid(p.status_info))
                .child("Querying Windows Restart Manager...")
                .into_any_element(),
            FileLockStatus::Unlocked => h_flex()
                .gap_1p5()
                .items_center()
                .child(
                    div()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded(px(999.))
                        .bg(solid(p.status_success)),
                )
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(solid(p.status_success))
                        .child("Unlocked (no active locks)"),
                )
                .into_any_element(),
            FileLockStatus::LockedBy(procs) => v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(solid(p.status_error))
                        .child(format!("Locked by {} process(es):", procs.len())),
                )
                .children(procs.iter().take(6).map(|proc_info| {
                    h_flex()
                        .px_2()
                        .py_1()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(solid(p.bg_elevated))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(solid(p.text_primary))
                                .child(proc_info.name.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .text_color(solid(p.text_muted))
                                .child(format!("PID {} • {}", proc_info.pid, proc_info.app_type)),
                        )
                }))
                .into_any_element(),
            FileLockStatus::LockedUnknown => div()
                .text_xs()
                .text_color(solid(p.status_warning))
                .child("Locked by an elevated or system process.")
                .into_any_element(),
            FileLockStatus::AccessDenied => div()
                .text_xs()
                .text_color(solid(p.status_error))
                .child("Access denied while probing file lock.")
                .into_any_element(),
            FileLockStatus::NotApplicable => div()
                .text_size(px(13.))
                .text_color(solid(p.text_muted))
                .child("Not applicable to directories.")
                .into_any_element(),
            FileLockStatus::Error(err) => div()
                .text_size(px(13.))
                .text_color(solid(p.status_error))
                .child(err.clone())
                .into_any_element(),
        })
}

fn render_hash_section(
    ws: &Workspace,
    matches_target: bool,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let state = if matches_target {
        &ws.hash_state
    } else {
        &HashInspectState::Idle
    };

    v_flex()
        .w_full()
        .p_2p5()
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
                        .text_size(px(13.))
                        .font_semibold()
                        .text_color(solid(p.text_secondary))
                        .child("CHECKSUMS"),
                )
                .child(
                    Button::new("insp-compute-hash")
                        .with_size(Size::XSmall)
                        .ghost()
                        .label("Compute")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.compute_checksums_for_selection(cx);
                        })),
                ),
        )
        .child(match state {
            HashInspectState::Idle => div()
                .text_size(px(13.))
                .text_color(solid(p.text_muted))
                .child("Calculate streaming SHA-256 and MD5 digests.")
                .into_any_element(),
            HashInspectState::Computing => div()
                .text_size(px(13.))
                .text_color(solid(p.status_info))
                .child("Computing SHA-256 & MD5...")
                .into_any_element(),
            HashInspectState::Error(err) => div()
                .text_size(px(13.))
                .text_color(solid(p.status_error))
                .child(err.clone())
                .into_any_element(),
            HashInspectState::Ready(hashes) => {
                let sha_full = hashes.sha256.clone();
                let md5_full = hashes.md5.clone();
                let sha_short: SharedString =
                    format!("{}…{}", &hashes.sha256[..12], &hashes.sha256[56..]).into();
                let md5_short: SharedString =
                    format!("{}…{}", &hashes.md5[..10], &hashes.md5[24..]).into();
                let verify_val = ws.hash_verify_text.trim();
                let verify_badge: Option<(SharedString, u32)> = if verify_val.is_empty() {
                    None
                } else if let Some(algo) = verify_hash_match(hashes, verify_val) {
                    Some((format!("Match ({algo})").into(), p.status_success))
                } else {
                    Some(("Hash Mismatch".into(), p.status_error))
                };

                v_flex()
                    .gap_1p5()
                    .child(hash_digest_row(
                        "SHA-256",
                        sha_short,
                        "insp-copy-sha256",
                        sha_full,
                        p,
                        cx,
                    ))
                    .child(hash_digest_row(
                        "MD5",
                        md5_short,
                        "insp-copy-md5",
                        md5_full,
                        p,
                        cx,
                    ))
                    .child(
                        div()
                            .w_full()
                            .h(px(30.))
                            .px_2()
                            .rounded(px(RADIUS_CONTROL))
                            .bg(solid(p.bg_surface))
                            .border_1()
                            .border_color(solid(p.border_subtle))
                            .child(full_hit_input(
                                "insp-hash-verify-hit",
                                &ws.hash_verify_input,
                                Input::new(&ws.hash_verify_input)
                                    .with_size(Size::XSmall)
                                    .appearance(false),
                            )),
                    )
                    .when_some(verify_badge, |el, (label, color)| {
                        el.child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded(px(RADIUS_CONTROL))
                                .bg(soft_tint(color, 0.14))
                                .text_size(px(13.))
                                .font_semibold()
                                .text_color(solid(color))
                                .child(label),
                        )
                    })
                    .into_any_element()
            }
        })
}

fn hash_digest_row(
    label: &'static str,
    preview: SharedString,
    copy_btn_id: &'static str,
    full_value: String,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .px_2()
        .py_1()
        .items_center()
        .justify_between()
        .gap_1p5()
        .rounded(px(RADIUS_CONTROL))
        .bg(solid(p.bg_elevated))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(12.))
                .font_semibold()
                .text_color(solid(p.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family("Consolas")
                .text_size(px(12.))
                .text_color(solid(p.text_primary))
                .child(preview),
        )
        .child(
            Button::new(copy_btn_id)
                .with_size(Size::XSmall)
                .ghost()
                .icon(LucideIcon::Copy)
                .tooltip(format!("Copy {label}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copy_text_to_clipboard(&full_value, &format!("{label} copied"), cx);
                })),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_inline_preview(
    payload: &PreviewPayload,
    preview_gen: u64,
    sys_icon: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    fallback_icon: LucideIcon,
    fallback_hex: u32,
    unit: texplorer_core::SizeUnit,
    p: Palette,
) -> impl IntoElement {
    match payload {
        PreviewPayload::Image {
            image,
            orig_width,
            orig_height,
            frame_count,
        } => v_flex()
            .w_full()
            .items_center()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .h(px(164.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(
                        img(image.clone())
                            .id(("insp-preview-img", preview_gen))
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(solid(p.text_muted))
                    .child(if *frame_count > 1 {
                        format!("{orig_width} × {orig_height}  •  Animated")
                    } else {
                        format!("{orig_width} × {orig_height}")
                    }),
            )
            .into_any_element(),
        PreviewPayload::Pdf {
            image,
            page_count,
            ..
        } => v_flex()
            .w_full()
            .items_center()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .h(px(172.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(
                        img(image.clone())
                            .id(("insp-preview-pdf", preview_gen))
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(solid(p.text_muted))
                    .child(if *page_count == 1 {
                        "PDF Document  •  1 page".to_string()
                    } else {
                        format!("PDF Document  •  {page_count} pages")
                    }),
            )
            .into_any_element(),
        PreviewPayload::ShellThumbnail { image, .. } => div()
            .w_full()
            .h(px(148.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(image.clone())
                    .w(px(128.))
                    .h(px(128.))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element(),
        PreviewPayload::Code { lines, .. } => v_flex()
            .w_full()
            .h(px(148.))
            .p_2()
            .overflow_hidden()
            .rounded(px(RADIUS_CONTROL))
            .bg(solid(p.bg_surface))
            .border_1()
            .border_color(solid(p.border_subtle))
            .font_family("Consolas")
            .children(lines.iter().take(9).map(|line| {
                div()
                    .w_full()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(solid(p.text_primary))
                    .child(
                        StyledText::new(line.text.clone()).with_highlights(line.spans.clone()),
                    )
            }))
            .into_any_element(),
        PreviewPayload::Markdown { source, .. } => v_flex()
            .w_full()
            .h(px(140.))
            .p_2()
            .overflow_hidden()
            .rounded(px(RADIUS_CONTROL))
            .bg(solid(p.bg_surface))
            .border_1()
            .border_color(solid(p.border_subtle))
            .children(source.lines().take(8).map(|l| {
                div()
                    .w_full()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(solid(p.text_secondary))
                    .child(l.to_string())
            }))
            .into_any_element(),
        PreviewPayload::FolderSummary {
            total_items,
            files,
            folders,
            immediate_bytes,
        } => v_flex()
            .items_center()
            .gap_1p5()
            .child(render_icon_box(sys_icon, fallback_icon, fallback_hex))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(solid(p.text_secondary))
                    .child(format!(
                        "{total_items} items ({folders} dirs, {files} files • {})",
                        format_size(*immediate_bytes, unit)
                    )),
            )
            .into_any_element(),
        _ => render_icon_box(sys_icon, fallback_icon, fallback_hex).into_any_element(),
    }
}

fn render_icon_box(
    sys_icon: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    icon: LucideIcon,
    icon_hex: u32,
) -> impl IntoElement {
    if let Some(render_img) = sys_icon {
        div()
            .w(px(52.))
            .h(px(52.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(render_img)
                    .w(px(44.))
                    .h(px(44.))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element()
    } else {
        div()
            .w(px(52.))
            .h(px(52.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_ROW))
            .bg(soft_tint(icon_hex, 0.14))
            .child(Icon::new(icon).large().text_color(solid(icon_hex)))
            .into_any_element()
    }
}

fn meta_row(label: &'static str, value: SharedString, p: Palette) -> impl IntoElement {
    h_flex()
        .w_full()
        .px_2p5()
        .py_1p5()
        .items_center()
        .justify_between()
        .gap_2()
        .border_b_1()
        .border_color(solid(p.border_subtle))
        .child(
            div()
                .text_size(px(13.))
                .text_color(solid(p.text_muted))
                .child(label),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(solid(p.text_primary))
                .child(value),
        )
}
