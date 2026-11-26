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
use texplorer_core::{AccentPreset, DriveKind, FileKind, KnownFolderKind, SizeUnit, ThemePreset};

pub const RADIUS_ROW: f32 = 6.;
pub const RADIUS_CONTROL: f32 = 4.;
pub const UI_REM_SIZE: f32 = 19.5;

/// Resolved flat surface, text, semantic, accent, and syntax color palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub is_dark: bool,
    pub bg_base: u32,
    pub bg_surface: u32,
    pub bg_elevated: u32,
    pub bg_hover: u32,
    pub border_subtle: u32,
    pub border: u32,
    pub resize_hover: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub accent_primary: u32,
    pub accent_hover: u32,
    pub accent_active: u32,
    pub accent_fg: u32,
    pub status_success: u32,
    pub status_warning: u32,
    pub status_error: u32,
    pub status_info: u32,
    pub status_violet: u32,
    pub media_video: u32,
    pub media_audio: u32,
    pub syntax_keyword: u32,
    pub syntax_string: u32,
    pub syntax_comment: u32,
    pub syntax_type: u32,
    pub syntax_number: u32,
    pub danger_hover: u32,
    pub danger_active: u32,
    pub danger_fg: u32,
}

impl Palette {
    /// Backdrop scrim opacity tuned per luminance mode so modals feel balanced in dark & light.
    #[inline]
    pub fn backdrop_alpha(self) -> f32 {
        if self.is_dark { 0.64 } else { 0.32 }
    }
}

