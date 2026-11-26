//! Transparent virtual archive provider supporting `.zip`, `.7z`, `.tar`, and `.tar.gz`/`.tgz`:
//! - Cached directory listing (`Location::Archive`) with virtual intermediate folder synthesis
//! - In-memory single-entry reading for live Inspector & Spacebar QuickLook preview
//! - Safe Zip-Slip-protected extraction (full archive or selective entries/subfolders)
//! - Deflate `.zip` compression from selected files and folders

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use flate2::read::GzDecoder;
use texplorer_core::{
    EntryFlags, FileEntry, Location, classify_extension, location::normalize_archive_inner,
};
use zip::write::SimpleFileOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Tar,
    TarGz,
}

#[derive(Debug, Clone)]
struct ArchiveRawEntry {
    inner_path: String,
    is_dir: bool,
    size: u64,
    modified_secs: i64,
}

type ArchiveCacheMap = HashMap<(PathBuf, i64), Vec<ArchiveRawEntry>>;

static ARCHIVE_INDEX_CACHE: LazyLock<Mutex<ArchiveCacheMap>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Detects the supported virtual archive format from a file's extension.
pub fn detect_archive_format(path: &Path) -> Option<ArchiveFormat> {
    let lower = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_ascii_lowercase())?;
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some(ArchiveFormat::TarGz)
    } else if lower.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else if lower.ends_with(".7z") {
        Some(ArchiveFormat::SevenZ)
    } else if lower.ends_with(".tar") {
        Some(ArchiveFormat::Tar)
    } else {
        None
    }
}

/// Returns `true` when `path` is a browsable archive (`.zip`, `.7z`, `.tar`, `.tar.gz`, `.tgz`).
pub fn is_supported_archive(path: &Path) -> bool {
    detect_archive_format(path).is_some()
}

/// Lists immediate virtual children of `inner_path` inside `archive_path`.
pub fn list_archive_dir(archive_path: &Path, inner_path: &str) -> Result<Vec<FileEntry>, String> {
    let all = load_or_cache_archive_entries(archive_path)?;
    let prefix = normalize_archive_inner(inner_path);

    let mut folders: BTreeMap<String, i64> = BTreeMap::new();
    let mut files: Vec<(String, ArchiveRawEntry)> = Vec::new();

    for entry in &all {
        let rel = if prefix.is_empty() {
            entry.inner_path.as_str()
        } else if let Some(rest) = entry.inner_path.strip_prefix(&prefix) {
            match rest.strip_prefix('/') {
                Some(r) if !r.is_empty() => r,
                _ => continue,
            }
        } else {
            continue;
        };

        if let Some((dir_name, _rest)) = rel.split_once('/') {
            if !dir_name.is_empty() {
                let slot = folders.entry(dir_name.to_string()).or_insert(0);
                if entry.modified_secs > *slot {
                    *slot = entry.modified_secs;
                }
            }
        } else if entry.is_dir {
            if !rel.is_empty() {
                let slot = folders.entry(rel.to_string()).or_insert(0);
                if entry.modified_secs > *slot {
                    *slot = entry.modified_secs;
                }
            }
        } else if !rel.is_empty() {
            files.push((rel.to_string(), entry.clone()));
        }
    }

    let mut out = Vec::with_capacity(folders.len() + files.len());
    for (name, mtime) in folders {
        let child_inner = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let (kind, type_label) = classify_extension("", true, false);
        out.push(FileEntry {
            name,
            extension: String::new(),
            location: Location::archive(archive_path, child_inner),
            size: None,
            modified_secs: mtime,
            created_secs: mtime,
            flags: EntryFlags::IS_DIR | EntryFlags::IS_ARCHIVE,
            kind,
            type_label,
        });
    }

    for (name, raw) in files {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let (kind, type_label) = classify_extension(&ext, false, false);
        out.push(FileEntry {
            name,
            extension: ext,
            location: Location::archive(archive_path, &raw.inner_path),
            size: Some(raw.size),
            modified_secs: raw.modified_secs,
            created_secs: raw.modified_secs,
            flags: EntryFlags::IS_ARCHIVE | EntryFlags::IS_READONLY,
            kind,
            type_label,
        });
    }

    Ok(out)
}

