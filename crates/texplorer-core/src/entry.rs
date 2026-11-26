use std::cmp::Ordering;
use std::path::PathBuf;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::location::Location;

bitflags! {
    /// Compact metadata flags for filesystem entries.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct EntryFlags: u16 {
        const IS_DIR        = 1 << 0;
        const IS_HIDDEN     = 1 << 1;
        const IS_SYSTEM     = 1 << 2;
        const IS_READONLY   = 1 << 3;
        const IS_SYMLINK    = 1 << 4;
        const IS_ARCHIVE    = 1 << 5;
        const IS_COMPRESSED = 1 << 6;
        const IS_CLOUD_ONLY = 1 << 7;
    }
}

/// High-level semantic category used for icons, badges, and preview dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileKind {
    Folder,
    Image,
    Video,
    Audio,
    Code,
    Document,
    Archive,
    Executable,
    Symlink,
    Other,
}

/// Single row or grid tile representing an item in a directory, archive, or search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub extension: String,
    pub location: Location,
    /// File size in bytes. Folders start as `None` until resolved in the background.
    pub size: Option<u64>,
    pub modified_secs: i64,
    pub created_secs: i64,
    pub flags: EntryFlags,
    pub kind: FileKind,
    pub type_label: String,
}

impl FileEntry {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags.contains(EntryFlags::IS_DIR)
    }

    #[inline]
    pub fn is_hidden(&self) -> bool {
        self.flags.contains(EntryFlags::IS_HIDDEN) || self.flags.contains(EntryFlags::IS_SYSTEM)
    }

    #[inline]
    pub fn is_symlink(&self) -> bool {
        self.flags.contains(EntryFlags::IS_SYMLINK)
    }
}

/// Display layout mode for a file pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Details,
    Grid,
    Compact,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::Grid => "Grid",
            Self::Compact => "Compact",
        }
    }
}

/// Active sort column for file listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortColumn {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

/// Sort specification applied to a pane's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    pub column: SortColumn,
    pub descending: bool,
    pub folders_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            column: SortColumn::Name,
            descending: false,
            folders_first: true,
        }
    }
}

impl SortSpec {
    /// Toggles direction when clicking the same column, or switches to a new column.
    pub fn toggle_column(&mut self, column: SortColumn) {
        if self.column == column {
            self.descending = !self.descending;
        } else {
            self.column = column;
            self.descending = matches!(column, SortColumn::Modified | SortColumn::Size);
        }
    }
}

/// Storage device category reported by the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    Wsl,
    Unknown,
}

/// Mounted storage volume displayed in the sidebar and Home dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub root_path: PathBuf,
    pub short_name: String,
    pub label: String,
    pub file_system: String,
    pub kind: DriveKind,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

impl VolumeInfo {
    #[inline]
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    #[inline]
    pub fn used_fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes() as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
        }
    }

    pub fn display_name(&self) -> String {
        if self.label.is_empty() {
            format!("Local Disk ({})", self.short_name)
        } else {
            format!("{} ({})", self.label, self.short_name)
        }
    }
}

/// Standard OS user folder category for the sidebar Quick Access section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KnownFolderKind {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    UserHome,
}

/// Resolved standard OS folder entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownFolder {
    pub kind: KnownFolderKind,
    pub name: String,
    pub path: PathBuf,
}

/// Classifies a lowercase file extension into a semantic `FileKind` and human-readable type label.
pub fn classify_extension(ext: &str, is_dir: bool, is_symlink: bool) -> (FileKind, String) {
    if is_dir {
        return (
            FileKind::Folder,
            if is_symlink {
                "Folder Link".to_string()
            } else {
                "File Folder".to_string()
            },
        );
    }

    let lower = ext.to_ascii_lowercase();
    let (kind, label) = match lower.as_str() {
        "png" => (FileKind::Image, "PNG Image"),
        "jpg" | "jpeg" => (FileKind::Image, "JPEG Image"),
        "webp" => (FileKind::Image, "WebP Image"),
        "gif" => (FileKind::Image, "GIF Image"),
        "bmp" => (FileKind::Image, "BMP Image"),
        "svg" => (FileKind::Image, "SVG Vector"),
        "ico" => (FileKind::Image, "Icon File"),
        "avif" | "heic" | "tiff" | "tif" => (FileKind::Image, "Image File"),

        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "flv" => (FileKind::Video, "Video File"),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" => (FileKind::Audio, "Audio File"),

        "rs" => (FileKind::Code, "Rust Source"),
        "toml" => (FileKind::Code, "TOML Config"),
        "json" | "jsonc" => (FileKind::Code, "JSON File"),
        "yaml" | "yml" => (FileKind::Code, "YAML File"),
        "js" | "mjs" | "cjs" => (FileKind::Code, "JavaScript"),
        "ts" | "mts" => (FileKind::Code, "TypeScript"),
        "tsx" | "jsx" => (FileKind::Code, "React Source"),
        "py" => (FileKind::Code, "Python Script"),
        "c" | "h" => (FileKind::Code, "C Source"),
        "cpp" | "cc" | "cxx" | "hpp" => (FileKind::Code, "C++ Source"),
        "cs" => (FileKind::Code, "C# Source"),
        "go" => (FileKind::Code, "Go Source"),
        "java" => (FileKind::Code, "Java Source"),
        "html" | "htm" => (FileKind::Code, "HTML Document"),
        "css" | "scss" => (FileKind::Code, "Stylesheet"),
        "sql" => (FileKind::Code, "SQL Script"),
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => (FileKind::Code, "Shell Script"),
        "xml" | "ini" | "cfg" | "conf" | "env" => (FileKind::Code, "Config File"),
        "lua" | "zig" | "rb" | "php" | "swift" | "kt" => (FileKind::Code, "Source Code"),

        "md" | "markdown" => (FileKind::Document, "Markdown"),
        "txt" | "log" | "csv" => (FileKind::Document, "Text Document"),
        "pdf" => (FileKind::Document, "PDF Document"),
        "doc" | "docx" => (FileKind::Document, "Word Document"),
        "xls" | "xlsx" => (FileKind::Document, "Spreadsheet"),
        "ppt" | "pptx" => (FileKind::Document, "Presentation"),

        "zip" => (FileKind::Archive, "ZIP Archive"),
        "7z" => (FileKind::Archive, "7-Zip Archive"),
        "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" => (FileKind::Archive, "TAR Archive"),
        "rar" => (FileKind::Archive, "RAR Archive"),
        "iso" => (FileKind::Archive, "Disk Image"),

        "exe" => (FileKind::Executable, "Application"),
        "msi" | "msix" => (FileKind::Executable, "Installer"),
        "dll" | "sys" => (FileKind::Executable, "System Library"),
        "lnk" => (FileKind::Symlink, "Shortcut"),

        "" => (FileKind::Other, "File"),
        _ => {
            return (
                if is_symlink {
                    FileKind::Symlink
                } else {
                    FileKind::Other
                },
                format!("{} File", lower.to_ascii_uppercase()),
            );
        }
    };

    (
        if is_symlink && kind == FileKind::Other {
            FileKind::Symlink
        } else {
            kind
        },
        label.to_string(),
    )
}

