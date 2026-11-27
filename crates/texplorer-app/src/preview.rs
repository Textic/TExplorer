//! Thumbnail cache and asynchronous rich preview decoders (images, syntax-highlighted code,
//! Markdown, folder summaries, and native Windows Shell thumbnails).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Cursor, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use gpui_kit::{HighlightStyle, RenderImage};
use image::{
    AnimationDecoder as _, Delay, Frame, ImageDecoder as _, ImageFormat, RgbaImage,
    codecs::{gif::GifDecoder, webp::WebPDecoder},
    imageops::FilterType,
};
use smallvec::{SmallVec, smallvec};
use texplorer_core::{EntryFlags, FileEntry, FileKind, IconStyle};
use texplorer_platform::{
    RawBgraImage, RenderedPdfPage, ShellWorker, SystemIconRequest, render_pdf_page_from_bytes,
    render_pdf_page_from_path,
};

pub const PDF_PREVIEW_MAX_DIM: u32 = 1400;

use crate::theme::{Palette, solid};

/// Cache key uniquely identifying a rendered thumbnail by path, timestamp, pixel dimension,
/// and whether Shell icon fallback was permitted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub path: PathBuf,
    pub modified_secs: i64,
    pub size_px: i32,
    pub icon_fallback: bool,
}

/// Cache key uniquely identifying a native Windows system icon.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SystemIconKey {
    /// Per-file or special/custom folder icon (`.exe`, `.lnk`, `.ico`, `.url`, `desktop.ini`).
    Path {
        path: PathBuf,
        modified_secs: i64,
        size_px: i32,
    },
    /// Extension-shared Windows file-type icon (e.g. `"dll"`, `"pdf"`, `""`).
    Extension { ext: String, size_px: i32 },
    /// Standard generic Windows folder icon.
    GenericFolder { size_px: i32 },
}

/// Extensions whose Windows Shell icon can differ per file rather than being shared by file type.
const PER_FILE_ICON_EXTENSIONS: &[&str] = &[
    "exe",
    "lnk",
    "ico",
    "cur",
    "ani",
    "url",
    "scr",
    "cpl",
    "msc",
    "appref-ms",
    "pif",
    "library-ms",
];

/// Determines the deduplicated cache key and platform lookup request for `entry` under
/// `icon_style`, or returns `None` when TExplorer's built-in Lucide icon should be rendered.
pub fn resolve_system_icon_target(
    entry: &FileEntry,
    icon_style: IconStyle,
    size_px: i32,
) -> Option<(SystemIconKey, SystemIconRequest)> {
    if entry.is_dir() {
        if !icon_style.uses_system_folder_icons() {
            return None;
        }
        // Windows only reads `desktop.ini` custom/special folder icons (Desktop, Downloads,
        // Pictures, OneDrive, custom folders) on directories marked READONLY or SYSTEM.
        // All regular folders share a single cached generic Windows folder icon.
        let may_have_custom_icon = entry.flags.intersects(
            EntryFlags::IS_READONLY | EntryFlags::IS_SYSTEM | EntryFlags::IS_SYMLINK,
        );
        if may_have_custom_icon
            && let Some(local) = entry.location.as_local_path()
        {
            let path = local.to_path_buf();
            return Some((
                SystemIconKey::Path {
                    path: path.clone(),
                    modified_secs: entry.modified_secs,
                    size_px,
                },
                SystemIconRequest::Path(path),
            ));
        }
        return Some((
            SystemIconKey::GenericFolder { size_px },
            SystemIconRequest::GenericFolder,
        ));
    }

    if !icon_style.uses_system_file_icons() {
        return None;
    }

    let ext_lower = entry.extension.trim().to_ascii_lowercase();
    let per_file = PER_FILE_ICON_EXTENSIONS.contains(&ext_lower.as_str())
        || entry.flags.contains(EntryFlags::IS_SYMLINK);
    if per_file
        && let Some(local) = entry.location.as_local_path()
    {
        let path = local.to_path_buf();
        return Some((
            SystemIconKey::Path {
                path: path.clone(),
                modified_secs: entry.modified_secs,
                size_px,
            },
            SystemIconRequest::Path(path),
        ));
    }

    Some((
        SystemIconKey::Extension {
            ext: ext_lower.clone(),
            size_px,
        },
        SystemIconRequest::Extension(ext_lower),
    ))
}

/// Bounded in-memory cache of native Windows system icons (`RenderImage`s) preventing duplicate
/// COM STA lookups and returning evicted GPU images for clean disposal.
pub struct SystemIconCache {
    max_entries: usize,
    entries: HashMap<SystemIconKey, Option<Arc<RenderImage>>>,
    order: VecDeque<SystemIconKey>,
    in_flight: HashSet<SystemIconKey>,
}

impl Default for SystemIconCache {
    fn default() -> Self {
        Self::new(600)
    }
}

