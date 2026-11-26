use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::{
    Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, SharedString,
    UniformListScrollHandle,
    assets::IconName as LucideIcon,
    component::{
        Disableable as _, Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    div, prelude::*, px, uniform_list,
};
use texplorer_core::PaneSide;
use texplorer_ops::{BulkRenamePreviewRow, BulkRenameSpec, RenameCasing};

use crate::theme::{RADIUS_CONTROL, RADIUS_ROW, full_hit_input, soft_tint, solid};
use crate::ui::workspace::Workspace;

/// Interactive state for the `Ctrl+Shift+R` Bulk Rename modal.
pub struct BulkRenameState {
    pub side: PaneSide,
    pub sources: Vec<PathBuf>,
    pub find_input: Entity<InputState>,
    pub replace_input: Entity<InputState>,
    pub template_input: Entity<InputState>,
    pub ext_input: Entity<InputState>,
    pub spec: BulkRenameSpec,
    pub preview_rows: Vec<BulkRenamePreviewRow>,
    pub regex_error: Option<String>,
    pub scroll_handle: UniformListScrollHandle,
}

/// Renders the flat Bulk Rename modal with live before -> after preview and collision checks.
pub fn render_bulk_rename_modal(
    ws: &Workspace,
    state: &BulkRenameState,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let total = state.preview_rows.len();
    let changed_count = state.preview_rows.iter().filter(|r| r.changed).count();
    let conflict_count = state
        .preview_rows
        .iter()
        .filter(|r| r.conflict.is_some())
        .count();
    let can_apply = state.regex_error.is_none() && conflict_count == 0 && changed_count > 0;
    let use_regex = state.spec.use_regex;
    let case_sensitive = state.spec.case_sensitive;
    let active_casing = state.spec.casing;
    let regex_err = state.regex_error.clone();

    let status_text: SharedString = if let Some(err) = &regex_err {
        err.clone().into()
    } else if conflict_count > 0 {
        format!("{conflict_count} conflict(s) detected — resolve before applying").into()
    } else if changed_count == 0 {
        format!("{total} item(s) selected — enter a rule above to preview new names").into()
    } else {
        format!("Ready: {changed_count} of {total} item(s) will be renamed (undoable with Ctrl+Z)")
            .into()
    };

    div()
        .id("bulk-rename-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
            if ev.keystroke.key.as_str() == "escape" {
                this.close_bulk_rename(window, cx);
                cx.stop_propagation();
            }
        }))
        .child(
            div()
                .id("bulk-rename-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, p.backdrop_alpha()))
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_bulk_rename(window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_bulk_rename(window, cx);
                    }),
                ),
        )
        .child(
            v_flex()
                .id("bulk-rename-card")
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
                                    Icon::new(LucideIcon::Pencil)
                                        .small()
                                        .text_color(solid(p.accent_primary)),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(solid(p.text_primary))
                                        .child(format!("Bulk Rename ({total} items)")),
                                ),
                        )
                        .child(
                            Button::new("br-close")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::X)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_bulk_rename(window, cx);
                                })),
                        ),
                )
                // Rule controls panel
                .child(
                    v_flex()
                        .w_full()
                        .flex_shrink_0()
                        .p_3p5()
                        .gap_2p5()
                        .bg(solid(p.bg_surface))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        // Row 1: Find + Replace + Regex / Case toggles
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(labeled_input(
                                    "Find",
                                    "br-find-hit",
                                    &state.find_input,
                                    p,
                                ))
                                .child(labeled_input(
                                    "Replace",
                                    "br-replace-hit",
                                    &state.replace_input,
                                    p,
                                ))
                                .child(
                                    v_flex()
                                        .flex_shrink_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(solid(p.text_muted))
                                                .child("Options"),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(toggle_chip(
                                                    "br-toggle-regex",
                                                    "Regex",
                                                    use_regex,
                                                    p,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.toggle_bulk_rename_regex(cx);
                                                    }),
                                                ))
                                                .child(toggle_chip(
                                                    "br-toggle-case",
                                                    "Aa",
                                                    case_sensitive,
                                                    p,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.toggle_bulk_rename_case(cx);
                                                    }),
                                                )),
                                        ),
                                ),
                        )
                        // Row 2: Template + Extension override
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(labeled_input(
                                    "Template ({name}, {ext}, {n:03}, {date})",
                                    "br-template-hit",
                                    &state.template_input,
                                    p,
                                ))
                                .child(
                                    v_flex()
                                        .w(px(156.))
                                        .flex_shrink_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(solid(p.text_muted))
                                                .child("Extension"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(34.))
                                                .px_2()
                                                .rounded(px(RADIUS_CONTROL))
                                                .bg(solid(p.bg_base))
                                                .border_1()
                                                .border_color(solid(p.border_subtle))
                                                .child(full_hit_input(
                                                    "br-ext-hit",
                                                    &state.ext_input,
                                                    Input::new(&state.ext_input)
                                                        .with_size(Size::XSmall)
                                                        .appearance(false),
                                                )),
                                        ),
                                ),
                        )
                        // Row 3: Casing pills
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .text_color(solid(p.text_secondary))
                                        .child("Casing:"),
                                )
                                .child(h_flex().flex_wrap().gap_1().children(
                                    RenameCasing::ALL.into_iter().map(|casing| {
                                        let active = active_casing == casing;
                                        toggle_chip(
                                            casing.label(),
                                            casing.label(),
                                            active,
                                            p,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_bulk_rename_casing(casing, cx);
                                            }),
                                        )
                                    }),
                                )),
                        ),
                )
                // Preview column header
                .child(
                    h_flex()
                        .w_full()
                        .h(px(32.))
                        .px_3p5()
                        .flex_shrink_0()
                        .items_center()
                        .gap_3()
                        .bg(solid(p.bg_elevated))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(13.))
                                .font_semibold()
                                .text_color(solid(p.text_secondary))
                                .child("Original Name"),
                        )
                        .child(div().w(px(18.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(13.))
                                .font_semibold()
                                .text_color(solid(p.text_secondary))
                                .child("New Name Preview"),
                        ),
                )
                // Virtualized preview rows
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .bg(solid(p.bg_base))
                        .child(
                            uniform_list(
                                "bulk-rename-preview-list",
                                total,
                                cx.processor(|this, range: Range<usize>, _window, _cx| {
                                    render_preview_rows(this, range)
                                }),
                            )
                            .track_scroll(&state.scroll_handle)
                            .size_full(),
                        ),
                )
                // Footer status + Cancel / Apply buttons
                .child(
                    h_flex()
                        .w_full()
                        .h(px(46.))
                        .px_4()
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .bg(solid(p.bg_elevated))
                        .border_t_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(if regex_err.is_some() || conflict_count > 0 {
                                    solid(p.status_error)
                                } else if changed_count > 0 {
                                    solid(p.status_success)
                                } else {
                                    solid(p.text_muted)
                                })
                                .child(status_text),
                        )
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .gap_2()
                                .child(
                                    Button::new("br-cancel")
                                        .with_size(Size::Small)
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_bulk_rename(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("br-apply")
                                        .with_size(Size::Small)
                                        .primary()
                                        .disabled(!can_apply)
                                        .label(format!("Rename ({changed_count})"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_bulk_rename(window, cx);
                                        })),
                                ),
                        ),
                ),
        )
}