/// Resolves the active `Palette` from the chosen `ThemePreset`, `AccentPreset`, and OS mode.
pub fn resolve_palette(theme: ThemePreset, accent: AccentPreset, system_is_dark: bool) -> Palette {
    let is_dark = theme.resolves_dark(system_is_dark);
    let accent_primary = accent.primary_hex_for(is_dark);
    let accent_hover = accent.hover_hex_for(is_dark);
    let accent_active = accent.active_hex_for(is_dark);
    let accent_fg = accent.text_on_accent_hex_for(is_dark);

    match (theme, is_dark) {
        // Obsidian Dark (Neutral Carbon Black) flat surface palette
        (ThemePreset::ObsidianDark, _) => Palette {
            is_dark: true,
            bg_base: 0x09090b,
            bg_surface: 0x111113,
            bg_elevated: 0x18181b,
            bg_hover: 0x202024,
            border_subtle: 0x27272a,
            border: 0x323236,
            resize_hover: 0x3f3f46,
            text_primary: 0xf4f4f5,
            text_secondary: 0xa1a1aa,
            text_muted: 0x71717a,
            accent_primary,
            accent_hover,
            accent_active,
            accent_fg,
            status_success: 0x34d399,
            status_warning: 0xfbbf24,
            status_error: 0xf87171,
            status_info: 0x60a5fa,
            status_violet: 0xa78bfa,
            media_video: 0xf472b6,
            media_audio: 0x38bdf8,
            syntax_keyword: 0xa78bfa,
            syntax_string: 0x34d399,
            syntax_comment: 0x71717a,
            syntax_type: 0x60a5fa,
            syntax_number: 0xfbbf24,
            danger_hover: 0xef4444,
            danger_active: 0xdc2626,
            danger_fg: 0x09090b,
        },
        // Fluent / WinUI 3 Files Dark palette (deep matte-black surface hierarchy with Files accents & semantics)
        (_, true) => Palette {
            is_dark: true,
            bg_base: 0x111111,
            bg_surface: 0x0b0b0b,
            bg_elevated: 0x161616,
            bg_hover: 0x1f1f1f,
            border_subtle: 0x222222,
            border: 0x2d2d2d,
            resize_hover: 0x3f3f3f,
            text_primary: 0xffffff,
            text_secondary: 0xcecece,
            text_muted: 0x949494,
            accent_primary,
            accent_hover,
            accent_active,
            accent_fg,
            status_success: 0x6ccb5f,
            status_warning: 0xfce100,
            status_error: 0xff99a4,
            status_info: 0x50c0ff,
            status_violet: 0xc586c0,
            media_video: 0xf472b6,
            media_audio: 0x50c0ff,
            syntax_keyword: 0xc586c0,
            syntax_string: 0xce9178,
            syntax_comment: 0x6a9955,
            syntax_type: 0x9cdcfe,
            syntax_number: 0xfce100,
            danger_hover: 0xf87171,
            danger_active: 0xef4444,
            danger_fg: 0x111111,
        },
        // Fluent / WinUI 3 Files Light palette (composited solid tokens from files-community/Files)
        (_, false) => Palette {
            is_dark: false,
            bg_base: 0xfafafa,
            bg_surface: 0xf2f2f2,
            bg_elevated: 0xffffff,
            bg_hover: 0xeaeaea,
            border_subtle: 0xe3e3e3,
            border: 0xd1d1d1,
            resize_hover: 0xb8b8b8,
            text_primary: 0x1a1a1a,
            text_secondary: 0x5f5f5f,
            text_muted: 0x858585,
            accent_primary,
            accent_hover,
            accent_active,
            accent_fg,
            status_success: 0x0f7b0f,
            status_warning: 0x9d5d00,
            status_error: 0xc42b1c,
            status_info: 0x0070cb,
            status_violet: 0xaf00db,
            media_video: 0xc2185b,
            media_audio: 0x0070cb,
            syntax_keyword: 0xaf00db,
            syntax_string: 0xa31515,
            syntax_comment: 0x008000,
            syntax_type: 0x0451a5,
            syntax_number: 0x9d5d00,
            danger_hover: 0xa82315,
            danger_active: 0x8d1d12,
            danger_fg: 0xffffff,
        },
    }
}

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
pub fn crisp_resize_handle_appearance(palette: Palette) -> ResizeHandleRenderer {
    Rc::new(move |handle: &ResizeHandleContext, _, _| {
        let axis = handle.axis();
        let (color, thick) = match handle.state() {
            ResizeHandleState::Idle => (solid(palette.border_subtle), false),
            ResizeHandleState::Hovered => (solid(palette.resize_hover), true),
            ResizeHandleState::Pressed | ResizeHandleState::Dragging => {
                (solid(palette.accent_primary), true)
            }
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

/// Applies the active flat theme palette (Files Dark, Files Light, or Obsidian Dark)
/// with zero shadows and zero focus rings.
pub fn apply_theme(p: Palette, cx: &mut App) {
    Theme::change(
        if p.is_dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.shadow = false;
        theme.focus_ring = false;
        theme.radius = px(RADIUS_CONTROL);
        theme.radius_lg = px(RADIUS_ROW);
        theme.font_size = px(15.5);
        theme.mono_font_size = px(13.5);

        let bg_base = solid(p.bg_base);
        let bg_surface = solid(p.bg_surface);
        let bg_elevated = solid(p.bg_elevated);
        let bg_hover = solid(p.bg_hover);
        let border = solid(p.border_subtle);
        let fg = solid(p.text_primary);
        let fg_muted = solid(p.text_secondary);

        let primary = solid(p.accent_primary);
        let primary_hover = solid(p.accent_hover);
        let primary_active = solid(p.accent_active);
        let primary_fg = solid(p.accent_fg);

        let danger = solid(p.status_error);
        let danger_hover = solid(p.danger_hover);
        let danger_active = solid(p.danger_active);
        let danger_fg = solid(p.danger_fg);

        let c = &mut theme.colors;
        c.background = bg_base;
        c.foreground = fg;
        c.border = border;
        c.input = border;
        c.caret = primary;
        c.ring = primary;
        c.selection = soft_tint(p.accent_primary, if p.is_dark { 0.22 } else { 0.16 });
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
        c.danger_foreground = danger_fg;
        c.button_danger = danger;
        c.button_danger_hover = danger_hover;
        c.button_danger_active = danger_active;
        c.button_danger_foreground = danger_fg;

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
        c.scrollbar_thumb = solid(p.border);
        c.scrollbar_thumb_hover = solid(p.resize_hover);

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
pub fn file_kind_visual(kind: FileKind, p: Palette) -> (LucideIcon, u32) {
    match kind {
        FileKind::Folder => (LucideIcon::Folder, p.accent_primary),
        FileKind::Image => (LucideIcon::FileImage, p.status_violet),
        FileKind::Video => (LucideIcon::Film, p.media_video),
        FileKind::Audio => (LucideIcon::Music, p.media_audio),
        FileKind::Code => (LucideIcon::FileCode, p.status_info),
        FileKind::Document => (LucideIcon::FileText, p.text_primary),
        FileKind::Archive => (LucideIcon::FileArchive, p.status_warning),
        FileKind::Executable => (LucideIcon::Package, p.status_success),
        FileKind::Symlink => (LucideIcon::Link, p.status_info),
        FileKind::Other => (LucideIcon::File, p.text_secondary),
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

pub fn usage_bar_color(fraction: f32, p: Palette) -> u32 {
    if fraction >= 0.92 {
        p.status_error
    } else if fraction >= 0.80 {
        p.status_warning
    } else {
        p.accent_primary
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