/// Reads up to `max_bytes` from a single file inside an archive for instant Inspector/QuickLook preview.
pub fn read_archive_entry_bytes(
    archive_path: &Path,
    inner_path: &str,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), String> {
    let format = detect_archive_format(archive_path)
        .ok_or_else(|| format!("Unsupported archive format: {}", archive_path.display()))?;
    let target = normalize_archive_inner(inner_path);
    if target.is_empty() {
        return Err("Archive path refers to a directory.".to_string());
    }

    match format {
        ArchiveFormat::Zip => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed opening archive: {e}"))?;
            let mut zip = zip::ZipArchive::new(file)
                .map_err(|e| format!("Failed reading ZIP archive: {e}"))?;
            for i in 0..zip.len() {
                let mut entry = zip
                    .by_index(i)
                    .map_err(|e| format!("Failed reading ZIP entry: {e}"))?;
                if normalize_archive_inner(entry.name()) == target {
                    if entry.is_dir() {
                        return Err("Selected archive entry is a directory.".to_string());
                    }
                    return read_bounded(&mut entry, max_bytes);
                }
            }
            Err(format!("Entry \"{target}\" not found in ZIP archive."))
        }
        ArchiveFormat::Tar => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed opening TAR archive: {e}"))?;
            read_tar_entry_bytes(file, &target, max_bytes)
        }
        ArchiveFormat::TarGz => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed opening TAR.GZ archive: {e}"))?;
            let gz = GzDecoder::new(file);
            read_tar_entry_bytes(gz, &target, max_bytes)
        }
        ArchiveFormat::SevenZ => {
            let mut reader = sevenz_rust::SevenZReader::open(
                archive_path,
                sevenz_rust::Password::empty(),
            )
            .map_err(|e| format!("Failed opening 7z archive: {e}"))?;
            let mut found: Option<(Vec<u8>, bool)> = None;
            reader
                .for_each_entries(|entry, r| {
                    if !entry.is_directory() && normalize_archive_inner(entry.name()) == target {
                        let (bytes, trunc) = read_bounded(r, max_bytes)
                            .map_err(sevenz_rust::Error::other)?;
                        found = Some((bytes, trunc));
                        return Ok(false);
                    }
                    Ok(true)
                })
                .map_err(|e| format!("Failed reading 7z entry: {e}"))?;
            found.ok_or_else(|| format!("Entry \"{target}\" not found in 7z archive."))
        }
    }
}

/// Extracts an entire archive safely into `dest_dir` with Zip-Slip path sanitization.
pub fn extract_archive_all(archive_path: &Path, dest_dir: &Path) -> Result<usize, String> {
    extract_archive_entries(archive_path, &[], dest_dir)
}

/// Extracts either all entries (when `inner_paths` is empty) or only matching files/subfolders
/// from `archive_path` into `dest_dir`.
pub fn extract_archive_entries(
    archive_path: &Path,
    inner_paths: &[String],
    dest_dir: &Path,
) -> Result<usize, String> {
    let format = detect_archive_format(archive_path)
        .ok_or_else(|| format!("Unsupported archive format: {}", archive_path.display()))?;
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("Failed creating destination folder: {e}"))?;

    let filters: Vec<String> = inner_paths
        .iter()
        .map(|s| normalize_archive_inner(s))
        .filter(|s| !s.is_empty())
        .collect();

    match format {
        ArchiveFormat::Zip => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed opening ZIP archive: {e}"))?;
            let mut zip = zip::ZipArchive::new(file)
                .map_err(|e| format!("Failed reading ZIP archive: {e}"))?;
            let mut extracted = 0usize;
            for i in 0..zip.len() {
                let mut entry = zip
                    .by_index(i)
                    .map_err(|e| format!("Failed reading ZIP entry: {e}"))?;
                let norm = normalize_archive_inner(entry.name());
                let Some(rel_out) = match_extract_relative(&norm, &filters) else {
                    continue;
                };
                let target_path = safe_join(dest_dir, &rel_out)?;
                if entry.is_dir() {
                    std::fs::create_dir_all(&target_path)
                        .map_err(|e| format!("Failed creating folder: {e}"))?;
                } else {
                    if let Some(parent) = target_path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("Failed creating parent folder: {e}"))?;
                    }
                    let mut out_file = File::create(&target_path)
                        .map_err(|e| format!("Failed creating extracted file: {e}"))?;
                    io::copy(&mut entry, &mut out_file)
                        .map_err(|e| format!("Failed extracting ZIP entry: {e}"))?;
                    extracted += 1;
                }
            }
            Ok(extracted)
        }
        ArchiveFormat::Tar => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed opening TAR archive: {e}"))?;
            extract_tar_stream(file, &filters, dest_dir)
        }
        ArchiveFormat::TarGz => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed opening TAR.GZ archive: {e}"))?;
            extract_tar_stream(GzDecoder::new(file), &filters, dest_dir)
        }
        ArchiveFormat::SevenZ => {
            let mut reader = sevenz_rust::SevenZReader::open(
                archive_path,
                sevenz_rust::Password::empty(),
            )
            .map_err(|e| format!("Failed opening 7z archive: {e}"))?;
            let mut extracted = 0usize;
            reader
                .for_each_entries(|entry, r| {
                    let norm = normalize_archive_inner(entry.name());
                    let Some(rel_out) = match_extract_relative(&norm, &filters) else {
                        return Ok(true);
                    };
                    let target_path = safe_join(dest_dir, &rel_out)
                        .map_err(sevenz_rust::Error::other)?;
                    if entry.is_directory() {
                        std::fs::create_dir_all(&target_path)
                            .map_err(|e| sevenz_rust::Error::other(e.to_string()))?;
                    } else {
                        if let Some(parent) = target_path.parent() {
                            std::fs::create_dir_all(parent)
                                .map_err(|e| sevenz_rust::Error::other(e.to_string()))?;
                        }
                        let mut out_file = File::create(&target_path)
                            .map_err(|e| sevenz_rust::Error::other(e.to_string()))?;
                        io::copy(r, &mut out_file)
                            .map_err(|e| sevenz_rust::Error::other(e.to_string()))?;
                        extracted += 1;
                    }
                    Ok(true)
                })
                .map_err(|e| format!("Failed extracting 7z archive: {e}"))?;
            Ok(extracted)
        }
    }
}