fn render_preview_rows(ws: &Workspace, range: Range<usize>) -> Vec<gpui_kit::AnyElement> {
    let p = ws.palette();
    let Some(state) = ws.bulk_rename.as_ref() else {
        return Vec::new();
    };

    range
        .filter_map(|idx| {
            let row = state.preview_rows.get(idx)?;
            let old_name: SharedString = row.old_name.clone().into();
            let new_name: SharedString = row.new_name.clone().into();
            let conflict: Option<SharedString> = row.conflict.clone().map(Into::into);
            let row_bg = if conflict.is_some() {
                soft_tint(p.status_error, 0.10)
            } else if idx % 2 == 1 {
                solid(p.bg_surface)
            } else {
                solid(p.bg_base)
            };

            Some(
                h_flex()
                    .id(("br-row", idx))
                    .w_full()
                    .h(px(34.))
                    .px_3p5()
                    .gap_3()
                    .items_center()
                    .bg(row_bg)
                    .border_b_1()
                    .border_color(solid(p.border_subtle))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(solid(p.text_secondary))
                            .child(old_name),
                    )
                    .child(
                        Icon::new(LucideIcon::ArrowRight)
                            .xsmall()
                            .text_color(solid(if row.changed {
                                p.accent_primary
                            } else {
                                p.text_muted
                            })),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
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
                                    .text_color(if conflict.is_some() {
                                        solid(p.status_error)
                                    } else if row.changed {
                                        solid(p.text_primary)
                                    } else {
                                        solid(p.text_muted)
                                    })
                                    .child(new_name),
                            )
                            .when_some(conflict, |el, msg| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(3.))
                                        .bg(soft_tint(p.status_error, 0.16))
                                        .text_size(px(12.))
                                        .text_color(solid(p.status_error))
                                        .child(msg),
                                )
                            }),
                    )
                    .into_any_element(),
            )
        })
        .collect()
}

fn labeled_input(
    label: &'static str,
    hit_id: &'static str,
    input: &Entity<InputState>,
    p: crate::theme::Palette,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .text_size(px(12.))
                .text_color(solid(p.text_muted))
                .child(label),
        )
        .child(
            div()
                .w_full()
                .h(px(34.))
                .px_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(solid(p.bg_base))
                .border_1()
                .border_color(solid(p.border_subtle))
                .child(full_hit_input(
                    hit_id,
                    input,
                    Input::new(input).with_size(Size::XSmall).appearance(false),
                )),
        )
}

fn toggle_chip(
    id: &'static str,
    label: &'static str,
    active: bool,
    p: crate::theme::Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(34.))
        .px_2p5()
        .flex()
        .items_center()
        .justify_center()
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
