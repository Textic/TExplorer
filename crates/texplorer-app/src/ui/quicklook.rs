use gpui_kit::{
    Context, MouseButton, MouseDownEvent, ObjectFit, ScrollDelta, ScrollEvent, SharedString,
    StyledText,
    assets::IconName as LucideIcon,
    base::TextView,
    canvas,
    component::{
        Disableable as _, Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, img, prelude::*, px, relative,
};
use texplorer_core::FileEntry;

use crate::preview::PreviewPayload;
use crate::theme::{
    Palette, RADIUS_CONTROL, RADIUS_ROW, file_kind_visual, format_size, format_timestamp,
    soft_tint, solid,
};
use crate::ui::workspace::{VideoPlaybackState, Workspace};

/// Formats playback timestamps as `M:SS` or `H:MM:SS`.
pub fn format_media_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total = seconds.floor() as u64;
    let hrs = total / 3600;
    let mins = (total % 3600) / 60;
    let secs = total % 60;
    if hrs > 0 {
        format!("{hrs}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

/// Renders the centered flat QuickLook modal triggered by `Space`.
pub fn render_quicklook_modal(
    ws: &Workspace,
    entry: &FileEntry,
    payload: &PreviewPayload,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let (icon, icon_hex) = file_kind_visual(entry.kind, p);
    let name: SharedString = entry.name.clone().into();
    let is_dir = entry.is_dir();
    let open_loc = entry.location.clone();

    let active_video = ws.video_playback.as_ref().filter(|v| {
        entry
            .location
            .as_local_path()
            .is_some_and(|lp| lp == v.path.as_path())
    });
    let video_playable = active_video.is_some_and(|v| v.error.is_none());

    let badge_text: SharedString = if let Some(video) = active_video.filter(|v| v.error.is_none()) {
        if video.native_width > 0 && video.duration_secs > 0.0 {
            format!(
                "{}×{}  •  {}",
                video.native_width,
                video.native_height,
                format_media_time(video.duration_secs)
            )
            .into()
        } else if video.native_width > 0 {
            format!("{}×{}", video.native_width, video.native_height).into()
        } else {
            entry
                .size
                .map(|b| format!("{}  •  {}", entry.type_label, format_size(b, unit)))
                .unwrap_or_else(|| entry.type_label.clone())
                .into()
        }
    } else {
        match payload {
            PreviewPayload::Image {
                orig_width,
                orig_height,
                frame_count,
                ..
            } => {
                let size_str = entry
                    .size
                    .map(|b| format_size(b, unit))
                    .unwrap_or_default();
                if *frame_count > 1 {
                    format!("{orig_width}×{orig_height}  •  Animated  •  {size_str}").into()
                } else {
                    format!("{orig_width}×{orig_height}  •  {size_str}").into()
                }
            }
            PreviewPayload::Pdf {
                page_index,
                page_count,
                ..
            } => {
                let page_num = page_index.saturating_add(1);
                if *page_count <= 1 {
                    "PDF  •  1 page".into()
                } else {
                    format!("PDF  •  Page {page_num} of {page_count}").into()
                }
            }
            PreviewPayload::Code {
                language,
                lines,
                truncated,
            } => format!(
                "{language}  •  {} lines{}",
                lines.len(),
                if *truncated { "+" } else { "" }
            )
            .into(),
            PreviewPayload::Markdown { truncated, .. } => {
                if *truncated {
                    "Markdown (truncated)".into()
                } else {
                    "Markdown".into()
                }
            }
            PreviewPayload::FolderSummary {
                total_items,
                folders,
                files,
                ..
            } => format!("{total_items} items ({folders} folders, {files} files)").into(),
            PreviewPayload::ShellThumbnail { subtitle, .. } => subtitle.clone().into(),
            _ => entry.type_label.clone().into(),
        }
    };

    let body = if let Some(video) = active_video.filter(|v| v.error.is_none()) {
        render_video_player_body(video, payload, p, cx).into_any_element()
    } else {
        let video_err = active_video.and_then(|v| v.error.as_deref());
        match payload {
            PreviewPayload::Loading => v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child("Loading preview..."),
                )
                .into_any_element(),
            // Static images AND animated GIFs / animated WebPs (pure looping animation, NO video controls).
            PreviewPayload::Image { image, .. } => div()
                .flex_1()
                .size_full()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .bg(solid(p.bg_base))
                .child(
                    img(image.clone())
                        .id(("ql-preview-img", ws.preview_generation))
                        .max_w_full()
                        .max_h_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            PreviewPayload::Pdf {
                image,
                page_index,
                page_count,
                page_width,
                page_height,
                loading_page,
            } => render_pdf_preview_body(
                ws,
                PdfViewData {
                    image: image.clone(),
                    page_index: *page_index,
                    page_count: *page_count,
                    page_width: *page_width,
                    page_height: *page_height,
                    loading_page: *loading_page,
                },
                p,
                cx,
            )
            .into_any_element(),
            PreviewPayload::ShellThumbnail { image, .. } => v_flex()
                .flex_1()
                .size_full()
                .p_6()
                .gap_4()
                .items_center()
                .justify_center()
                .bg(solid(p.bg_base))
                .child(
                    img(image.clone())
                        .w(px(280.))
                        .h(px(280.))
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_secondary))
                        .child(format!(
                            "{}  •  Modified {}",
                            entry
                                .size
                                .map(|b| format_size(b, unit))
                                .unwrap_or_else(|| entry.type_label.clone()),
                            format_timestamp(entry.modified_secs)
                        )),
                )
                .when_some(video_err, |el, err_msg| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child(err_msg.to_string()),
                    )
                })
                .into_any_element(),
            PreviewPayload::Markdown { source, .. } => v_flex()
                .id("quicklook-markdown-scroll")
                .flex_1()
                .size_full()
                .overflow_y_scrollbar()
                .p_5()
                .bg(solid(p.bg_base))
                .child(TextView::markdown("quicklook-md-view", source.clone()))
                .into_any_element(),
            PreviewPayload::Code {
                lines, truncated, ..
            } => v_flex()
                .id("quicklook-code-scroll")
                .flex_1()
                .size_full()
                .overflow_y_scrollbar()
                .p_3()
                .bg(solid(p.bg_base))
                .font_family("Consolas")
                .children(lines.iter().map(|line| {
                    let ln: SharedString = format!("{:>4}", line.line_number).into();
                    let styled =
                        StyledText::new(line.text.clone()).with_highlights(line.spans.clone());
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(
                            div()
                                .w(px(42.))
                                .flex_shrink_0()
                                .text_right()
                                .text_size(px(12.5))
                                .text_color(solid(p.text_muted))
                                .child(ln),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(14.))
                                .text_color(solid(p.text_primary))
                                .child(styled),
                        )
                }))
                .when(*truncated, |el| {
                    el.child(
                        div()
                            .pt_3()
                            .text_xs()
                            .text_color(solid(p.text_muted))
                            .child("Preview truncated to first 192 KiB."),
                    )
                })
                .into_any_element(),
            PreviewPayload::FolderSummary {
                total_items,
                files,
                folders,
                immediate_bytes,
            } => v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(solid(p.bg_base))
                .child(
                    div()
                        .w(px(64.))
                        .h(px(64.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_ROW))
                        .bg(soft_tint(p.accent_primary, 0.14))
                        .child(
                            Icon::new(LucideIcon::Folder)
                                .large()
                                .text_color(solid(p.accent_primary)),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.text_primary))
                        .child(format!("{total_items} immediate items")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_secondary))
                        .child(format!(
                            "{folders} folders  •  {files} files ({})",
                            format_size(*immediate_bytes, unit)
                        )),
                )
                .into_any_element(),
            PreviewPayload::Unsupported { reason } => v_flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .bg(solid(p.bg_base))
                .child(Icon::new(icon).large().text_color(solid(icon_hex)))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(solid(p.text_primary))
                        .child(entry.type_label.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(p.text_muted))
                        .child(video_err.unwrap_or(reason.as_str()).to_string()),
                )
                .into_any_element(),
        }
    };

    let is_multipage_pdf = matches!(
        payload,
        PreviewPayload::Pdf { page_count, .. } if *page_count > 1
    );
    let footer_hint = if video_playable {
        "K Play/Pause   •   ← / → Seek 5s   •   M Mute   •   ↑ / ↓ Files   •   Space / Esc Close"
    } else if is_multipage_pdf {
        "← / → or PgUp / PgDn Pages   •   Scroll Flip pages   •   ↑ / ↓ Files   •   Enter Open   •   Space / Esc Close"
    } else {
        "↑ / ↓ Navigate files   •   Enter Open   •   Space / Esc Close"
    };

    div()
        .id("quicklook-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .child(
            div()
                .id("quicklook-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, p.backdrop_alpha()))
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_quicklook(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_quicklook(cx);
                    }),
                ),
        )
        .child(
            v_flex()
                .id("quicklook-card")
                .relative()
                .w(px(860.))
                .h(px(640.))
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
                        .px_3p5()
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .bg(solid(p.bg_elevated))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(Icon::new(icon).small().text_color(solid(icon_hex)))
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(solid(p.text_primary))
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(solid(p.bg_base))
                                        .border_1()
                                        .border_color(solid(p.border_subtle))
                                        .text_size(px(13.))
                                        .text_color(solid(p.text_secondary))
                                        .child(badge_text),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    Button::new("ql-open")
                                        .with_size(Size::XSmall)
                                        .primary()
                                        .label("Open")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.close_quicklook(cx);
                                            this.open_location_or_file(
                                                open_loc.clone(),
                                                is_dir,
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("ql-close")
                                        .with_size(Size::XSmall)
                                        .ghost()
                                        .icon(LucideIcon::X)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_quicklook(cx);
                                        })),
                                ),
                        ),
                )
                // Main preview body
                .child(div().flex_1().min_h_0().w_full().overflow_hidden().child(body))
                // Footer hint strip
                .child(
                    h_flex()
                        .w_full()
                        .h(px(32.))
                        .px_3p5()
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .bg(solid(p.bg_elevated))
                        .border_t_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(solid(p.text_muted))
                                .child(footer_hint),
                        ),
                ),
        )
}