/// Compresses one or more local files/folders into a Deflate `.zip` archive at `output_zip`.
pub fn compress_to_zip(sources: &[PathBuf], output_zip: &Path) -> Result<usize, String> {
    if sources.is_empty() {
        return Err("No items selected to compress.".to_string());
    }
    let out_file = File::create(output_zip)
        .map_err(|e| format!("Failed creating ZIP \"{}\": {e}", output_zip.display()))?;
    let mut writer = zip::ZipWriter::new(out_file);
    let options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut file_count = 0usize;

    for src in sources {
        let base_name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".to_string());
        if src.is_dir() {
            add_dir_to_zip(&mut writer, src, &base_name, options, &mut file_count)?;
        } else if src.is_file() {
            add_file_to_zip(&mut writer, src, &base_name, options)?;
            file_count += 1;
        }
    }

    writer
        .finish()
        .map_err(|e| format!("Failed finalizing ZIP archive: {e}"))?;
    invalidate_archive_cache(output_zip);
    Ok(file_count)
}

pub fn invalidate_archive_cache(archive_path: &Path) {
    if let Ok(mut guard) = ARCHIVE_INDEX_CACHE.lock() {
        guard.retain(|(p, _), _| p != archive_path);
    }
}

fn add_dir_to_zip<W: Write + io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    dir: &Path,
    prefix: &str,
    options: SimpleFileOptions,
    count: &mut usize,
) -> Result<(), String> {
    let dir_key = format!("{}/", prefix.trim_end_matches('/'));
    writer
        .add_directory(dir_key, options)
        .map_err(|e| format!("Failed adding directory to ZIP: {e}"))?;

    let rd = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed reading folder \"{}\": {e}", dir.display()))?;
    for item in rd.flatten() {
        let path = item.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let child_name = item.file_name().to_string_lossy().into_owned();
        let child_key = format!("{prefix}/{child_name}");
        if meta.is_dir() {
            add_dir_to_zip(writer, &path, &child_key, options, count)?;
        } else if meta.is_file() {
            add_file_to_zip(writer, &path, &child_key, options)?;
            *count += 1;
        }
    }
    Ok(())
}

fn add_file_to_zip<W: Write + io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    src: &Path,
    zip_name: &str,
    options: SimpleFileOptions,
) -> Result<(), String> {
    writer
        .start_file(zip_name, options)
        .map_err(|e| format!("Failed adding \"{zip_name}\" to ZIP: {e}"))?;
    let mut f = File::open(src)
        .map_err(|e| format!("Failed reading \"{}\": {e}", src.display()))?;
    io::copy(&mut f, writer).map_err(|e| format!("Failed writing \"{zip_name}\" to ZIP: {e}"))?;
    Ok(())
}

