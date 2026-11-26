use std::rc::Rc;
use std::sync::LazyLock;

use gpui_kit::{
    App, Axis, ElementId, Entity, Hsla, MouseButton,
    assets::IconName as LucideIcon,
    base::{ResizeHandleContext, ResizeHandleRenderer, ResizeHandleState},
    component::{
        Theme, ThemeMode,
        input::{Input, InputState},
    },
    div, hsla,
    prelude::*,
    px, rgb,
};
use texplorer_core::{AccentPreset, DriveKind, FileKind, KnownFolderKind, SizeUnit};

// Obsidian Dark (Neutral Carbon Black) strict flat surface palette
pub const BG_BASE: u32 = 0x09090b;
pub const BG_SURFACE: u32 = 0x111113;
pub const BG_ELEVATED: u32 = 0x18181b;
pub const BG_HOVER: u32 = 0x202024;
pub const BORDER_SUBTLE: u32 = 0x27272a;
pub const BORDER: u32 = 0x323236;

pub const TEXT_PRIMARY: u32 = 0xf4f4f5;
pub const TEXT_SECONDARY: u32 = 0xa1a1aa;
pub const TEXT_MUTED: u32 = 0x71717a;

// Flat semantic status colors
pub const STATUS_SUCCESS: u32 = 0x34d399;
pub const STATUS_WARNING: u32 = 0xfbbf24;
pub const STATUS_ERROR: u32 = 0xf87171;
pub const STATUS_INFO: u32 = 0x60a5fa;
pub const STATUS_VIOLET: u32 = 0xa78bfa;

pub const RADIUS_ROW: f32 = 6.;
pub const RADIUS_CONTROL: f32 = 4.;
pub const UI_REM_SIZE: f32 = 17.0;