impl SystemIconCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: HashMap::with_capacity(max_entries),
            order: VecDeque::with_capacity(max_entries),
            in_flight: HashSet::new(),
        }
    }

    /// Returns `Some(Some(img))` if cached, `Some(None)` if lookup failed, or `None` if uncached.
    pub fn get(&self, key: &SystemIconKey) -> Option<&Option<Arc<RenderImage>>> {
        self.entries.get(key)
    }

    /// Marks `key` as in-flight if neither cached nor already requested.
    pub fn try_mark_in_flight(&mut self, key: SystemIconKey) -> bool {
        if self.entries.contains_key(&key) || self.in_flight.contains(&key) {
            return false;
        }
        self.in_flight.insert(key);
        true
    }

    /// Stores a completed icon lookup and returns any evicted `RenderImage` so the caller can
    /// release its GPU texture via `window.drop_image`.
    pub fn insert(
        &mut self,
        key: SystemIconKey,
        image: Option<Arc<RenderImage>>,
    ) -> Option<Arc<RenderImage>> {
        self.in_flight.remove(&key);
        if let Some(slot) = self.entries.get_mut(&key) {
            return std::mem::replace(slot, image);
        }
        let mut evicted_img = None;
        while self.entries.len() >= self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(old_val) = self.entries.remove(&oldest)
                    && old_val.is_some()
                {
                    evicted_img = old_val;
                }
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, image);
        evicted_img
    }

    /// Clears all cached and in-flight system icons and returns previously cached GPU images
    /// so the caller can dispose them cleanly via `window.drop_image`.
    pub fn clear_and_drain_images(&mut self) -> Vec<Arc<RenderImage>> {
        self.order.clear();
        self.in_flight.clear();
        self.entries
            .drain()
            .filter_map(|(_, img)| img)
            .collect()
    }
}

/// Bounded in-memory LRU-ish thumbnail store preventing duplicate COM Shell lookups.
pub struct ThumbnailCache {
    max_entries: usize,
    entries: HashMap<ThumbnailKey, Option<Arc<RenderImage>>>,
    order: VecDeque<ThumbnailKey>,
    in_flight: HashSet<ThumbnailKey>,
}

impl Default for ThumbnailCache {
    fn default() -> Self {
        Self::new(400)
    }
}

impl ThumbnailCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: HashMap::with_capacity(max_entries),
            order: VecDeque::with_capacity(max_entries),
            in_flight: HashSet::new(),
        }
    }

    /// Returns `Some(Some(img))` if cached with image, `Some(None)` if lookup returned no image,
    /// or `None` if not yet fetched.
    pub fn get(&self, key: &ThumbnailKey) -> Option<&Option<Arc<RenderImage>>> {
        self.entries.get(key)
    }

    /// Marks `key` as in-flight if neither cached nor already requested.
    pub fn try_mark_in_flight(&mut self, key: ThumbnailKey) -> bool {
        if self.entries.contains_key(&key) || self.in_flight.contains(&key) {
            return false;
        }
        self.in_flight.insert(key);
        true
    }

    /// Stores a completed lookup result and evicts the oldest entry when full.
    pub fn insert(&mut self, key: ThumbnailKey, image: Option<Arc<RenderImage>>) {
        self.in_flight.remove(&key);
        if let Some(slot) = self.entries.get_mut(&key) {
            *slot = image;
            return;
        }
        while self.entries.len() >= self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, image);
    }

    /// Clears cached and in-flight entries that allowed Shell icon fallback (preserving pure
    /// content thumbnails) and returns their GPU images for clean disposal.
    pub fn clear_icon_fallbacks_and_drain_images(&mut self) -> Vec<Arc<RenderImage>> {
        self.order.retain(|k| !k.icon_fallback);
        self.in_flight.retain(|k| !k.icon_fallback);
        let fallback_keys: Vec<ThumbnailKey> = self
            .entries
            .keys()
            .filter(|k| k.icon_fallback)
            .cloned()
            .collect();
        let mut drained = Vec::with_capacity(fallback_keys.len());
        for key in fallback_keys {
            if let Some(Some(img)) = self.entries.remove(&key) {
                drained.push(img);
            }
        }
        drained
    }
}

/// Converts raw top-down BGRA bytes (e.g. from Windows `IShellItemImageFactory`) into a GPUI `RenderImage`.
pub fn raw_bgra_to_render_image(raw: RawBgraImage) -> Option<Arc<RenderImage>> {
    if raw.width == 0 || raw.height == 0 {
        return None;
    }
    let buf = RgbaImage::from_raw(raw.width, raw.height, raw.bgra)?;
    let frame = Frame::new(buf);
    Some(Arc::new(RenderImage::new(smallvec![frame])))
}

const MAX_ANIMATED_FRAMES: usize = 240;
const MAX_ANIMATED_DIM: u32 = 800;
const MAX_ANIMATED_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Normalizes GIF/WebP frame delays matching web browser behavior:
/// delays under 20 ms (such as 0 ms or 10 ms) default to 100 ms so animations never spin uncontrollably.
fn normalize_frame_delay(delay: Delay) -> Delay {
    let (numer, denom) = delay.numer_denom_ms();
    let ms = numer.checked_div(denom).unwrap_or(100);
    if ms < 20 {
        Delay::from_numer_denom_ms(100, 1)
    } else {
        delay
    }
}

