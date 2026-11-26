use gpui_kit::{
    Context, MouseButton, MouseDownEvent, SharedString,
    assets::IconName as LucideIcon,
    component::{
        Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div, prelude::*, px, relative,
};
use texplorer_ops::{ConflictPrompt, ConflictResolution, TransferStatus};

use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, format_size, format_timestamp, soft_tint, solid,
};
use crate::ui::workspace::Workspace;

/// Renders the bottom-right floating Transfer Queue drawer showing active and recent copy/move jobs.
pub fn render_transfer_drawer(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    if ws.transfers.is_empty() {
        return div().into_any_element();
    }

    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let has_finished = ws.transfers.iter().any(|t| t.snapshot.status.is_terminal());

    v_flex()
        .id("transfer-queue-drawer")
        .absolute()
        .right(px(16.))
        .bottom(px(34.))
        .w(px(380.))
        .bg(solid(p.bg_surface))
        .border_1()
        .border_color(solid(p.border))
        .rounded(px(RADIUS_ROW))
        .overflow_hidden()
        .occlude()
        .child(
            h_flex()
                .w_full()
                .h(px(36.))
                .px_3()
                .items_center()
                .justify_between()
                .bg(solid(p.bg_elevated))
                .border_b_1()
                .border_color(solid(p.border_subtle))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_semibold()
                        .text_color(solid(p.text_secondary))
                        .child("TRANSFERS"),
                )
                .when(has_finished, |el| {
                    el.child(
                        div()
                            .id("xfer-clear-done")
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(solid(p.text_muted))
                            .hover(move |s| {
                                s.bg(solid(p.bg_hover)).text_color(solid(p.text_primary))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clear_finished_transfers(cx);
                            }))
                            .child("Clear finished"),
                    )
                }),
        )
        .children(ws.transfers.iter().enumerate().map(|(idx, job)| {
            let snap = &job.snapshot;
            let job_id = snap.job_id;
            let frac = snap.fraction();
            let is_paused = matches!(snap.status, TransferStatus::Paused);
            let is_terminal = snap.status.is_terminal();

            let (status_text, bar_hex): (SharedString, u32) = match &snap.status {
                TransferStatus::Queued => ("Queued...".into(), p.text_muted),
                TransferStatus::Running => {
                    let speed = if snap.speed_bytes_per_sec > 1.0 {
                        format!(
                            " • {}/s",
                            format_size(snap.speed_bytes_per_sec as u64, unit)
                        )
                    } else {
                        String::new()
                    };
                    (
                        format!(
                            "{}/{} items ({:.0}%){speed}",
                            snap.files_done,
                            snap.files_total.max(1),
                            frac * 100.0
                        )
                        .into(),
                        p.accent_primary,
                    )
                }
                TransferStatus::Paused => ("Paused".into(), p.status_warning),
                TransferStatus::WaitingConflict => {
                    ("Waiting for conflict decision...".into(), p.status_warning)
                }
                TransferStatus::Completed => ("Completed".into(), p.status_success),
                TransferStatus::Cancelled => ("Cancelled".into(), p.text_muted),
                TransferStatus::Failed(err) => (err.clone().into(), p.status_error),
            };

            let title: SharedString = snap.title.clone().into();
            let current: SharedString = snap.current_item.clone().into();

            v_flex()
                .id(("xfer-item", idx))
                .w_full()
                .p_2p5()
                .gap_1p5()
                .border_b_1()
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
                                .font_medium()
                                .text_color(solid(p.text_primary))
                                .child(title),
                        )
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .gap_0p5()
                                .when(!is_terminal, |el| {
                                    el.child(
                                        Button::new(("xfer-pause", idx))
                                            .with_size(Size::XSmall)
                                            .ghost()
                                            .icon(if is_paused {
                                                LucideIcon::Play
                                            } else {
                                                LucideIcon::Pause
                                            })
                                            .tooltip(if is_paused { "Resume" } else { "Pause" })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.toggle_pause_transfer(job_id, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new(("xfer-cancel", idx))
                                            .with_size(Size::XSmall)
                                            .ghost()
                                            .icon(LucideIcon::X)
                                            .tooltip("Cancel")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.cancel_transfer(job_id, cx);
                                            })),
                                    )
                                })
                                .when(is_terminal, |el| {
                                    el.child(
                                        Button::new(("xfer-dismiss", idx))
                                            .with_size(Size::XSmall)
                                            .ghost()
                                            .icon(LucideIcon::X)
                                            .tooltip("Dismiss")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.dismiss_transfer(job_id, cx);
                                            })),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(solid(p.bg_base))
                        .overflow_hidden()
                        .child(div().h_full().w(relative(frac)).bg(solid(bar_hex))),
                )
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
                                .text_size(px(13.))
                                .text_color(solid(p.text_muted))
                                .child(status_text),
                        )
                        .when(!current.is_empty() && !is_terminal, |el| {
                            el.child(
                                div()
                                    .max_w(px(130.))
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(solid(p.text_secondary))
                                    .child(current),
                            )
                        }),
                )
        }))
        .into_any_element()
}