fn load_or_cache_archive_entries(archive_path: &Path) -> Result<Vec<ArchiveRawEntry>, String> {
    let mtime = std::fs::metadata(archive_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let key = (archive_path.to_path_buf(), mtime);

    if let Ok(guard) = ARCHIVE_INDEX_CACHE.lock()
        && let Some(cached) = guard.get(&key)
    {
        return Ok(cached.clone());
    }

    let format = detect_archive_format(archive_path)
        .ok_or_else(|| format!("Unsupported archive format: {}", archive_path.display()))?;
    let entries = match format {
        ArchiveFormat::Zip => index_zip(archive_path)?,
        ArchiveFormat::Tar => index_tar_stream(
            File::open(archive_path).map_err(|e| format!("Failed opening TAR archive: {e}"))?,
        )?,
        ArchiveFormat::TarGz => index_tar_stream(GzDecoder::new(
            File::open(archive_path).map_err(|e| format!("Failed opening TAR.GZ archive: {e}"))?,
        ))?,
        ArchiveFormat::SevenZ => index_sevenz(archive_path)?,
    };

    if let Ok(mut guard) = ARCHIVE_INDEX_CACHE.lock() {
        if guard.len() >= 16 {
            guard.clear();
        }
        guard.insert(key, entries.clone());
    }
    Ok(entries)
}

fn index_zip(path: &Path) -> Result<Vec<ArchiveRawEntry>, String> {
    let file = File::open(path).map_err(|e| format!("Failed opening ZIP archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Failed parsing ZIP archive: {e}"))?;
    let mut out = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let norm = normalize_archive_inner(entry.name());
        if norm.is_empty() {
            continue;
        }
        let modified_secs = entry
            .last_modified()
            .and_then(zip_datetime_to_unix)
            .unwrap_or(0);
        out.push(ArchiveRawEntry {
            inner_path: norm,
            is_dir: entry.is_dir(),
            size: entry.size(),
            modified_secs,
        });
    }
    Ok(out)
}

fn zip_datetime_to_unix(dt: zip::DateTime) -> Option<i64> {
    let y = i64::from(dt.year());
    let m = i64::from(dt.month());
    let d = i64::from(dt.day());
    let days = days_from_civil(y, m, d);
    let secs = days * 86_400
        + i64::from(dt.hour()) * 3_600
        + i64::from(dt.minute()) * 60
        + i64::from(dt.second());
    Some(secs)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn index_tar_stream<R: Read>(reader: R) -> Result<Vec<ArchiveRawEntry>, String> {
    let mut tar = tar::Archive::new(reader);
    let entries = tar
        .entries()
        .map_err(|e| format!("Failed parsing TAR archive: {e}"))?;
    let mut out = Vec::new();
    for item in entries.flatten() {
        let Ok(raw_path) = item.path() else {
            continue;
        };
        let norm = normalize_archive_inner(&raw_path.to_string_lossy());
        if norm.is_empty() {
            continue;
        }
        let header = item.header();
        let is_dir = header.entry_type().is_dir();
        let size = header.size().unwrap_or(0);
        let modified_secs = header.mtime().unwrap_or(0) as i64;
        out.push(ArchiveRawEntry {
            inner_path: norm,
            is_dir,
            size,
            modified_secs,
        });
    }
    Ok(out)
}

fn index_sevenz(path: &Path) -> Result<Vec<ArchiveRawEntry>, String> {
    let reader = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
        .map_err(|e| format!("Failed opening 7z archive: {e}"))?;
    let mut out = Vec::with_capacity(reader.archive().files.len());
    for entry in &reader.archive().files {
        let norm = normalize_archive_inner(entry.name());
        if norm.is_empty() {
            continue;
        }
        let modified_secs = if entry.has_last_modified_date {
            let ticks = entry.last_modified_date().to_raw();
            ((ticks / 10_000_000) as i64).saturating_sub(11_644_473_600)
        } else {
            0
        };
        out.push(ArchiveRawEntry {
            inner_path: norm,
            is_dir: entry.is_directory(),
            size: entry.size(),
            modified_secs,
        });
    }
    Ok(out)
}

fn read_tar_entry_bytes<R: Read>(
    reader: R,
    target: &str,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut tar = tar::Archive::new(reader);
    let entries = tar
        .entries()
        .map_err(|e| format!("Failed reading TAR archive: {e}"))?;
    for item in entries {
        let mut entry = item.map_err(|e| format!("Failed reading TAR entry: {e}"))?;
        let raw_path = entry
            .path()
            .map_err(|e| format!("Invalid TAR entry path: {e}"))?;
        if normalize_archive_inner(&raw_path.to_string_lossy()) == target {
            if entry.header().entry_type().is_dir() {
                return Err("Selected archive entry is a directory.".to_string());
            }
            return read_bounded(&mut entry, max_bytes);
        }
    }
    Err(format!("Entry \"{target}\" not found in TAR archive."))
}

fn extract_tar_stream<R: Read>(
    reader: R,
    filters: &[String],
    dest_dir: &Path,
) -> Result<usize, String> {
    let mut tar = tar::Archive::new(reader);
    let entries = tar
        .entries()
        .map_err(|e| format!("Failed reading TAR archive: {e}"))?;
    let mut extracted = 0usize;
    for item in entries {
        let mut entry = item.map_err(|e| format!("Failed reading TAR entry: {e}"))?;
        let raw_path = entry
            .path()
            .map_err(|e| format!("Invalid TAR entry path: {e}"))?;
        let norm = normalize_archive_inner(&raw_path.to_string_lossy());
        let Some(rel_out) = match_extract_relative(&norm, filters) else {
            continue;
        };
        let target_path = safe_join(dest_dir, &rel_out)?;
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&target_path)
                .map_err(|e| format!("Failed creating folder: {e}"))?;
        } else {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed creating parent folder: {e}"))?;
            }
            let mut out_file = File::create(&target_path)
                .map_err(|e| format!("Failed creating extracted file: {e}"))?;
            io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("Failed extracting TAR entry: {e}"))?;
            extracted += 1;
        }
    }
    Ok(extracted)
}

