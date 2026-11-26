//! Hybrid instant search engine: queries `texplorer-service` ($MFT + live USN Journal) first,
//! transparently falling back to a multi-threaded `rayon` filesystem walk without Admin privileges.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use rayon::prelude::*;
use texplorer_core::{EntryFlags, FileEntry, Location, SearchScope, classify_extension};
use texplorer_mft::{
    FLAG_IS_COMPRESSED, FLAG_IS_DIR, FLAG_IS_HIDDEN, FLAG_IS_READONLY, FLAG_IS_SYMLINK,
    FLAG_IS_SYSTEM, ParsedQuery, QueryCandidate, SearchHit, parse_query, search_via_service,
};

/// Result batch returned by either the MFT Index Service or the parallel fallback engine.
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub entries: Vec<FileEntry>,
    pub total_matches: usize,
    pub elapsed_ms: u64,
    pub from_mft: bool,
    pub truncated: bool,
}

/// Executes an Everything-style search across `scope` (using MFT Service when enabled and ready,
/// or parallel multi-threaded fallback walk otherwise).
pub fn execute_search(
    raw_query: &str,
    scope: &SearchScope,
    use_mft_service: bool,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<SearchOutcome, String> {
    let query = parse_query(raw_query);
    if query.is_empty() {
        return Ok(SearchOutcome {
            entries: Vec::new(),
            total_matches: 0,
            elapsed_ms: 0,
            from_mft: false,
            truncated: false,
        });
    }

    let limit = limit.clamp(1, 2_000);
    let scope_str = match scope {
        SearchScope::Global => None,
        SearchScope::Folder(dir) => Some(dir.to_string_lossy().into_owned()),
    };

    // 1. Instant path: query read-only NTFS MFT + USN Index Service via Named Pipe (< 10 ms).
    if use_mft_service
        && let Ok(resp) = search_via_service(raw_query, scope_str.clone(), limit)
    {
        let entries = resp.hits.into_iter().map(search_hit_to_entry).collect();
        return Ok(SearchOutcome {
            entries,
            total_matches: resp.total_matches,
            elapsed_ms: resp.elapsed_ms,
            from_mft: true,
            truncated: resp.truncated,
        });
    }

    // 2. Automatic zero-privilege fallback: multi-threaded parallel filesystem walk using `rayon`.
    parallel_fallback_search(&query, scope, limit, cancel)
}

fn search_hit_to_entry(hit: SearchHit) -> FileEntry {
    let mut flags = EntryFlags::empty();
    if (hit.flags & FLAG_IS_DIR) != 0 {
        flags |= EntryFlags::IS_DIR;
    }
    if (hit.flags & FLAG_IS_HIDDEN) != 0 {
        flags |= EntryFlags::IS_HIDDEN;
    }
    if (hit.flags & FLAG_IS_SYSTEM) != 0 {
        flags |= EntryFlags::IS_SYSTEM;
    }
    if (hit.flags & FLAG_IS_READONLY) != 0 {
        flags |= EntryFlags::IS_READONLY;
    }
    if (hit.flags & FLAG_IS_SYMLINK) != 0 {
        flags |= EntryFlags::IS_SYMLINK;
    }
    if (hit.flags & FLAG_IS_COMPRESSED) != 0 {
        flags |= EntryFlags::IS_COMPRESSED;
    }

    let is_dir = flags.contains(EntryFlags::IS_DIR);
    let is_symlink = flags.contains(EntryFlags::IS_SYMLINK);
    let extension = if is_dir {
        String::new()
    } else {
        hit.name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    };
    let (kind, type_label) = classify_extension(&extension, is_dir, is_symlink);

    FileEntry {
        name: hit.name,
        extension,
        location: Location::local(hit.full_path),
        size: if is_dir && hit.size == 0 {
            None
        } else {
            Some(hit.size)
        },
        modified_secs: hit.modified_secs,
        created_secs: hit.modified_secs,
        flags,
        kind,
        type_label,
    }
}

fn parallel_fallback_search(
    query: &ParsedQuery,
    scope: &SearchScope,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<SearchOutcome, String> {
    let started = Instant::now();
    let roots = resolve_fallback_roots(scope);
    let scope_lower = match scope {
        SearchScope::Global => None,
        SearchScope::Folder(dir) => Some(dir.to_string_lossy().replace('/', "\\").to_ascii_lowercase()),
    };

    let collect_cap = (limit * 2).min(4_000);
    let matches: Mutex<Vec<(u8, FileEntry)>> = Mutex::new(Vec::with_capacity(limit.min(256)));
    let total_found = AtomicUsize::new(0);
    let hit_cap_reached = AtomicBool::new(false);

    // Collect level-1 seed directories across all roots so `rayon` spreads work smoothly across CPU cores.
    let mut seed_dirs: Vec<PathBuf> = Vec::new();
    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        for item in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let path = item.path();
            if let Ok(meta) = item.metadata() {
                check_and_record(
                    &path,
                    &meta,
                    query,
                    scope_lower.as_deref(),
                    &matches,
                    &total_found,
                    collect_cap,
                    &hit_cap_reached,
                );
                if meta.is_dir() && !is_reparse_or_symlink(&meta) {
                    seed_dirs.push(path);
                }
            }
        }
    }

    seed_dirs.par_iter().for_each(|seed| {
        if cancel.load(Ordering::Relaxed) || hit_cap_reached.load(Ordering::Relaxed) {
            return;
        }
        walk_dir_iterative(
            seed,
            query,
            scope_lower.as_deref(),
            &matches,
            &total_found,
            collect_cap,
            &hit_cap_reached,
            cancel,
        );
    });

    let mut ranked = matches.into_inner().unwrap_or_default();
    ranked.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
    ranked.truncate(limit);

    let total_matches = total_found.load(Ordering::Relaxed);
    let entries: Vec<FileEntry> = ranked.into_iter().map(|(_, e)| e).collect();
    let truncated = total_matches > entries.len() || hit_cap_reached.load(Ordering::Relaxed);

    Ok(SearchOutcome {
        entries,
        total_matches,
        elapsed_ms: started.elapsed().as_millis() as u64,
        from_mft: false,
        truncated,
    })
}