/// Resizes an animation frame proportionally if larger than `max_dim` and swizzles RGBA -> BGRA for GPUI.
fn prepare_animated_frame(frame: Frame, max_dim: u32) -> Frame {
    let delay = normalize_frame_delay(frame.delay());
    let mut rgba = frame.into_buffer();
    let w = rgba.width();
    let h = rgba.height();
    if (w > max_dim || h > max_dim) && w > 0 && h > 0 {
        let scale = (max_dim as f32 / w as f32).min(max_dim as f32 / h as f32);
        let target_w = ((w as f32 * scale).round() as u32).max(1);
        let target_h = ((h as f32 * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, target_w, target_h, FilterType::Triangle);
    }
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    Frame::from_parts(rgba, 0, 0, delay)
}

/// Decodes a multi-frame GIF from memory into an animated GPUI `RenderImage`.
/// Returns `None` if decoding fails or the GIF contains only a single static frame.
pub fn decode_animated_gif_bytes(
    bytes: &[u8],
    max_dim: u32,
) -> Option<(Arc<RenderImage>, u32, u32, usize)> {
    let decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
    let (orig_w, orig_h) = decoder.dimensions();
    let mut frames: SmallVec<[Frame; 1]> = SmallVec::new();
    for frame_res in decoder.into_frames().take(MAX_ANIMATED_FRAMES) {
        match frame_res {
            Ok(frame) => frames.push(prepare_animated_frame(frame, max_dim)),
            Err(_) if !frames.is_empty() => break,
            Err(_) => return None,
        }
    }
    if frames.len() <= 1 {
        return None;
    }
    let count = frames.len();
    Some((Arc::new(RenderImage::new(frames)), orig_w, orig_h, count))
}

/// Decodes an animated WebP from memory into an animated GPUI `RenderImage`.
/// Returns `None` for static WebP images so they decode at full static resolution.
pub fn decode_animated_webp_bytes(
    bytes: &[u8],
    max_dim: u32,
) -> Option<(Arc<RenderImage>, u32, u32, usize)> {
    let decoder = WebPDecoder::new(Cursor::new(bytes)).ok()?;
    if !decoder.has_animation() {
        return None;
    }
    let (orig_w, orig_h) = decoder.dimensions();
    let mut frames: SmallVec<[Frame; 1]> = SmallVec::new();
    for frame_res in decoder.into_frames().take(MAX_ANIMATED_FRAMES) {
        match frame_res {
            Ok(frame) => frames.push(prepare_animated_frame(frame, max_dim)),
            Err(_) if !frames.is_empty() => break,
            Err(_) => return None,
        }
    }
    if frames.len() <= 1 {
        return None;
    }
    let count = frames.len();
    Some((Arc::new(RenderImage::new(frames)), orig_w, orig_h, count))
}

/// Decodes a local raster image (including animated GIFs & animated WebPs), scales it cleanly
/// to fit `max_dim`, and swizzles RGBA -> BGRA for GPUI.
pub fn decode_local_image(path: &Path, max_dim: u32) -> Option<(Arc<RenderImage>, u32, u32, usize)> {
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let format = reader.format();

    // Decode animated GIF / WebP with all frames preserved so GPUI animates them natively.
    if matches!(format, Some(ImageFormat::Gif | ImageFormat::WebP))
        && std::fs::metadata(path)
            .map(|m| m.len() <= MAX_ANIMATED_FILE_BYTES)
            .unwrap_or(false)
        && let Ok(bytes) = std::fs::read(path)
    {
        let anim_dim = max_dim.min(MAX_ANIMATED_DIM);
        let animated = match format {
            Some(ImageFormat::Gif) => decode_animated_gif_bytes(&bytes, anim_dim),
            Some(ImageFormat::WebP) => decode_animated_webp_bytes(&bytes, anim_dim),
            _ => None,
        };
        if let Some(result) = animated {
            return Some(result);
        }
    }

    let dyn_img = reader.decode().ok()?;
    let orig_w = dyn_img.width();
    let orig_h = dyn_img.height();
    let resized = if orig_w > max_dim || orig_h > max_dim {
        dyn_img.resize(max_dim, max_dim, FilterType::Triangle)
    } else {
        dyn_img
    };

    let mut rgba = resized.to_rgba8();
    // GPUI's `RenderImage` expects BGRA channel order inside `image::Frame`.
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    let frame = Frame::new(rgba);
    Some((Arc::new(RenderImage::new(smallvec![frame])), orig_w, orig_h, 1))
}

/// A single line of syntax-highlighted source code ready for GPUI `StyledText`.
#[derive(Debug, Clone)]
pub struct HighlightedLine {
    pub line_number: usize,
    pub text: String,
    pub spans: Vec<(Range<usize>, HighlightStyle)>,
}

/// Rich asynchronous preview payload shared between Inspector and Spacebar QuickLook.
#[derive(Clone)]
pub enum PreviewPayload {
    Loading,
    Image {
        image: Arc<RenderImage>,
        orig_width: u32,
        orig_height: u32,
        frame_count: usize,
    },
    Pdf {
        image: Arc<RenderImage>,
        page_index: u32,
        page_count: u32,
        page_width: u32,
        page_height: u32,
        loading_page: Option<u32>,
    },
    ShellThumbnail {
        image: Arc<RenderImage>,
        subtitle: String,
    },
    Markdown {
        source: String,
        truncated: bool,
    },
    Code {
        language: &'static str,
        lines: Vec<HighlightedLine>,
        truncated: bool,
    },
    FolderSummary {
        total_items: usize,
        files: usize,
        folders: usize,
        immediate_bytes: u64,
    },
    Unsupported {
        reason: String,
    },
}

fn rendered_pdf_to_payload(rendered: RenderedPdfPage) -> Result<PreviewPayload, String> {
    let dyn_img = image::load_from_memory(&rendered.png_bytes)
        .map_err(|e| format!("Failed decoding rendered PDF page: {e}"))?;
    let mut rgba = dyn_img.to_rgba8();
    let page_width = rgba.width();
    let page_height = rgba.height();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    let frame = Frame::new(rgba);
    Ok(PreviewPayload::Pdf {
        image: Arc::new(RenderImage::new(smallvec![frame])),
        page_index: rendered.page_index,
        page_count: rendered.page_count,
        page_width,
        page_height,
        loading_page: None,
    })
}

/// Renders a specific zero-based page of a local PDF file into a GPUI `PreviewPayload::Pdf`.
pub fn decode_pdf_page_from_path(
    path: &Path,
    page_index: u32,
    max_dim: u32,
) -> Result<PreviewPayload, String> {
    let rendered = render_pdf_page_from_path(path, page_index, max_dim)?;
    rendered_pdf_to_payload(rendered)
}

/// Renders a specific zero-based page of an in-memory PDF buffer (e.g. from an archive or SFTP/FTP)
/// into a GPUI `PreviewPayload::Pdf`.
pub fn decode_pdf_page_from_bytes(
    bytes: &[u8],
    page_index: u32,
    max_dim: u32,
) -> Result<PreviewPayload, String> {
    let rendered = render_pdf_page_from_bytes(bytes, page_index, max_dim)?;
    rendered_pdf_to_payload(rendered)
}

/// Loads a preview payload for `path` on a background worker thread.
pub fn load_preview_blocking(
    path: PathBuf,
    kind: FileKind,
    shell_worker: ShellWorker,
    palette: Palette,
) -> PreviewPayload {
    if path.is_dir() || kind == FileKind::Folder {
        return load_folder_summary(&path);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // 1. Direct local raster image decoding for crisp full-resolution preview (and animated GIF/WebP).
    if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico"
    ) && let Some((image, orig_width, orig_height, frame_count)) = decode_local_image(&path, 1280)
    {
        return PreviewPayload::Image {
            image,
            orig_width,
            orig_height,
            frame_count,
        };
    }

    // 1b. Native Windows WinRT PDF rendering (multi-page capable in QuickLook).
    if ext == "pdf" {
        match decode_pdf_page_from_path(&path, 0, PDF_PREVIEW_MAX_DIM) {
            Ok(payload) => return payload,
            Err(reason) => return PreviewPayload::Unsupported { reason },
        }
    }

    // 2. Markdown documents rendered via GPUI `TextView::markdown`.
    if matches!(ext.as_str(), "md" | "markdown")
        && let Some((text, truncated)) = read_utf8_preview(&path, 192 * 1024)
    {
        return PreviewPayload::Markdown {
            source: text,
            truncated,
        };
    }

    // 3. Text & source code with lightweight, zero-latency syntax highlighting.
    if is_text_or_code_extension(&ext)
        && let Some((text, truncated)) = read_utf8_preview(&path, 192 * 1024)
    {
        let language = language_label(&ext);
        let lines = highlight_code_lines(&text, &ext, 450, palette);
        return PreviewPayload::Code {
            language,
            lines,
            truncated,
        };
    }

    // 4. Native Windows Shell high-resolution thumbnail (videos, PDFs, Office docs, EXEs, SVGs, etc.).
    let (tx, rx) = mpsc::channel();
    shell_worker.request_thumbnail(path.clone(), 384, true, move |raw| {
        let _ = tx.send(raw);
    });
    if let Ok(Some(raw)) = rx.recv()
        && let Some(image) = raw_bgra_to_render_image(raw)
    {
        let subtitle = if ext.is_empty() {
            "Windows Shell Preview".to_string()
        } else {
            format!("{} Preview", ext.to_ascii_uppercase())
        };
        return PreviewPayload::ShellThumbnail { image, subtitle };
    }

    // 5. Fallback heuristic: attempt UTF-8 text preview for extensionless small text files (e.g. LICENSE, Dockerfile).
    if let Some((text, truncated)) = read_utf8_preview(&path, 64 * 1024) {
        let lines = highlight_code_lines(&text, "", 300, palette);
        return PreviewPayload::Code {
            language: "Plain Text",
            lines,
            truncated,
        };
    }

    PreviewPayload::Unsupported {
        reason: "Binary file — no inline preview available.".to_string(),
    }
}

/// Loads an inline preview directly from an entry inside a `.zip`/`.7z`/`.tar`/`.tar.gz` archive
/// without extracting anything to disk.
pub fn load_archive_entry_preview_blocking(
    archive_path: PathBuf,
    inner_path: String,
    kind: FileKind,
    palette: Palette,
) -> PreviewPayload {
    if kind == FileKind::Folder {
        return match texplorer_vfs::archive::list_archive_dir(&archive_path, &inner_path) {
            Ok(entries) => {
                let folders = entries.iter().filter(|e| e.is_dir()).count();
                let files = entries.len().saturating_sub(folders);
                let immediate_bytes = entries.iter().filter_map(|e| e.size).sum();
                PreviewPayload::FolderSummary {
                    total_items: entries.len(),
                    files,
                    folders,
                    immediate_bytes,
                }
            }
            Err(err) => PreviewPayload::Unsupported { reason: err },
        };
    }

    let ext = inner_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();

    // Images inside archive (up to 16 MiB, including animated GIF/WebP)
    if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico"
    ) && let Ok((bytes, false)) =
        texplorer_vfs::archive::read_archive_entry_bytes(&archive_path, &inner_path, 16 * 1024 * 1024)
    {
        let animated = match ext.as_str() {
            "gif" => decode_animated_gif_bytes(&bytes, MAX_ANIMATED_DIM),
            "webp" => decode_animated_webp_bytes(&bytes, MAX_ANIMATED_DIM),
            _ => None,
        };
        if let Some((image, orig_width, orig_height, frame_count)) = animated {
            return PreviewPayload::Image {
                image,
                orig_width,
                orig_height,
                frame_count,
            };
        }
        if let Ok(dyn_img) = image::load_from_memory(&bytes) {
            let orig_w = dyn_img.width();
            let orig_h = dyn_img.height();
            let resized = if orig_w > 1280 || orig_h > 1280 {
                dyn_img.resize(1280, 1280, FilterType::Triangle)
            } else {
                dyn_img
            };
            let mut rgba = resized.to_rgba8();
            for px in rgba.pixels_mut() {
                px.0.swap(0, 2);
            }
            let frame = Frame::new(rgba);
            return PreviewPayload::Image {
                image: Arc::new(RenderImage::new(smallvec![frame])),
                orig_width: orig_w,
                orig_height: orig_h,
                frame_count: 1,
            };
        }
    }

    // PDF documents inside archive (up to 16 MiB)
    if ext == "pdf"
        && let Ok((bytes, false)) =
            texplorer_vfs::archive::read_archive_entry_bytes(&archive_path, &inner_path, 16 * 1024 * 1024)
    {
        return match decode_pdf_page_from_bytes(&bytes, 0, PDF_PREVIEW_MAX_DIM) {
            Ok(payload) => payload,
            Err(reason) => PreviewPayload::Unsupported { reason },
        };
    }

    // Markdown or source code inside archive (up to 192 KiB)
    if let Ok((bytes, truncated)) =
        texplorer_vfs::archive::read_archive_entry_bytes(&archive_path, &inner_path, 192 * 1024)
        && !bytes.contains(&0)
        && let Ok(text) = String::from_utf8(bytes)
    {
        if matches!(ext.as_str(), "md" | "markdown") {
            return PreviewPayload::Markdown {
                source: text,
                truncated,
            };
        }
        let language = if is_text_or_code_extension(&ext) {
            language_label(&ext)
        } else {
            "Plain Text"
        };
        let lines = highlight_code_lines(&text, &ext, 450, palette);
        return PreviewPayload::Code {
            language,
            lines,
            truncated,
        };
    }

    PreviewPayload::Unsupported {
        reason: "Binary archive entry — extract to open natively.".to_string(),
    }
}