fn read_bounded<R: Read + ?Sized>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut limited = reader.take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| format!("Failed reading archive entry bytes: {e}"))?;
    let truncated = buf.len() > max_bytes;
    if truncated {
        buf.truncate(max_bytes);
    }
    Ok((buf, truncated))
}

/// Determines whether `norm_entry` matches `filters` (empty = extract everything) and returns
/// the clean relative output path under `dest_dir`.
fn match_extract_relative(norm_entry: &str, filters: &[String]) -> Option<String> {
    if norm_entry.is_empty() {
        return None;
    }
    if filters.is_empty() {
        return Some(norm_entry.to_string());
    }
    for f in filters {
        if norm_entry == f {
            let leaf = norm_entry.rsplit('/').next().unwrap_or(norm_entry);
            return Some(leaf.to_string());
        }
        if let Some(rest) = norm_entry.strip_prefix(f)
            && let Some(sub) = rest.strip_prefix('/')
        {
            let folder_name = f.rsplit('/').next().unwrap_or(f);
            return Some(format!("{folder_name}/{sub}"));
        }
    }
    None
}

/// Joins `rel_posix` onto `dest_dir`, rejecting any `..` or root traversal (Zip-Slip guard).
fn safe_join(dest_dir: &Path, rel_posix: &str) -> Result<PathBuf, String> {
    let mut out = dest_dir.to_path_buf();
    for part in rel_posix.split('/').filter(|s| !s.is_empty()) {
        let p = Path::new(part);
        match p.components().next() {
            Some(Component::Normal(seg)) => out.push(seg),
            _ => {
                return Err(format!(
                    "Blocked unsafe archive path component \"{part}\"."
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compresses_browses_reads_and_extracts_zip_archive() {
        let root =
            std::env::temp_dir().join(format!("texplorer_archive_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src_dir = root.join("src_data");
        std::fs::create_dir_all(src_dir.join("nested")).unwrap();
        std::fs::write(src_dir.join("readme.md"), b"# Hello Archive").unwrap();
        std::fs::write(src_dir.join("nested").join("code.rs"), b"fn ok() {}").unwrap();

        let zip_path = root.join("bundle.zip");
        let count = compress_to_zip(std::slice::from_ref(&src_dir), &zip_path).unwrap();
        assert_eq!(count, 2);
        assert!(is_supported_archive(&zip_path));

        let top = list_archive_dir(&zip_path, "").unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].name, "src_data");
        assert!(top[0].is_dir());

        let inside = list_archive_dir(&zip_path, "src_data").unwrap();
        assert_eq!(inside.len(), 2);
        assert!(inside.iter().any(|e| e.name == "nested" && e.is_dir()));
        assert!(inside.iter().any(|e| e.name == "readme.md" && !e.is_dir()));

        let (bytes, trunc) =
            read_archive_entry_bytes(&zip_path, "src_data/nested/code.rs", 1024).unwrap();
        assert!(!trunc);
        assert_eq!(bytes, b"fn ok() {}");

        let extract_dir = root.join("extracted");
        let extracted =
            extract_archive_entries(&zip_path, &["src_data/nested".to_string()], &extract_dir)
                .unwrap();
        assert_eq!(extracted, 1);
        assert_eq!(
            std::fs::read_to_string(extract_dir.join("nested").join("code.rs")).unwrap(),
            "fn ok() {}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
