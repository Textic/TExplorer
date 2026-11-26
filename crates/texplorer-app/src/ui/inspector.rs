use gpui_kit::{
    Context, SharedString,
    assets::IconName as LucideIcon,
    component::{
        Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, prelude::*, px,
};
use texplorer_core::EntryFlags;

use crate::theme::{
    BG_BASE, BG_ELEVATED, BG_SURFACE, BORDER_SUBTLE, RADIUS_CONTROL, RADIUS_ROW, TEXT_MUTED,
    TEXT_PRIMARY, TEXT_SECONDARY, file_kind_visual, format_size, format_timestamp, soft_tint,
    solid,
};
use crate::ui::workspace::Workspace;

/// Renders the collapsible right-hand Preview & Inspector panel (`Alt+P`).
pub fn render_inspector(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let accent = ws.settings.accent;
    let unit = ws.settings.size_unit;
    let active_pane = ws.active_tab().map(|t| t.model.active_pane_ref());
    let primary = active_pane.and_then(|p| p.primary_selected_entry()).cloned();
    let sel_count = active_pane.map(|p| p.selection.len()).unwrap_or(0);

    v_flex()
        .id("inspector-panel")
        .size_full()
        .bg(solid(BG_SURFACE))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .h(px(38.))
                .px_3()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(solid(BORDER_SUBTLE))
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(solid(TEXT_SECONDARY))
                        .child("INSPECTOR"),
                )
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
                        .text_color(solid(TEXT_PRIMARY))
                        .child("No selection"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(solid(TEXT_MUTED))
                        .child("Select a file or folder to inspect details."),
                )
                .into_any_element(),
            Some(entry) => {
                let (icon, icon_hex) = file_kind_visual(entry.kind, accent);
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
                let local_path = entry.location.as_local_path().map(|p| p.to_path_buf());
                let is_dir = entry.is_dir();

                let mut attr_badges: Vec<&'static str> = Vec::new();
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

                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .gap_3()
                    // Semantic header preview card
                    .child(
                        v_flex()
                            .w_full()
                            .p_4()
                            .gap_2()
                            .items_center()
                            .rounded(px(RADIUS_ROW))
                            .bg(solid(BG_BASE))
                            .border_1()
                            .border_color(solid(BORDER_SUBTLE))
                            .child(
                                div()
                                    .w(px(52.))
                                    .h(px(52.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(RADIUS_ROW))
                                    .bg(soft_tint(icon_hex, 0.14))
                                    .child(Icon::new(icon).large().text_color(solid(icon_hex))),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(solid(TEXT_PRIMARY))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(solid(TEXT_MUTED))
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
                            .bg(solid(BG_BASE))
                            .border_1()
                            .border_color(solid(BORDER_SUBTLE))
                            .overflow_hidden()
                            .child(meta_row("Size", size_str))
                            .child(meta_row("Modified", modified_str))
                            .child(meta_row("Created", created_str))
                            .child(
                                v_flex()
                                    .p_2p5()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(solid(TEXT_MUTED))
                                            .child("Location"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(solid(TEXT_SECONDARY))
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
                                        .bg(solid(BG_ELEVATED))
                                        .border_1()
                                        .border_color(solid(BORDER_SUBTLE))
                                        .text_size(px(11.))
                                        .text_color(solid(TEXT_SECONDARY))
                                        .child(badge)
                                })),
                        )
                    })
                    // Quick Actions
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                Button::new("insp-open")
                                    .with_size(Size::Small)
                                    .primary()
                                    .label(if is_dir { "Open Folder" } else { "Open File" })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_location_or_file(open_loc.clone(), is_dir, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("insp-copy-path")
                                    .with_size(Size::Small)
                                    .label("Copy Path")
                                    .icon(LucideIcon::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_text_to_clipboard(&copy_path, "Path copied", cx);
                                    })),
                            )
                            .when_some(term_path, |el, p| {
                                el.child(
                                    Button::new("insp-terminal")
                                        .with_size(Size::Small)
                                        .label("Open Terminal Here")
                                        .icon(LucideIcon::Terminal)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_terminal_at(&p, cx);
                                        })),
                                )
                            })
                            .when_some(pin_path.filter(|_| is_dir), |el, p| {
                                el.child(
                                    Button::new("insp-pin")
                                        .with_size(Size::Small)
                                        .label("Pin to Sidebar")
                                        .icon(LucideIcon::Pin)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.pin_folder(p.clone(), cx);
                                        })),
                                )
                            })
                            .when_some(props_path, |el, p| {
                                el.child(
                                    Button::new("insp-props")
                                        .with_size(Size::Small)
                                        .label("Properties")
                                        .icon(LucideIcon::ExternalLink)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.show_properties_for(&p, cx);
                                        })),
                                )
                            }),
                    )
                    .into_any_element()
            }
        })
}

fn meta_row(label: &'static str, value: SharedString) -> impl IntoElement {
    h_flex()
        .w_full()
        .px_2p5()
        .py_1p5()
        .items_center()
        .justify_between()
        .gap_2()
        .border_b_1()
        .border_color(solid(BORDER_SUBTLE))
        .child(
            div()
                .text_size(px(11.))
                .text_color(solid(TEXT_MUTED))
                .child(label),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(solid(TEXT_PRIMARY))
                .child(value),
        )
}