/// Builds a rich inline preview payload from raw bytes downloaded from a remote SFTP/FTP server.
pub fn preview_from_remote_bytes(
    remote_path: &str,
    bytes: Vec<u8>,
    truncated: bool,
    palette: Palette,
) -> PreviewPayload {
    let ext = remote_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();

    if !truncated
        && matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico"
        )
    {
        let animated = match ext.as_str() {
            "gif" => decode_animated_gif_bytes(&bytes, MAX_ANIMATED_DIM),
            "webp" => decode_animated_webp_bytes(&bytes, MAX_ANIMATED_DIM),
            _ => None,
        };
        if let Some((image, orig_width, orig_height, frame_count)) = animated {
            return PreviewPayload::Image {
                image,
                orig_width,
                orig_height,
                frame_count,
            };
        }
        if let Ok(dyn_img) = image::load_from_memory(&bytes) {
            let orig_w = dyn_img.width();
            let orig_h = dyn_img.height();
            let resized = if orig_w > 1280 || orig_h > 1280 {
                dyn_img.resize(1280, 1280, FilterType::Triangle)
            } else {
                dyn_img
            };
            let mut rgba = resized.to_rgba8();
            for px in rgba.pixels_mut() {
                px.0.swap(0, 2);
            }
            let frame = Frame::new(rgba);
            return PreviewPayload::Image {
                image: Arc::new(RenderImage::new(smallvec![frame])),
                orig_width: orig_w,
                orig_height: orig_h,
                frame_count: 1,
            };
        }
    }

    if !truncated && ext == "pdf" {
        return match decode_pdf_page_from_bytes(&bytes, 0, PDF_PREVIEW_MAX_DIM) {
            Ok(payload) => payload,
            Err(reason) => PreviewPayload::Unsupported { reason },
        };
    }

    if !bytes.contains(&0)
        && let Ok(text) = String::from_utf8(bytes)
    {
        if matches!(ext.as_str(), "md" | "markdown") {
            return PreviewPayload::Markdown {
                source: text,
                truncated,
            };
        }
        let language = if is_text_or_code_extension(&ext) {
            language_label(&ext)
        } else {
            "Plain Text"
        };
        let lines = highlight_code_lines(&text, &ext, 450, palette);
        return PreviewPayload::Code {
            language,
            lines,
            truncated,
        };
    }

    PreviewPayload::Unsupported {
        reason: "Binary remote file — click Open or Download to view locally.".to_string(),
    }
}

