use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::{
    Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, SharedString,
    UniformListScrollHandle,
    assets::IconName as LucideIcon,
    component::{
        Icon, Sizable as _, Size, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    div, prelude::*, px, uniform_list,
};
use texplorer_core::{FileEntry, SearchScope};
use texplorer_mft::IndexServiceInstallState;

use crate::theme::{
    RADIUS_CONTROL, RADIUS_ROW, file_kind_visual, format_size, format_timestamp, full_hit_input,
    soft_tint, solid,
};
use crate::ui::workspace::Workspace;

/// Interactive state for the `Ctrl+K` Global Search overlay.
pub struct GlobalSearchState {
    pub input: Entity<InputState>,
    pub scroll_handle: UniformListScrollHandle,
    pub scope: SearchScope,
    pub active_folder: Option<PathBuf>,
    pub query_text: String,
    pub results: Vec<FileEntry>,
    pub total_matches: usize,
    pub elapsed_ms: u64,
    pub from_mft: bool,
    pub truncated: bool,
    pub searching: bool,
    pub selected_idx: usize,
    pub generation: u64,
    pub cancel_flag: Arc<AtomicBool>,
}

/// Renders the Everything-style `Ctrl+K` Global Search modal with scope switching,
/// engine indicator (`MFT Instant` vs `Parallel Fallback`), and virtualized hit rows.
pub fn render_global_search_modal(
    ws: &Workspace,
    state: &GlobalSearchState,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let p = ws.palette();
    let is_global = matches!(state.scope, SearchScope::Global);
    let has_folder_scope = state.active_folder.is_some();
    let result_count = state.results.len();
    let has_query = !state.query_text.trim().is_empty();
    let searching = state.searching;
    let service_running = ws.service_install_state == IndexServiceInstallState::Running
        && ws.settings.use_mft_service;
    let using_mft = if has_query && !searching && result_count > 0 {
        state.from_mft
    } else {
        service_running
    };

    let status_summary: SharedString = if !has_query {
        "Syntax: ext:rs;toml  •  size:>10mb  •  dir:  •  file:  •  !exclude  •  /regex/".into()
    } else if searching {
        "Searching...".into()
    } else if state.truncated {
        format!(
            "Showing {} of {} matches ({} ms)",
            result_count, state.total_matches, state.elapsed_ms
        )
        .into()
    } else {
        format!("{} matches ({} ms)", result_count, state.elapsed_ms).into()
    };

    let folder_scope_label: SharedString = state
        .active_folder
        .as_ref()
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Current Folder".to_string())
        .into();

    div()
        .id("global-search-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(76.))
        .occlude()
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
            let key = ev.keystroke.key.as_str();
            match key {
                "up" => {
                    this.move_global_search_selection(-1, cx);
                    cx.stop_propagation();
                }
                "down" => {
                    this.move_global_search_selection(1, cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    this.close_global_search(window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
        .child(
            div()
                .id("global-search-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, p.backdrop_alpha()))
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_global_search(window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_global_search(window, cx);
                    }),
                ),
        )
        .child(
            v_flex()
                .id("global-search-card")
                .relative()
                .w(px(760.))
                .h(px(540.))
                .bg(solid(p.bg_surface))
                .border_1()
                .border_color(solid(p.border))
                .rounded(px(RADIUS_ROW))
                .overflow_hidden()
                .occlude()
                // Search Input Bar
                .child(
                    h_flex()
                        .w_full()
                        .h(px(48.))
                        .px_3p5()
                        .gap_2p5()
                        .flex_shrink_0()
                        .items_center()
                        .bg(solid(p.bg_elevated))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            Icon::new(LucideIcon::Search)
                                .small()
                                .text_color(solid(p.accent_primary)),
                        )
                        .child(
                            full_hit_input(
                                "global-search-input-hit",
                                &state.input,
                                Input::new(&state.input)
                                    .with_size(Size::Small)
                                    .appearance(false),
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        // Scope Switcher
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_1()
                                .p_0p5()
                                .rounded(px(RADIUS_CONTROL))
                                .bg(solid(p.bg_base))
                                .border_1()
                                .border_color(solid(p.border_subtle))
                                .child(
                                    div()
                                        .id("gs-scope-global")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(3.))
                                        .cursor_pointer()
                                        .text_size(px(13.))
                                        .font_medium()
                                        .bg(if is_global {
                                            soft_tint(p.accent_primary, 0.16)
                                        } else {
                                            gpui_kit::transparent_black()
                                        })
                                        .text_color(if is_global {
                                            solid(p.accent_primary)
                                        } else {
                                            solid(p.text_muted)
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_global_search_scope(SearchScope::Global, cx);
                                        }))
                                        .child("Everywhere"),
                                )
                                .when(has_folder_scope, |el| {
                                    let active_dir = state.active_folder.clone().unwrap();
                                    el.child(
                                        div()
                                            .id("gs-scope-folder")
                                            .max_w(px(130.))
                                            .truncate()
                                            .px_2()
                                            .py_0p5()
                                            .rounded(px(3.))
                                            .cursor_pointer()
                                            .text_size(px(13.))
                                            .font_medium()
                                            .bg(if !is_global {
                                                soft_tint(p.accent_primary, 0.16)
                                            } else {
                                                gpui_kit::transparent_black()
                                            })
                                            .text_color(if !is_global {
                                                solid(p.accent_primary)
                                            } else {
                                                solid(p.text_muted)
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_global_search_scope(
                                                    SearchScope::Folder(active_dir.clone()),
                                                    cx,
                                                );
                                            }))
                                            .child(folder_scope_label),
                                    )
                                }),
                        )
                        .child(
                            Button::new("gs-close")
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(LucideIcon::X)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_global_search(window, cx);
                                })),
                        ),
                )
                // Status & Engine strip
                .child(
                    h_flex()
                        .w_full()
                        .h(px(34.))
                        .px_3p5()
                        .gap_2()
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .bg(solid(p.bg_surface))
                        .border_b_1()
                        .border_color(solid(p.border_subtle))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .flex_shrink_0()
                                        .px_1p5()
                                        .py_0p5()
                                        .gap_1()
                                        .items_center()
                                        .rounded(px(3.))
                                        .bg(if using_mft {
                                            soft_tint(p.status_success, 0.14)
                                        } else {
                                            solid(p.bg_elevated)
                                        })
                                        .child(
                                            div()
                                                .w(px(6.))
                                                .h(px(6.))
                                                .rounded(px(999.))
                                                .bg(solid(if using_mft {
                                                    p.status_success
                                                } else {
                                                    p.text_muted
                                                })),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_semibold()
                                                .text_color(solid(if using_mft {
                                                    p.status_success
                                                } else {
                                                    p.text_secondary
                                                }))
                                                .child(if using_mft {
                                                    "MFT Instant"
                                                } else {
                                                    "Parallel Fallback"
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .text_color(solid(p.text_muted))
                                        .child(status_summary),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_1p5()
                                .when(!service_running, |el| {
                                    el.child(
                                        div()
                                            .id("gs-setup-mft")
                                            .px_2()
                                            .py_0p5()
                                            .rounded(px(RADIUS_CONTROL))
                                            .cursor_pointer()
                                            .text_size(px(12.))
                                            .font_medium()
                                            .text_color(solid(p.accent_primary))
                                            .hover(move |s| {
                                                s.bg(soft_tint(p.accent_primary, 0.12))
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.close_global_search(window, cx);
                                                this.open_settings_modal(window, cx);
                                            }))
                                            .child("Setup MFT Service"),
                                    )
                                })
                                .when(has_query, |el| {
                                    el.child(
                                        div()
                                            .id("gs-open-tab")
                                            .px_2()
                                            .py_0p5()
                                            .rounded(px(RADIUS_CONTROL))
                                            .cursor_pointer()
                                            .bg(solid(p.bg_elevated))
                                            .border_1()
                                            .border_color(solid(p.border_subtle))
                                            .text_size(px(12.))
                                            .font_medium()
                                            .text_color(solid(p.text_primary))
                                            .hover(move |s| s.bg(solid(p.bg_hover)))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_global_search_in_tab(window, cx);
                                            }))
                                            .child("Open in Tab"),
                                    )
                                }),
                        ),
                )
                // Virtualized Results Area
                .child(div().flex_1().min_h_0().w_full().bg(solid(p.bg_base)).child(
                    if !has_query {
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .gap_1p5()
                            .px_6()
                            .child(
                                Icon::new(LucideIcon::Search)
                                    .large()
                                    .text_color(solid(p.text_muted)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(solid(p.text_primary))
                                    .child("Instant Global File Search"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(solid(p.text_muted))
                                    .child("Type any filename or combine filters like ext:rs, size:>50mb, dir:, !target, or /regex/."),
                            )
                            .into_any_element()
                    } else if result_count == 0 && !searching {
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(solid(p.text_primary))
                                    .child("No matches found"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(solid(p.text_muted))
                                    .child("Try broadening your search terms or switching scope to Everywhere."),
                            )
                            .into_any_element()
                    } else {
                        uniform_list(
                            "global-search-results-list",
                            result_count,
                            cx.processor(|this, range: Range<usize>, _window, cx| {
                                render_search_result_rows(this, range, cx)
                            }),
                        )
                        .track_scroll(&state.scroll_handle)
                        .size_full()
                        .into_any_element()
                    },
                ))
                // Footer Shortcut Strip
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
                                .child("↑ / ↓ Navigate   •   Enter Open   •   Shift+Enter Open in Tab   •   Esc Close"),
                        ),
                ),
        )
}