/// Natural case-insensitive comparison (`file2.txt` < `file10.txt`) matching Windows Explorer.
pub fn natural_cmp_ignore_ascii_case(a: &str, b: &str) -> Ordering {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let (mut i, mut j) = (0usize, 0usize);

    while i < ab.len() && j < bb.len() {
        let ca = ab[i];
        let cb = bb[j];

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let start_a = i;
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            let start_b = j;
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }

            let num_a = &ab[start_a..i];
            let num_b = &bb[start_b..j];
            let trimmed_a = trim_leading_zeros(num_a);
            let trimmed_b = trim_leading_zeros(num_b);

            let cmp = trimmed_a
                .len()
                .cmp(&trimmed_b.len())
                .then_with(|| trimmed_a.cmp(trimmed_b))
                .then_with(|| num_a.len().cmp(&num_b.len()));
            if cmp != Ordering::Equal {
                return cmp;
            }
        } else {
            let la = ca.to_ascii_lowercase();
            let lb = cb.to_ascii_lowercase();
            if la != lb {
                return la.cmp(&lb);
            }
            i += 1;
            j += 1;
        }
    }

    ab.len().cmp(&bb.len())
}

fn trim_leading_zeros(digits: &[u8]) -> &[u8] {
    let first_non_zero = digits
        .iter()
        .position(|&b| b != b'0')
        .unwrap_or(digits.len().saturating_sub(1));
    &digits[first_non_zero..]
}

/// Sorts file entries in place according to `spec`.
pub fn sort_entries(entries: &mut [FileEntry], spec: SortSpec) {
    entries.sort_by(|a, b| {
        if spec.folders_first {
            match (a.is_dir(), b.is_dir()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }

        let primary = match spec.column {
            SortColumn::Name => natural_cmp_ignore_ascii_case(&a.name, &b.name),
            SortColumn::Modified => a
                .modified_secs
                .cmp(&b.modified_secs)
                .then_with(|| natural_cmp_ignore_ascii_case(&a.name, &b.name)),
            SortColumn::Type => a
                .type_label
                .to_ascii_lowercase()
                .cmp(&b.type_label.to_ascii_lowercase())
                .then_with(|| natural_cmp_ignore_ascii_case(&a.name, &b.name)),
            SortColumn::Size => a
                .size
                .unwrap_or(0)
                .cmp(&b.size.unwrap_or(0))
                .then_with(|| natural_cmp_ignore_ascii_case(&a.name, &b.name)),
        };

        if spec.descending {
            primary.reverse()
        } else {
            primary
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_entry(name: &str, is_dir: bool, size: Option<u64>, mtime: i64) -> FileEntry {
        let ext = if is_dir {
            String::new()
        } else {
            name.rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_default()
        };
        let (kind, type_label) = classify_extension(&ext, is_dir, false);
        FileEntry {
            name: name.to_string(),
            extension: ext,
            location: Location::local(format!("X:\\{name}")),
            size,
            modified_secs: mtime,
            created_secs: mtime,
            flags: if is_dir {
                EntryFlags::IS_DIR
            } else {
                EntryFlags::empty()
            },
            kind,
            type_label,
        }
    }

    #[test]
    fn natural_sort_places_folders_first_and_orders_numbers() {
        let mut items = vec![
            dummy_entry("file10.txt", false, Some(10), 100),
            dummy_entry("file2.txt", false, Some(20), 200),
            dummy_entry("Zeta", true, None, 50),
            dummy_entry("alpha", true, None, 60),
        ];

        sort_entries(&mut items, SortSpec::default());
        let names: Vec<&str> = items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Zeta", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn sorts_by_size_descending() {
        let mut items = vec![
            dummy_entry("small.bin", false, Some(100), 1),
            dummy_entry("huge.bin", false, Some(9999), 2),
            dummy_entry("folder", true, None, 3),
        ];
        let mut spec = SortSpec::default();
        spec.toggle_column(SortColumn::Size);
        assert!(spec.descending);

        sort_entries(&mut items, spec);
        let names: Vec<&str> = items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["folder", "huge.bin", "small.bin"]);
    }
}