#[inline]
pub fn solid(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Returns a translucent flat tint derived from a hex color for soft badges and selection fills.
pub fn soft_tint(hex: u32, alpha: f32) -> Hsla {
    let base: Hsla = rgb(hex).into();
    hsla(base.h, base.s, base.l, alpha)
}

/// Crisp 1px flat panel cut divider at rest; expands cleanly to 2px on hover or drag.
pub fn crisp_resize_handle_appearance(accent: AccentPreset) -> ResizeHandleRenderer {
    let accent_hex = accent.primary_hex();
    Rc::new(move |handle: &ResizeHandleContext, _, _| {
        let axis = handle.axis();
        let (color, thick) = match handle.state() {
            ResizeHandleState::Idle => (solid(BORDER_SUBTLE), false),
            ResizeHandleState::Hovered => (solid(0x3f3f46), true),
            ResizeHandleState::Pressed | ResizeHandleState::Dragging => (solid(accent_hex), true),
        };

        Some(
            div()
                .flex_none()
                .bg(color)
                .map(|d| match (axis, thick) {
                    (Axis::Horizontal, false) => d.w(px(1.)).h_full(),
                    (Axis::Horizontal, true) => d.w(px(2.)).h_full(),
                    (Axis::Vertical, false) => d.h(px(1.)).w_full(),
                    (Axis::Vertical, true) => d.h(px(2.)).w_full(),
                })
                .into_any_element(),
        )
    })
}

/// Applies the Obsidian Dark strict flat theme with zero shadows and zero focus rings.
pub fn apply_obsidian_theme(accent: AccentPreset, cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.shadow = false;
        theme.focus_ring = false;
        theme.radius = px(RADIUS_CONTROL);
        theme.radius_lg = px(RADIUS_ROW);
        theme.font_size = px(14.);
        theme.mono_font_size = px(12.);

        let bg_base = solid(BG_BASE);
        let bg_surface = solid(BG_SURFACE);
        let bg_elevated = solid(BG_ELEVATED);
        let bg_hover = solid(BG_HOVER);
        let border = solid(BORDER_SUBTLE);
        let fg = solid(TEXT_PRIMARY);
        let fg_muted = solid(TEXT_SECONDARY);

        let primary = solid(accent.primary_hex());
        let primary_hover = solid(accent.hover_hex());
        let primary_active = solid(accent.active_hex());
        let primary_fg = solid(accent.text_on_accent_hex());

        let danger = solid(STATUS_ERROR);
        let danger_hover = solid(0xef4444);
        let danger_active = solid(0xdc2626);

        let c = &mut theme.colors;
        c.background = bg_base;
        c.foreground = fg;
        c.border = border;
        c.input = border;
        c.caret = primary;
        c.ring = primary;
        c.selection = soft_tint(accent.primary_hex(), 0.22);
        c.drag_border = primary;
        c.drop_target = bg_hover;

        c.muted = bg_elevated;
        c.muted_foreground = fg_muted;
        c.accent = bg_hover;
        c.accent_foreground = fg;

        c.primary = primary;
        c.primary_hover = primary_hover;
        c.primary_active = primary_active;
        c.primary_foreground = primary_fg;

        c.secondary = bg_elevated;
        c.secondary_hover = bg_hover;
        c.secondary_active = bg_hover;
        c.secondary_foreground = fg;

        c.button = bg_elevated;
        c.button_hover = bg_hover;
        c.button_active = bg_hover;
        c.button_foreground = fg;

        c.button_primary = primary;
        c.button_primary_hover = primary_hover;
        c.button_primary_active = primary_active;
        c.button_primary_foreground = primary_fg;

        c.button_secondary = bg_elevated;
        c.button_secondary_hover = bg_hover;
        c.button_secondary_active = bg_hover;
        c.button_secondary_foreground = fg;

        c.danger = danger;
        c.danger_hover = danger_hover;
        c.danger_active = danger_active;
        c.danger_foreground = solid(BG_BASE);
        c.button_danger = danger;
        c.button_danger_hover = danger_hover;
        c.button_danger_active = danger_active;
        c.button_danger_foreground = solid(BG_BASE);

        c.popover = bg_elevated;
        c.popover_foreground = fg;

        c.sidebar = bg_surface;
        c.sidebar_foreground = fg;
        c.sidebar_border = border;
        c.sidebar_accent = bg_elevated;
        c.sidebar_accent_foreground = fg;
        c.sidebar_primary = primary;
        c.sidebar_primary_foreground = primary_fg;

        c.list = bg_surface;
        c.list_even = bg_surface;
        c.list_head = bg_elevated;
        c.list_hover = bg_elevated;
        c.list_active = bg_elevated;
        c.list_active_border = primary;
        c.list_highlight_foreground = primary;

        c.tab_bar = bg_elevated;
        c.tab_bar_segmented = bg_surface;
        c.tab = bg_elevated;
        c.tab_foreground = fg_muted;
        c.tab_active = bg_base;
        c.tab_active_foreground = fg;

        c.title_bar = bg_elevated;
        c.title_bar_border = border;
        c.status_bar = bg_surface;
        c.status_bar_border = border;

        c.scrollbar = bg_base;
        c.scrollbar_thumb = solid(BORDER);
        c.scrollbar_thumb_hover = solid(0x3f3f46);

        c.switch = bg_hover;
        c.switch_thumb = fg;
        c.link = primary;
        c.link_hover = primary_hover;
        c.link_active = primary_active;
    });
}

/// Wraps a borderless single-line `Input` so clicks anywhere in its vertical height focus it.
pub fn full_hit_input(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    input: Input,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let target = state.clone();
    div()
        .id(id)
        .h_full()
        .flex()
        .items_center()
        .cursor_text()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if !window.default_prevented() {
                target.update(cx, |s, cx| s.focus(window, cx));
                window.prevent_default();
            }
        })
        .child(input.h_full().py_0())
}

