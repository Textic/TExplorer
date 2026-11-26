//! Hybrid background folder-size engine:
//! 1. Queries `texplorer-service` for instant `< 1 ms` recursive NTFS folder totals.
//! 2. Falls back transparently to parallel `rayon` directory summation with symlink/junction
//!    loop protection and cooperative cancellation when the service is not running.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use texplorer_mft::get_folder_sizes_via_service;

/// Resolves recursive byte sizes for a batch of folder paths.
pub fn resolve_folder_sizes(
    folders: &[PathBuf],
    use_mft_service: bool,
    cancel: &AtomicBool,
) -> Vec<(PathBuf, u64)> {
    if folders.is_empty() || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }

    let mut resolved: HashMap<PathBuf, u64> = HashMap::with_capacity(folders.len());
    let mut unresolved: Vec<PathBuf> = Vec::new();

    if use_mft_service {
        let req_paths: Vec<String> = folders
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if let Ok(entries) = get_folder_sizes_via_service(req_paths) {
            for (folder, entry) in folders.iter().zip(entries) {
                if let Some(bytes) = entry.bytes {
                    resolved.insert(folder.clone(), bytes);
                } else {
                    unresolved.push(folder.clone());
                }
            }
        } else {
            unresolved.extend(folders.iter().cloned());
        }
    } else {
        unresolved.extend(folders.iter().cloned());
    }

    if !unresolved.is_empty() && !cancel.load(Ordering::Relaxed) {
        let computed: Vec<(PathBuf, u64)> = unresolved
            .into_par_iter()
            .filter_map(|dir| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let bytes = compute_folder_size_blocking(&dir, cancel)?;
                Some((dir, bytes))
            })
            .collect();
        for (dir, bytes) in computed {
            resolved.insert(dir, bytes);
        }
    }

    folders
        .iter()
        .filter_map(|p| resolved.get(p).map(|&b| (p.clone(), b)))
        .collect()
}

/// Computes the recursive logical size of `root` while skipping symlinks/reparse junctions.
pub fn compute_folder_size_blocking(root: &Path, cancel: &AtomicBool) -> Option<u64> {
    let mut total = 0u64;
    let mut stack: Vec<(PathBuf, u16)> = vec![(root.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if depth > 64 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let path = item.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if is_reparse_or_symlink(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push((path, depth + 1));
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Some(total)
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
    fn computes_recursive_folder_sizes_in_fallback_mode() {
        let root = std::env::temp_dir().join(format!("texplorer_fsize_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir_a = root.join("A");
        let dir_a_sub = dir_a.join("Sub");
        let dir_b = root.join("B");
        std::fs::create_dir_all(&dir_a_sub).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        std::fs::write(dir_a.join("one.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir_a_sub.join("two.bin"), vec![0u8; 2500]).unwrap();
        std::fs::write(dir_b.join("three.bin"), vec![0u8; 400]).unwrap();

        let cancel = AtomicBool::new(false);
        let sizes = resolve_folder_sizes(&[dir_a.clone(), dir_b.clone()], false, &cancel);
        assert_eq!(sizes, vec![(dir_a, 3500), (dir_b, 400)]);

        let _ = std::fs::remove_dir_all(&root);
    }
}