fn load_folder_summary(path: &Path) -> PreviewPayload {
    let Ok(rd) = std::fs::read_dir(path) else {
        return PreviewPayload::Unsupported {
            reason: "Unable to read folder contents.".to_string(),
        };
    };
    let mut files = 0usize;
    let mut folders = 0usize;
    let mut immediate_bytes = 0u64;
    for entry in rd.flatten().take(10_000) {
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                folders += 1;
            } else {
                files += 1;
                immediate_bytes = immediate_bytes.saturating_add(meta.len());
            }
        }
    }
    PreviewPayload::FolderSummary {
        total_items: files + folders,
        files,
        folders,
        immediate_bytes,
    }
}

fn read_utf8_preview(path: &Path, max_bytes: usize) -> Option<(String, bool)> {
    let mut file = File::open(path).ok()?;
    let meta_len = file.metadata().ok().map(|m| m.len()).unwrap_or(0);
    let mut buf = vec![0u8; max_bytes.min(meta_len as usize).max(1)];
    let read = file.read(&mut buf).ok()?;
    buf.truncate(read);

    // Reject binary files containing NUL bytes in the sampled prefix.
    if buf.contains(&0) {
        return None;
    }

    let text = String::from_utf8(buf).ok()?;
    let truncated = (meta_len as usize) > read;
    Some((text, truncated))
}