fn render_search_result_rows(
    ws: &mut Workspace,
    range: Range<usize>,
    cx: &mut Context<Workspace>,
) -> Vec<gpui_kit::AnyElement> {
    let p = ws.palette();
    let unit = ws.settings.size_unit;
    let Some(state) = ws.global_search.as_ref() else {
        return Vec::new();
    };
    let selected_idx = state.selected_idx;

    range
        .filter_map(|idx| {
            let entry = state.results.get(idx)?;
            let selected = idx == selected_idx;
            let (icon, icon_hex) = file_kind_visual(entry.kind, p);
            let name: SharedString = entry.name.clone().into();
            let parent_dir: SharedString = entry
                .location
                .as_local_path()
                .and_then(|path| path.parent())
                .map(|par| par.display().to_string())
                .unwrap_or_default()
                .into();
            let size_label: SharedString = match entry.size {
                Some(bytes) => format_size(bytes, unit).into(),
                None => "Folder".into(),
            };
            let mod_label: SharedString = format_timestamp(entry.modified_secs).into();
            let loc_open = entry.location.clone();
            let is_dir = entry.is_dir();
            let reveal_path = entry.location.as_local_path().map(|path| path.to_path_buf());

            Some(
                h_flex()
                    .id(("gs-row", idx))
                    .w_full()
                    .h(px(50.))
                    .px_3()
                    .gap_2p5()
                    .items_center()
                    .cursor_pointer()
                    .bg(if selected {
                        soft_tint(p.accent_primary, if p.is_dark { 0.16 } else { 0.12 })
                    } else {
                        solid(p.bg_base)
                    })
                    .border_b_1()
                    .border_color(if selected {
                        solid(p.accent_primary)
                    } else {
                        solid(p.border_subtle)
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
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            if let Some(gs) = this.global_search.as_mut() {
                                gs.selected_idx = idx;
                            }
                            if ev.click_count >= 2 {
                                this.close_global_search(window, cx);
                                this.open_location_or_file(loc_open.clone(), is_dir, window, cx);
                            } else {
                                cx.notify();
                            }
                        }),
                    )
                    .child(Icon::new(icon).small().text_color(solid(icon_hex)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(solid(p.text_primary))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(solid(p.text_muted))
                                    .child(parent_dir),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_shrink_0()
                            .items_end()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(solid(p.text_secondary))
                                    .child(size_label),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(solid(p.text_muted))
                                    .child(mod_label),
                            ),
                    )
                    .when_some(reveal_path, |el, path| {
                        el.child(
                            div()
                                .id(("gs-reveal", idx))
                                .w(px(24.))
                                .h(px(24.))
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .rounded(px(RADIUS_CONTROL))
                                .text_color(solid(p.text_muted))
                                .hover(move |s| {
                                    s.bg(solid(p.bg_elevated))
                                        .text_color(solid(p.accent_primary))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        window.prevent_default();
                                        this.reveal_search_hit_in_pane(&path, window, cx);
                                    }),
                                )
                                .child(Icon::new(LucideIcon::FolderOpen).xsmall()),
                        )
                    })
                    .into_any_element(),
            )
        })
        .collect()
}
