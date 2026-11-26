use gpui_kit::{
    Context, MouseButton, MouseDownEvent, ObjectFit, SharedString, StyledText,
    assets::IconName as LucideIcon,
    base::TextView,
    component::{
        Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, img, prelude::*, px,
};
use texplorer_core::FileEntry;

use crate::preview::PreviewPayload;
use crate::theme::{
    RADIUS_CONTROL, RADIUS_ROW, file_kind_visual, format_size, format_timestamp, soft_tint, solid,
};
use crate::ui::workspace::Workspace;

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

    let badge_text: SharedString = match payload {
        PreviewPayload::Image {
            orig_width,
            orig_height,
            ..
        } => format!(
            "{orig_width}×{orig_height}  •  {}",
            entry
                .size
                .map(|b| format_size(b, unit))
                .unwrap_or_default()
        )
        .into(),
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
    };

    let body = match payload {
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
                    .max_w_full()
                    .max_h_full()
                    .object_fit(ObjectFit::Contain),
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
                            .w(px(34.))
                            .flex_shrink_0()
                            .text_right()
                            .text_size(px(11.))
                            .text_color(solid(p.text_muted))
                            .child(ln),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
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
                    .child(reason.clone()),
            )
            .into_any_element(),
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
                                        .text_size(px(11.))
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
                        .h(px(28.))
                        .px_3p5()
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .bg(solid(p.bg_elevated))
                        .border_t_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(solid(p.text_muted))
                                .child("↑ / ↓ Navigate files   •   Enter Open   •   Space / Esc Close"),
                        ),
                ),
        )
}