fn is_text_or_code_extension(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "toml"
            | "json"
            | "jsonc"
            | "yaml"
            | "yml"
            | "js"
            | "mjs"
            | "cjs"
            | "ts"
            | "mts"
            | "tsx"
            | "jsx"
            | "py"
            | "c"
            | "h"
            | "cpp"
            | "cc"
            | "cxx"
            | "hpp"
            | "cs"
            | "go"
            | "java"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sql"
            | "sh"
            | "bash"
            | "zsh"
            | "ps1"
            | "bat"
            | "cmd"
            | "xml"
            | "ini"
            | "cfg"
            | "conf"
            | "env"
            | "lua"
            | "zig"
            | "rb"
            | "php"
            | "swift"
            | "kt"
            | "txt"
            | "log"
            | "csv"
            | "iss"
            | "gitignore"
            | "gitattributes"
    )
}

fn language_label(ext: &str) -> &'static str {
    match ext {
        "rs" => "Rust",
        "toml" => "TOML",
        "json" | "jsonc" => "JSON",
        "yaml" | "yml" => "YAML",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" | "mts" => "TypeScript",
        "tsx" | "jsx" => "React TSX",
        "py" => "Python",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "go" => "Go",
        "java" => "Java",
        "html" | "htm" => "HTML",
        "css" | "scss" => "CSS",
        "sql" => "SQL",
        "sh" | "bash" | "zsh" => "Shell",
        "ps1" => "PowerShell",
        "bat" | "cmd" => "Batch",
        "xml" => "XML",
        "lua" => "Lua",
        "zig" => "Zig",
        "iss" => "Inno Setup",
        "csv" => "CSV",
        "log" => "Log",
        _ => "Plain Text",
    }
}