#[allow(clippy::too_many_arguments)]
fn walk_dir_iterative(
    start: &Path,
    query: &ParsedQuery,
    scope_lower: Option<&str>,
    matches: &Mutex<Vec<(u8, FileEntry)>>,
    total_found: &AtomicUsize,
    collect_cap: usize,
    hit_cap_reached: &AtomicBool,
    cancel: &AtomicBool,
) {
    let mut stack: Vec<(PathBuf, u16)> = vec![(start.to_path_buf(), 1)];
    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || hit_cap_reached.load(Ordering::Relaxed) {
            return;
        }
        if depth > 64 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in rd.flatten() {
            if cancel.load(Ordering::Relaxed) || hit_cap_reached.load(Ordering::Relaxed) {
                return;
            }
            let path = item.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            check_and_record(
                &path,
                &meta,
                query,
                scope_lower,
                matches,
                total_found,
                collect_cap,
                hit_cap_reached,
            );
            if meta.is_dir() && !is_reparse_or_symlink(&meta) {
                stack.push((path, depth + 1));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_and_record(
    path: &Path,
    meta: &std::fs::Metadata,
    query: &ParsedQuery,
    scope_lower: Option<&str>,
    matches: &Mutex<Vec<(u8, FileEntry)>>,
    total_found: &AtomicUsize,
    collect_cap: usize,
    hit_cap_reached: &AtomicBool,
) {
    let Some(name_os) = path.file_name() else {
        return;
    };
    let name = name_os.to_string_lossy();
    let name_lower = name.to_ascii_lowercase();
    let is_dir = meta.is_dir();
    let size = if is_dir { 0 } else { meta.len() };

    if !query.matches_fast(QueryCandidate {
        name_lower: &name_lower,
        is_dir,
        size,
    }) {
        return;
    }

    if scope_lower.is_some()
        || !query.path_filters.is_empty()
        || !query.exclude_terms.is_empty()
    {
        let full_lower = path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        if !query.matches_full_path(&full_lower, scope_lower) {
            return;
        }
    }

    let prev = total_found.fetch_add(1, Ordering::Relaxed);
    if prev >= collect_cap {
        hit_cap_reached.store(true, Ordering::Relaxed);
        return;
    }

    let is_symlink = is_reparse_or_symlink(meta);
    let mut flags = EntryFlags::empty();
    if is_dir {
        flags |= EntryFlags::IS_DIR;
    }
    if is_symlink {
        flags |= EntryFlags::IS_SYMLINK;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        let attrs = meta.file_attributes();
        if attrs & 0x0001 != 0 {
            flags |= EntryFlags::IS_READONLY;
        }
        if attrs & 0x0002 != 0 {
            flags |= EntryFlags::IS_HIDDEN;
        }
        if attrs & 0x0004 != 0 {
            flags |= EntryFlags::IS_SYSTEM;
        }
        if attrs & 0x0800 != 0 {
            flags |= EntryFlags::IS_COMPRESSED;
        }
    }

    let extension = if is_dir {
        String::new()
    } else {
        name.rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    };
    let (kind, type_label) = classify_extension(&extension, is_dir, is_symlink);
    let modified_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let created_secs = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(modified_secs);

    let rank = query.relevance_rank(&name_lower, is_dir);
    let entry = FileEntry {
        name: name.into_owned(),
        extension,
        location: Location::local(path),
        size: if is_dir { None } else { Some(size) },
        modified_secs,
        created_secs,
        flags,
        kind,
        type_label,
    };

    if let Ok(mut guard) = matches.lock()
        && guard.len() < collect_cap
    {
        guard.push((rank, entry));
    }
}

fn resolve_fallback_roots(scope: &SearchScope) -> Vec<PathBuf> {
    match scope {
        SearchScope::Folder(dir) => vec![dir.clone()],
        SearchScope::Global => {
            #[cfg(target_os = "windows")]
            {
                use windows_sys::Win32::Storage::FileSystem::{
                    GetDriveTypeW, GetLogicalDriveStringsW,
                };
                const DRIVE_FIXED: u32 = 3;
                let mut buf = vec![0u16; 512];
                let len =
                    unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
                let mut roots = Vec::new();
                if len > 0 && len <= buf.len() {
                    for chunk in buf[..len].split(|&c| c == 0) {
                        if chunk.len() < 2 {
                            continue;
                        }
                        let s = String::from_utf16_lossy(chunk);
                        let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                        if unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_FIXED {
                            roots.push(PathBuf::from(s));
                        }
                    }
                }
                if !roots.is_empty() {
                    return roots;
                }
            }
            vec![PathBuf::from("/")]
        }
    }
}

fn is_reparse_or_symlink(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        (meta.file_attributes() & 0x0400) != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_search_filters_by_extension_and_size() {
        let root = std::env::temp_dir().join(format!("texplorer_search_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("alpha.rs"), vec![b'a'; 2048]).unwrap();
        std::fs::write(sub.join("tiny.rs"), b"hi").unwrap();
        std::fs::write(sub.join("notes.txt"), vec![b'b'; 4096]).unwrap();

        let cancel = AtomicBool::new(false);
        let res = execute_search(
            "ext:rs size:>1kb",
            &SearchScope::Folder(root.clone()),
            false,
            100,
            &cancel,
        )
        .unwrap();

        assert!(!res.from_mft);
        assert_eq!(res.entries.len(), 1);
        assert_eq!(res.entries[0].name, "alpha.rs");

        let _ = std::fs::remove_dir_all(&root);
    }
}