struct PdfViewData {
    image: std::sync::Arc<gpui_kit::RenderImage>,
    page_index: u32,
    page_count: u32,
    page_width: u32,
    page_height: u32,
    loading_page: Option<u32>,
}

fn render_pdf_preview_body(
    ws: &Workspace,
    pdf: PdfViewData,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let PdfViewData {
        image,
        page_index,
        page_count,
        page_width,
        page_height,
        loading_page,
    } = pdf;
    let can_prev = page_index > 0;
    let can_next = page_index.saturating_add(1) < page_count;
    let zoom = ws.pdf_zoom;
    let is_fit = (zoom - 1.0).abs() < 0.01;
    let page_label: SharedString = if let Some(target) = loading_page {
        format!("Loading page {} / {}...", target + 1, page_count).into()
    } else {
        format!("Page {} / {}", page_index + 1, page_count).into()
    };
    let zoom_label: SharedString = if is_fit {
        "Fit".into()
    } else {
        format!("{}%", (zoom * 100.0).round() as u32).into()
    };
    let img_id = ("ql-preview-pdf", ws.preview_generation ^ ((page_index as u64) << 32));

    let stage = if is_fit {
        div()
            .id("ql-pdf-stage-fit")
            .flex_1()
            .min_h_0()
            .w_full()
            .p_4()
            .flex()
            .items_center()
            .justify_center()
            .bg(solid(p.bg_base))
            .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                cx.stop_propagation();
                if page_count <= 1 {
                    return;
                }
                let dy = match ev.delta {
                    ScrollDelta::Lines(pt) => pt.y,
                    ScrollDelta::Pixels(pt) => f32::from(pt.y) / 40.0,
                };
                if dy < -0.15 {
                    this.pdf_step_page(1, cx);
                } else if dy > 0.15 {
                    this.pdf_step_page(-1, cx);
                }
            }))
            .child(
                div()
                    .max_w_full()
                    .max_h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_CONTROL))
                    .border_1()
                    .border_color(solid(p.border))
                    .overflow_hidden()
                    .child(
                        img(image)
                            .id(img_id)
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .into_any_element()
    } else {
        let base_w = 780.0_f32;
        let aspect = if page_width > 0 {
            page_height as f32 / page_width as f32
        } else {
            1.294
        };
        let render_w = (base_w * zoom).clamp(320.0, 2200.0);
        let render_h = (render_w * aspect).max(200.0);
        v_flex()
            .id("ql-pdf-stage-zoom")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scrollbar()
            .p_4()
            .items_center()
            .bg(solid(p.bg_base))
            .child(
                div()
                    .w(px(render_w))
                    .h(px(render_h))
                    .flex_shrink_0()
                    .rounded(px(RADIUS_CONTROL))
                    .border_1()
                    .border_color(solid(p.border))
                    .overflow_hidden()
                    .child(
                        img(image)
                            .id(img_id)
                            .w_full()
                            .h_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .into_any_element()
    };

    v_flex()
        .flex_1()
        .size_full()
        .bg(solid(p.bg_base))
        .child(stage)
        .child(
            h_flex()
                .w_full()
                .h(px(40.))
                .px_3p5()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .gap_3()
                .bg(solid(p.bg_elevated))
                .border_t_1()
                .border_color(solid(p.border_subtle))
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Button::new("ql-pdf-prev")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::ChevronLeft)
                                .disabled(!can_prev)
                                .tooltip("Previous Page (← / PgUp)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pdf_step_page(-1, cx);
                                })),
                        )
                        .child(
                            div()
                                .min_w(px(110.))
                                .text_center()
                                .font_family("Consolas")
                                .text_size(px(12.5))
                                .text_color(solid(if loading_page.is_some() {
                                    p.accent_primary
                                } else {
                                    p.text_primary
                                }))
                                .child(page_label),
                        )
                        .child(
                            Button::new("ql-pdf-next")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::ChevronRight)
                                .disabled(!can_next)
                                .tooltip("Next Page (→ / PgDn)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pdf_step_page(1, cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("ql-pdf-zoom-fit")
                                .with_size(Size::XSmall)
                                .ghost()
                                .label("Fit")
                                .disabled(is_fit)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pdf_set_zoom(1.0, cx);
                                })),
                        )
                        .child(
                            Button::new("ql-pdf-zoom-out")
                                .with_size(Size::XSmall)
                                .ghost()
                                .label("−")
                                .disabled(zoom <= 1.0)
                                .tooltip("Zoom Out")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pdf_adjust_zoom(-0.25, cx);
                                })),
                        )
                        .child(
                            div()
                                .w(px(44.))
                                .text_center()
                                .font_family("Consolas")
                                .text_size(px(12.))
                                .text_color(solid(p.text_secondary))
                                .child(zoom_label),
                        )
                        .child(
                            Button::new("ql-pdf-zoom-in")
                                .with_size(Size::XSmall)
                                .ghost()
                                .label("+")
                                .disabled(zoom >= 2.0)
                                .tooltip("Zoom In")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pdf_adjust_zoom(0.25, cx);
                                })),
                        ),
                ),
        )
}