/// Lightweight per-line syntax highlighter producing sorted, non-overlapping byte ranges
/// guaranteed safe for GPUI `StyledText::with_highlights`.
pub fn highlight_code_lines(
    text: &str,
    ext: &str,
    max_lines: usize,
    palette: Palette,
) -> Vec<HighlightedLine> {
    let hash_comments = matches!(
        ext,
        "py" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh" | "ps1" | "rb" | "env" | "conf" | "cfg"
    );
    let slash_comments = matches!(
        ext,
        "rs" | "js" | "mjs" | "cjs" | "ts" | "mts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "cc"
            | "cxx" | "hpp" | "cs" | "go" | "java" | "swift" | "kt" | "zig" | "jsonc" | "css" | "scss"
    );

    let kw_style = HighlightStyle {
        color: Some(solid(palette.syntax_keyword)),
        ..Default::default()
    };
    let str_style = HighlightStyle {
        color: Some(solid(palette.syntax_string)),
        ..Default::default()
    };
    let num_style = HighlightStyle {
        color: Some(solid(palette.syntax_number)),
        ..Default::default()
    };
    let type_style = HighlightStyle {
        color: Some(solid(palette.syntax_type)),
        ..Default::default()
    };
    let comment_style = HighlightStyle {
        color: Some(solid(palette.syntax_comment)),
        ..Default::default()
    };

    text.lines()
        .take(max_lines)
        .enumerate()
        .map(|(idx, raw_line)| {
            // Normalize tabs to 4 spaces so columns align cleanly in monospace rendering.
            let line = raw_line.replace('\t', "    ");
            let bytes = line.as_bytes();
            let mut spans = Vec::new();
            let mut i = 0usize;

            while i < bytes.len() {
                let b = bytes[i];

                // Single-line comments (`//`, `#`, `--` for SQL/Lua, `;` for Inno/INI)
                if (slash_comments && b == b'/' && bytes.get(i + 1) == Some(&b'/'))
                    || (hash_comments && b == b'#')
                    || (matches!(ext, "sql" | "lua") && b == b'-' && bytes.get(i + 1) == Some(&b'-'))
                    || (matches!(ext, "iss" | "ini") && b == b';')
                {
                    spans.push((i..bytes.len(), comment_style));
                    break;
                }

                // Quoted string literals
                if b == b'"' || b == b'\'' {
                    let quote = b;
                    let start = i;
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == b'\\' && i + 1 < bytes.len() {
                            i += 2;
                            continue;
                        }
                        if bytes[i] == quote {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    spans.push((start..i, str_style));
                    continue;
                }

                // Numeric literals
                if b.is_ascii_digit()
                    && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_')
                {
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'.'
                            || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    spans.push((start..i, num_style));
                    continue;
                }

                // Identifiers, keywords, and PascalCase types
                if b.is_ascii_alphabetic() || b == b'_' {
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let word = &line[start..i];
                    if is_keyword(word) {
                        spans.push((start..i, kw_style));
                    } else if word.len() >= 2
                        && word.as_bytes()[0].is_ascii_uppercase()
                        && word.bytes().any(|c| c.is_ascii_lowercase())
                    {
                        spans.push((start..i, type_style));
                    }
                    continue;
                }

                i += 1;
            }

            HighlightedLine {
                line_number: idx + 1,
                text: line,
                spans,
            }
        })
        .collect()
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "fn" | "pub"
            | "let"
            | "mut"
            | "const"
            | "static"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "for"
            | "in"
            | "while"
            | "loop"
            | "if"
            | "else"
            | "match"
            | "return"
            | "use"
            | "mod"
            | "async"
            | "await"
            | "move"
            | "where"
            | "type"
            | "self"
            | "Self"
            | "true"
            | "false"
            | "null"
            | "None"
            | "Some"
            | "Ok"
            | "Err"
            | "function"
            | "export"
            | "import"
            | "from"
            | "class"
            | "interface"
            | "def"
            | "elif"
            | "try"
            | "except"
            | "catch"
            | "finally"
            | "throw"
            | "new"
            | "var"
            | "package"
            | "func"
            | "select"
            | "SELECT"
            | "FROM"
            | "WHERE"
            | "INSERT"
            | "UPDATE"
            | "DELETE"
            | "unsafe"
            | "extern"
            | "crate"
            | "super"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use texplorer_core::{AccentPreset, ThemePreset};

    #[test]
    fn highlights_rust_code_with_non_overlapping_ranges() {
        let palette = crate::theme::resolve_palette(
            ThemePreset::FilesDark,
            AccentPreset::FilesBlue,
            true,
        );
        let src = "pub fn answer() -> u32 {\n    // comment\n    let x = \"hi\";\n    42\n}";
        let lines = highlight_code_lines(src, "rs", 50, palette);
        assert_eq!(lines.len(), 5);
        for line in &lines {
            let mut prev_end = 0usize;
            for (range, _) in &line.spans {
                assert!(range.start >= prev_end);
                assert!(range.end <= line.text.len());
                prev_end = range.end;
            }
        }
    }

    #[test]
    fn decodes_multi_frame_gif_with_preserved_animation_frames() {
        let mut encoded = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut encoded);
            let f1 = Frame::from_parts(
                RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(80, 1),
            );
            let f2 = Frame::from_parts(
                RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(0, 1), // tests normalization (<20ms -> 100ms)
            );
            encoder.encode_frames([f1, f2]).expect("encode 2-frame gif");
        }

        let (render_img, w, h, count) =
            decode_animated_gif_bytes(&encoded, 800).expect("decode animated gif");
        assert_eq!((w, h), (4, 4));
        assert_eq!(count, 2);
        assert_eq!(render_img.frame_count(), 2);
        assert!(render_img.delay(0).numer_denom_ms().0 >= 70);
        assert_eq!(render_img.delay(1).numer_denom_ms(), (100, 1));
    }

    #[test]
    fn resolves_system_icon_targets_by_icon_style_and_entry_kind() {
        use texplorer_core::{Location, NetworkProtocol};

        let make_entry = |name: &str, ext: &str, loc: Location, flags: EntryFlags, kind: FileKind| {
            FileEntry {
                location: loc,
                name: name.to_string(),
                extension: ext.to_string(),
                size: if flags.contains(EntryFlags::IS_DIR) {
                    None
                } else {
                    Some(1024)
                },
                modified_secs: 100,
                created_secs: 100,
                flags,
                kind,
                type_label: String::new(),
            }
        };

        let dll = make_entry(
            "kernel32.dll",
            "dll",
            Location::local(r"C:\Windows\System32\kernel32.dll"),
            EntryFlags::empty(),
            FileKind::Other,
        );
        let exe = make_entry(
            "app.exe",
            "exe",
            Location::local(r"C:\Apps\app.exe"),
            EntryFlags::empty(),
            FileKind::Executable,
        );
        let remote_exe = make_entry(
            "tool.exe",
            "exe",
            Location::Remote {
                connection_id: uuid::Uuid::nil(),
                protocol: NetworkProtocol::Sftp,
                label: "VPS".to_string(),
                remote_path: "/opt/tool.exe".to_string(),
            },
            EntryFlags::empty(),
            FileKind::Executable,
        );
        let plain_dir = make_entry(
            "Projects",
            "",
            Location::local(r"C:\Projects"),
            EntryFlags::IS_DIR,
            FileKind::Folder,
        );
        let special_dir = make_entry(
            "Downloads",
            "",
            Location::local(r"C:\Users\Demo\Downloads"),
            EntryFlags::IS_DIR | EntryFlags::IS_READONLY,
            FileKind::Folder,
        );

        // Minimal mode: built-in icons everywhere.
        assert!(resolve_system_icon_target(&dll, IconStyle::Minimal, 32).is_none());
        assert!(resolve_system_icon_target(&plain_dir, IconStyle::Minimal, 32).is_none());

        // SystemFiles (default): extension-cached DLL, per-path local EXE, extension-cached remote EXE,
        // built-in Lucide icons for all folders.
        assert!(matches!(
            resolve_system_icon_target(&dll, IconStyle::SystemFiles, 32),
            Some((
                SystemIconKey::Extension { ref ext, size_px: 32 },
                SystemIconRequest::Extension(ref req_ext)
            )) if ext == "dll" && req_ext == "dll"
        ));
        assert!(matches!(
            resolve_system_icon_target(&exe, IconStyle::SystemFiles, 32),
            Some((SystemIconKey::Path { size_px: 32, .. }, SystemIconRequest::Path(_)))
        ));
        assert!(matches!(
            resolve_system_icon_target(&remote_exe, IconStyle::SystemFiles, 32),
            Some((
                SystemIconKey::Extension { ref ext, size_px: 32 },
                SystemIconRequest::Extension(ref req_ext)
            )) if ext == "exe" && req_ext == "exe"
        ));
        assert!(resolve_system_icon_target(&plain_dir, IconStyle::SystemFiles, 32).is_none());
        assert!(resolve_system_icon_target(&special_dir, IconStyle::SystemFiles, 32).is_none());

        // SystemAll: generic folder icon for standard dirs, per-path icon for READONLY/SYSTEM dirs.
        assert!(matches!(
            resolve_system_icon_target(&plain_dir, IconStyle::SystemAll, 32),
            Some((SystemIconKey::GenericFolder { size_px: 32 }, SystemIconRequest::GenericFolder))
        ));
        assert!(matches!(
            resolve_system_icon_target(&special_dir, IconStyle::SystemAll, 32),
            Some((SystemIconKey::Path { size_px: 32, .. }, SystemIconRequest::Path(_)))
        ));
    }

    #[test]
    fn icon_cache_invalidation_clears_system_icons_and_only_icon_fallback_thumbnails() {
        let make_img = || {
            let buf = RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
            Arc::new(RenderImage::new(smallvec![Frame::new(buf)]))
        };

        let mut sys_cache = SystemIconCache::new(16);
        let png_key = SystemIconKey::Extension {
            ext: "png".to_string(),
            size_px: 32,
        };
        let pdf_key = SystemIconKey::Extension {
            ext: "pdf".to_string(),
            size_px: 32,
        };
        assert!(sys_cache.try_mark_in_flight(png_key.clone()));
        assert!(sys_cache.insert(png_key.clone(), Some(make_img())).is_none());
        assert!(sys_cache.try_mark_in_flight(pdf_key.clone()));

        let drained_sys = sys_cache.clear_and_drain_images();
        assert_eq!(drained_sys.len(), 1);
        assert!(sys_cache.get(&png_key).is_none());
        assert!(sys_cache.try_mark_in_flight(png_key));
        assert!(sys_cache.try_mark_in_flight(pdf_key));

        let mut thumb_cache = ThumbnailCache::new(16);
        let photo_key = ThumbnailKey {
            path: PathBuf::from(r"C:\Photos\cat.png"),
            modified_secs: 1,
            size_px: 128,
            icon_fallback: false,
        };
        let doc_key = ThumbnailKey {
            path: PathBuf::from(r"C:\Docs\notes.txt"),
            modified_secs: 1,
            size_px: 128,
            icon_fallback: true,
        };
        thumb_cache.insert(photo_key.clone(), Some(make_img()));
        thumb_cache.insert(doc_key.clone(), Some(make_img()));

        let drained_thumbs = thumb_cache.clear_icon_fallbacks_and_drain_images();
        assert_eq!(drained_thumbs.len(), 1);
        assert!(thumb_cache.get(&photo_key).is_some());
        assert!(thumb_cache.get(&doc_key).is_none());
    }
}