/// Returns the semantic Lucide icon and flat accent color for a file category.
pub fn file_kind_visual(kind: FileKind, accent: AccentPreset) -> (LucideIcon, u32) {
    match kind {
        FileKind::Folder => (LucideIcon::Folder, accent.primary_hex()),
        FileKind::Image => (LucideIcon::FileImage, STATUS_VIOLET),
        FileKind::Video => (LucideIcon::Film, 0xf472b6),
        FileKind::Audio => (LucideIcon::Music, 0x38bdf8),
        FileKind::Code => (LucideIcon::FileCode, STATUS_INFO),
        FileKind::Document => (LucideIcon::FileText, TEXT_PRIMARY),
        FileKind::Archive => (LucideIcon::FileArchive, STATUS_WARNING),
        FileKind::Executable => (LucideIcon::Package, STATUS_SUCCESS),
        FileKind::Symlink => (LucideIcon::Link, STATUS_INFO),
        FileKind::Other => (LucideIcon::File, TEXT_SECONDARY),
    }
}

pub fn known_folder_icon(kind: KnownFolderKind) -> LucideIcon {
    match kind {
        KnownFolderKind::Desktop => LucideIcon::Monitor,
        KnownFolderKind::Downloads => LucideIcon::Download,
        KnownFolderKind::Documents => LucideIcon::FileText,
        KnownFolderKind::Pictures => LucideIcon::Image,
        KnownFolderKind::Music => LucideIcon::Music,
        KnownFolderKind::Videos => LucideIcon::Film,
        KnownFolderKind::UserHome => LucideIcon::House,
    }
}

pub fn drive_kind_icon(kind: DriveKind) -> LucideIcon {
    match kind {
        DriveKind::Fixed | DriveKind::RamDisk | DriveKind::Unknown => LucideIcon::HardDrive,
        DriveKind::Removable => LucideIcon::Usb,
        DriveKind::Network => LucideIcon::Network,
        DriveKind::Optical => LucideIcon::Disc,
        DriveKind::Wsl => LucideIcon::Terminal,
    }
}

pub fn usage_bar_color(fraction: f32, accent: AccentPreset) -> u32 {
    if fraction >= 0.92 {
        STATUS_ERROR
    } else if fraction >= 0.80 {
        STATUS_WARNING
    } else {
        accent.primary_hex()
    }
}

pub fn format_size(bytes: u64, unit: SizeUnit) -> String {
    let (base, suffixes): (f64, [&str; 6]) = match unit {
        SizeUnit::Binary => (1024.0, ["B", "KiB", "MiB", "GiB", "TiB", "PiB"]),
        SizeUnit::Decimal => (1000.0, ["B", "KB", "MB", "GB", "TB", "PB"]),
    };
    if bytes < base as u64 {
        return format!("{bytes} B");
    }
    let mut val = bytes as f64;
    let mut idx = 0usize;
    while val >= base && idx + 1 < suffixes.len() {
        val /= base;
        idx += 1;
    }
    if val >= 100.0 {
        format!("{val:.0} {}", suffixes[idx])
    } else if val >= 10.0 {
        format!("{val:.1} {}", suffixes[idx])
    } else {
        format!("{val:.2} {}", suffixes[idx])
    }
}

static LOCAL_UTC_OFFSET_SECS: LazyLock<i64> = LazyLock::new(|| {
    i64::from(chrono::Local::now().offset().local_minus_utc())
});

/// Fast, syscall-free timestamp formatter (`YYYY-MM-DD HH:MM`) safe inside virtualized frames.
pub fn format_timestamp(unix_secs: i64) -> String {
    if unix_secs <= 0 {
        return "—".to_string();
    }
    let local_secs = unix_secs.saturating_add(*LOCAL_UTC_OFFSET_SECS);
    let days = local_secs.div_euclid(86_400);
    let secs_of_day = local_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnant's civil-from-days algorithm converting days since 1970-01-01 to `(Y, M, D)`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year, m as u32, d as u32)
}