/// Renders the interactive File Conflict Resolution modal (`Overwrite | Skip | Keep Both | Cancel`).
pub fn render_conflict_modal(
    ws: &Workspace,
    prompt: &ConflictPrompt,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let apply_all = ws.conflict_apply_to_all;

    let item_name: SharedString = prompt
        .dest_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| prompt.dest_path.display().to_string())
        .into();

    let src_size: SharedString = if prompt.is_dir {
        "Folder".into()
    } else {
        format_size(prompt.source_size, unit).into()
    };
    let dst_size: SharedString = if prompt.is_dir {
        "Folder".into()
    } else {
        format_size(prompt.dest_size, unit).into()
    };
    let src_time: SharedString = format_timestamp(prompt.source_modified_secs).into();
    let dst_time: SharedString = format_timestamp(prompt.dest_modified_secs).into();

    div()
        .id("conflict-modal-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .child(
            div()
                .id("conflict-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, p.backdrop_alpha()))
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        )
        .child(
            v_flex()
                .id("conflict-card")
                .relative()
                .w(px(500.))
                .bg(solid(p.bg_surface))
                .border_1()
                .border_color(solid(p.border))
                .rounded(px(RADIUS_ROW))
                .overflow_hidden()
                .occlude()
                .child(
                    h_flex()
                        .w_full()
                        .h(px(42.))
                        .px_4()
                        .items_center()
                        .gap_2()
                        .bg(solid(p.bg_elevated))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            Icon::new(LucideIcon::TriangleAlert)
                                .small()
                                .text_color(solid(p.status_warning)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(solid(p.text_primary))
                                .child("Replace or Skip Item?"),
                        ),
                )
                .child(
                    v_flex()
                        .p_4()
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .text_color(solid(p.text_secondary))
                                .child(format!(
                                    "The destination already contains an item named \"{item_name}\"."
                                )),
                        )
                        // Source vs Destination comparison cards
                        .child(
                            h_flex()
                                .gap_2p5()
                                .child(compare_card(
                                    "Source (Incoming)",
                                    src_size,
                                    src_time,
                                    p.status_info,
                                    p,
                                ))
                                .child(compare_card(
                                    "Destination (Existing)",
                                    dst_size,
                                    dst_time,
                                    p.status_warning,
                                    p,
                                )),
                        )
                        // Apply to all toggle
                        .child(
                            h_flex()
                                .id("conflict-apply-all")
                                .py_1()
                                .gap_2()
                                .items_center()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                        this.toggle_conflict_apply_all(cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .w(px(14.))
                                        .h(px(14.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.))
                                        .bg(if apply_all {
                                            soft_tint(p.accent_primary, 0.22)
                                        } else {
                                            solid(p.bg_base)
                                        })
                                        .border_1()
                                        .border_color(if apply_all {
                                            solid(p.accent_primary)
                                        } else {
                                            solid(p.border)
                                        })
                                        .when(apply_all, |el| {
                                            el.child(
                                                Icon::new(LucideIcon::Check)
                                                    .xsmall()
                                                    .text_color(solid(p.accent_primary)),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(solid(p.text_secondary))
                                        .child("Apply this decision to all remaining conflicts"),
                                ),
                        )
                        // Action buttons
                        .child(
                            h_flex()
                                .pt_1()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("cfl-cancel")
                                        .with_size(Size::Small)
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.resolve_active_conflict(
                                                ConflictResolution::Cancel,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("cfl-skip")
                                        .with_size(Size::Small)
                                        .label("Skip")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.resolve_active_conflict(
                                                ConflictResolution::Skip,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("cfl-keep")
                                        .with_size(Size::Small)
                                        .label("Keep Both")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.resolve_active_conflict(
                                                ConflictResolution::KeepBoth,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("cfl-overwrite")
                                        .with_size(Size::Small)
                                        .primary()
                                        .label("Overwrite")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.resolve_active_conflict(
                                                ConflictResolution::Overwrite,
                                                cx,
                                            );
                                        })),
                                ),
                        ),
                ),
        )
}

fn compare_card(
    heading: &'static str,
    size_text: SharedString,
    time_text: SharedString,
    dot_hex: u32,
    p: Palette,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .p_2p5()
        .gap_1()
        .rounded(px(RADIUS_ROW))
        .bg(solid(p.bg_base))
        .border_1()
        .border_color(solid(p.border_subtle))
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .child(div().w(px(6.)).h(px(6.)).rounded(px(999.)).bg(solid(dot_hex)))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_semibold()
                        .text_color(solid(p.text_primary))
                        .child(heading),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(solid(p.text_secondary))
                .child(size_text),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(solid(p.text_muted))
                .child(time_text),
        )
}