fn render_video_player_body(
    video: &VideoPlaybackState,
    fallback_payload: &PreviewPayload,
    p: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let progress = if video.duration_secs > 0.0 {
        (video.position_secs / video.duration_secs).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let pos_label: SharedString = format_media_time(video.position_secs).into();
    let dur_label: SharedString = format_media_time(video.duration_secs).into();
    let bounds_cell = video.seek_bar_bounds.clone();
    let volume_bounds_cell = video.volume_bar_bounds.clone();
    let effective_vol: u8 = if video.muted {
        0
    } else {
        video.volume.min(100)
    };
    let vol_progress: f32 = effective_vol as f32 / 100.0;
    let vol_icon = if effective_vol == 0 {
        LucideIcon::VolumeX
    } else if effective_vol < 50 {
        LucideIcon::Volume1
    } else {
        LucideIcon::Volume2
    };
    let vol_label: SharedString = format!("{effective_vol}%").into();

    let stage_content = if let Some(frame) = video.frame.as_ref() {
        img(frame.clone())
            .max_w_full()
            .max_h_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element()
    } else if let PreviewPayload::ShellThumbnail { image, .. } = fallback_payload {
        img(image.clone())
            .max_w_full()
            .max_h_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element()
    } else {
        div()
            .text_xs()
            .text_color(solid(p.text_muted))
            .child("Loading video...")
            .into_any_element()
    };

    v_flex()
        .flex_1()
        .size_full()
        .bg(solid(p.bg_base))
        // Video frame canvas (click anywhere on video to play/pause)
        .child(
            div()
                .id("ql-video-stage")
                .flex_1()
                .min_h_0()
                .w_full()
                .p_3()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.video_toggle_play(cx);
                    }),
                )
                .child(stage_content),
        )
        // Flat video transport & seek bar (shown ONLY for videos, never for GIFs)
        .child(
            h_flex()
                .w_full()
                .h(px(44.))
                .px_3p5()
                .gap_3()
                .flex_shrink_0()
                .items_center()
                .bg(solid(p.bg_elevated))
                .border_t_1()
                .border_color(solid(p.border_subtle))
                .child(
                    Button::new("ql-video-play-toggle")
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(if video.playing {
                            LucideIcon::Pause
                        } else {
                            LucideIcon::Play
                        })
                        .tooltip(if video.playing {
                            "Pause (K)"
                        } else {
                            "Play (K)"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.video_toggle_play(cx);
                        })),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .font_family("Consolas")
                        .text_size(px(12.5))
                        .text_color(solid(p.text_secondary))
                        .child(pos_label),
                )
                .child(
                    div()
                        .id("ql-video-seek-bar")
                        .flex_1()
                        .h(px(24.))
                        .relative()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.video_seek_to_position(ev.position.x, true, cx);
                            }),
                        )
                        // Track rail + filled progress
                        .child(
                            div()
                                .w_full()
                                .h(px(4.))
                                .rounded(px(999.))
                                .bg(solid(p.border))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(progress))
                                        .bg(solid(p.accent_primary)),
                                ),
                        )
                        // Playhead thumb
                        .child(
                            div()
                                .absolute()
                                .top(px(6.))
                                .left(relative(progress))
                                .ml(px(-6.))
                                .w(px(12.))
                                .h(px(12.))
                                .rounded(px(999.))
                                .bg(solid(p.accent_primary)),
                        )
                        // Zero-cost bounds measurer for click & drag scrubbing
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    bounds_cell.set(Some(bounds));
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .font_family("Consolas")
                        .text_size(px(12.5))
                        .text_color(solid(p.text_muted))
                        .child(dur_label),
                )
                .child(
                    h_flex()
                        .id("ql-video-volume-control")
                        .flex_shrink_0()
                        .items_center()
                        .gap_1p5()
                        .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                            cx.stop_propagation();
                            let dy = match ev.delta {
                                ScrollDelta::Lines(pt) => pt.y,
                                ScrollDelta::Pixels(pt) => f32::from(pt.y) / 20.0,
                            };
                            if dy.abs() > 0.001 {
                                let step = if dy > 0.0 { 5 } else { -5 };
                                this.video_adjust_volume_step(step, cx);
                            }
                        }))
                        .child(
                            Button::new("ql-video-mute-toggle")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(vol_icon)
                                .tooltip(if effective_vol == 0 {
                                    "Unmute (M) • Scroll to adjust volume"
                                } else {
                                    "Mute (M) • Scroll to adjust volume"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.video_toggle_mute(cx);
                                })),
                        )
                        .child(
                            div()
                                .id("ql-video-volume-bar")
                                .w(px(76.))
                                .h(px(24.))
                                .relative()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.video_set_volume_from_position(
                                            ev.position.x,
                                            true,
                                            cx,
                                        );
                                    }),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(4.))
                                        .rounded(px(999.))
                                        .bg(solid(p.border))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(vol_progress))
                                                .bg(solid(p.accent_primary)),
                                        ),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(7.))
                                        .left(relative(vol_progress))
                                        .ml(px(-5.))
                                        .w(px(10.))
                                        .h(px(10.))
                                        .rounded(px(999.))
                                        .bg(solid(p.accent_primary)),
                                )
                                .child(
                                    canvas(
                                        move |bounds, _, _| {
                                            volume_bounds_cell.set(Some(bounds));
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                ),
                        )
                        .child(
                            div()
                                .w(px(30.))
                                .text_right()
                                .font_family("Consolas")
                                .text_size(px(11.5))
                                .text_color(solid(p.text_muted))
                                .child(vol_label),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_media_time_cleanly() {
        assert_eq!(format_media_time(0.0), "0:00");
        assert_eq!(format_media_time(-5.0), "0:00");
        assert_eq!(format_media_time(f64::NAN), "0:00");
        assert_eq!(format_media_time(9.8), "0:09");
        assert_eq!(format_media_time(65.4), "1:05");
        assert_eq!(format_media_time(3661.0), "1:01:01");
    }
}
