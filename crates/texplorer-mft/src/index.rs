//! Compact in-RAM multi-volume NTFS index with live USN incremental updates,
//! fast Everything-style search, and O(1) folder size lookup.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::mft_raw::{MergedMftEntry, NTFS_ROOT_RECORD_ID};
use crate::protocol::{
    ExportedMftNode, FLAG_IS_DIR, FLAG_IS_SYMLINK, FolderSizeEntry, IndexServiceStatus,
    IndexedVolumeStatus, SearchHit, SearchResponse,
};
use crate::query::{ParsedQuery, QueryCandidate, parse_query};
use crate::usn::UsnDelta;

/// Compact node entry (~40 bytes + shared arena name slice).
#[derive(Debug, Clone, Copy)]
pub struct MftNode {
    pub parent_ref: u64,
    pub name_offset: u32,
    pub name_len: u16,
    pub lower_offset: u32,
    pub lower_len: u16,
    pub size: u64,
    pub modified_secs: i64,
    pub flags: u16,
}

impl MftNode {
    #[inline]
    pub fn is_dir(self) -> bool {
        (self.flags & FLAG_IS_DIR) != 0
    }

    #[inline]
    pub fn is_symlink(self) -> bool {
        (self.flags & FLAG_IS_SYMLINK) != 0
    }
}

/// Live recursive size/count aggregate maintained for every indexed directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirStats {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
}

/// In-memory index for a single NTFS volume (`C:`, `D:`, `X:`, etc.).
pub struct VolumeIndex {
    pub drive_letter: char,
    pub usn_journal_id: u64,
    pub next_usn: i64,
    pub usn_active: bool,
    pub nodes: HashMap<u64, MftNode>,
    pub children: HashMap<u64, Vec<u64>>,
    pub dir_sizes: HashMap<u64, DirStats>,
    pub name_arena: String,
    pub lower_arena: String,
}

impl VolumeIndex {
    /// Builds a compact `VolumeIndex` from merged `$MFT` records and computes recursive folder sizes in O(N).
    pub fn from_mft_entries(drive_letter: char, entries: Vec<MergedMftEntry>) -> Self {
        let upper = drive_letter.to_ascii_uppercase();
        let cap = entries.len().max(16);
        let mut idx = Self {
            drive_letter: upper,
            usn_journal_id: 0,
            next_usn: 0,
            usn_active: false,
            nodes: HashMap::with_capacity(cap),
            children: HashMap::with_capacity(cap / 8),
            dir_sizes: HashMap::with_capacity(cap / 8),
            name_arena: String::with_capacity(cap.saturating_mul(20)),
            lower_arena: String::with_capacity(cap.saturating_mul(20)),
        };

        // Ensure NTFS root directory (record #5) always exists.
        let root_name = format!("{upper}:");
        let root_node = idx.intern_node(
            NTFS_ROOT_RECORD_ID,
            &root_name,
            0,
            0,
            FLAG_IS_DIR,
        );
        idx.nodes.insert(NTFS_ROOT_RECORD_ID, root_node);

        for entry in entries {
            if entry.file_ref == NTFS_ROOT_RECORD_ID {
                if let Some(root) = idx.nodes.get_mut(&NTFS_ROOT_RECORD_ID) {
                    root.modified_secs = entry.modified_secs;
                }
                continue;
            }
            if entry.name.is_empty() || entry.parent_ref == entry.file_ref {
                continue;
            }
            let node = idx.intern_node(
                entry.parent_ref,
                &entry.name,
                entry.size,
                entry.modified_secs,
                entry.flags,
            );
            idx.nodes.insert(entry.file_ref, node);
            idx.children
                .entry(entry.parent_ref)
                .or_default()
                .push(entry.file_ref);
        }

        idx.recompute_all_dir_sizes();
        idx
    }

    fn intern_node(
        &mut self,
        parent_ref: u64,
        name: &str,
        size: u64,
        modified_secs: i64,
        flags: u16,
    ) -> MftNode {
        let name_offset = self.name_arena.len() as u32;
        self.name_arena.push_str(name);
        let name_len = name.len().min(u16::MAX as usize) as u16;

        let lower = name.to_ascii_lowercase();
        let lower_offset = self.lower_arena.len() as u32;
        self.lower_arena.push_str(&lower);
        let lower_len = lower.len().min(u16::MAX as usize) as u16;

        MftNode {
            parent_ref,
            name_offset,
            name_len,
            lower_offset,
            lower_len,
            size,
            modified_secs,
            flags,
        }
    }

    #[inline]
    pub fn node_name(&self, node: &MftNode) -> &str {
        let start = node.name_offset as usize;
        let end = start + node.name_len as usize;
        self.name_arena.get(start..end).unwrap_or("")
    }

    #[inline]
    pub fn node_name_lower(&self, node: &MftNode) -> &str {
        let start = node.lower_offset as usize;
        let end = start + node.lower_len as usize;
        self.lower_arena.get(start..end).unwrap_or("")
    }

    /// Computes bottom-up recursive `DirStats` starting from NTFS root `#5` (skipping symlinks/reparse points).
    fn recompute_all_dir_sizes(&mut self) {
        self.dir_sizes.clear();
        let mut order: Vec<u64> = Vec::with_capacity(self.children.len());
        let mut stack: Vec<(u64, u16)> = vec![(NTFS_ROOT_RECORD_ID, 0)];

        while let Some((dir_ref, depth)) = stack.pop() {
            order.push(dir_ref);
            if depth >= 128 {
                continue;
            }
            if let Some(kids) = self.children.get(&dir_ref) {
                for &kid_ref in kids {
                    if let Some(node) = self.nodes.get(&kid_ref)
                        && node.is_dir()
                        && !node.is_symlink()
                    {
                        stack.push((kid_ref, depth + 1));
                    }
                }
            }
        }

        // Process post-order (deepest directories first) to roll up totals cleanly.
        for &dir_ref in order.iter().rev() {
            let mut stats = DirStats::default();
            if let Some(kids) = self.children.get(&dir_ref) {
                for &kid_ref in kids {
                    let Some(node) = self.nodes.get(&kid_ref) else {
                        continue;
                    };
                    if node.is_dir() {
                        stats.folders = stats.folders.saturating_add(1);
                        if !node.is_symlink()
                            && let Some(sub) = self.dir_sizes.get(&kid_ref)
                        {
                            stats.bytes = stats.bytes.saturating_add(sub.bytes);
                            stats.files = stats.files.saturating_add(sub.files);
                            stats.folders = stats.folders.saturating_add(sub.folders);
                        }
                    } else if !node.is_symlink() {
                        stats.files = stats.files.saturating_add(1);
                        stats.bytes = stats.bytes.saturating_add(node.size);
                    }
                }
            }
            self.dir_sizes.insert(dir_ref, stats);
        }
    }

    /// Reconstructs the full Windows path (`X:\Dir\File.ext`) for `file_ref`.
    pub fn reconstruct_path(&self, file_ref: u64) -> Option<String> {
        if file_ref == NTFS_ROOT_RECORD_ID {
            return Some(format!("{}:\\", self.drive_letter));
        }

        let mut parts: Vec<&str> = Vec::with_capacity(12);
        let mut cur = file_ref;
        for _ in 0..128 {
            if cur == NTFS_ROOT_RECORD_ID {
                let mut total_len = 3 + parts.iter().map(|p| p.len() + 1).sum::<usize>();
                let mut out = String::with_capacity(total_len);
                out.push(self.drive_letter);
                out.push_str(":\\");
                for (i, part) in parts.iter().rev().enumerate() {
                    if i > 0 {
                        out.push('\\');
                    }
                    out.push_str(part);
                }
                let _ = &mut total_len;
                return Some(out);
            }
            let node = self.nodes.get(&cur)?;
            let name = self.node_name(node);
            if name.is_empty() || node.parent_ref == cur {
                return None;
            }
            parts.push(name);
            cur = node.parent_ref;
        }
        None
    }

    /// Resolves a local path on this volume (`X:\A\B`) to its MFT `file_ref` in O(depth).
    pub fn resolve_path_to_ref(&self, path: &str) -> Option<u64> {
        let trimmed = path.trim().trim_end_matches(['\\', '/']);
        let bytes = trimmed.as_bytes();
        if bytes.len() < 2
            || bytes[1] != b':'
            || (bytes[0] as char).to_ascii_uppercase() != self.drive_letter
        {
            return None;
        }
        let rest = trimmed[2..].trim_start_matches(['\\', '/']);
        if rest.is_empty() {
            return Some(NTFS_ROOT_RECORD_ID);
        }

        let mut cur = NTFS_ROOT_RECORD_ID;
        for seg in rest.split(['\\', '/']).filter(|s| !s.is_empty()) {
            let seg_lower = seg.to_ascii_lowercase();
            let kids = self.children.get(&cur)?;
            let next = kids.iter().copied().find(|&cid| {
                self.nodes
                    .get(&cid)
                    .is_some_and(|n| self.node_name_lower(n) == seg_lower)
            })?;
            cur = next;
        }
        Some(cur)
    }

    /// Returns instant cached recursive `DirStats` for a folder path on this volume.
    pub fn folder_stats(&self, path: &str) -> Option<DirStats> {
        let file_ref = self.resolve_path_to_ref(path)?;
        self.dir_sizes.get(&file_ref).copied()
    }

    /// Applies an incremental live USN Change Journal delta to nodes, children, and ancestor `dir_sizes`.
    pub fn apply_usn_delta(&mut self, delta: UsnDelta) {
        match delta {
            UsnDelta::Delete { file_ref } => {
                if file_ref == NTFS_ROOT_RECORD_ID {
                    return;
                }
                let Some(old) = self.nodes.remove(&file_ref) else {
                    return;
                };
                if let Some(kids) = self.children.get_mut(&old.parent_ref) {
                    kids.retain(|&id| id != file_ref);
                }
                if old.is_dir() {
                    let removed_sub = self.dir_sizes.remove(&file_ref).unwrap_or_default();
                    self.adjust_ancestors(
                        old.parent_ref,
                        -(removed_sub.bytes as i128),
                        -(removed_sub.files as i64),
                        -(removed_sub.folders as i64) - 1,
                    );
                } else if !old.is_symlink() {
                    self.adjust_ancestors(old.parent_ref, -(old.size as i128), -1, 0);
                }
            }
            UsnDelta::UpdateMetadata {
                file_ref,
                size,
                modified_secs,
            } => {
                let Some(node) = self.nodes.get_mut(&file_ref) else {
                    return;
                };
                if node.is_dir() || node.is_symlink() {
                    node.modified_secs = modified_secs;
                    return;
                }
                let old_size = node.size;
                let parent_ref = node.parent_ref;
                node.size = size;
                if modified_secs > 0 {
                    node.modified_secs = modified_secs;
                }
                let diff = size as i128 - old_size as i128;
                if diff != 0 {
                    self.adjust_ancestors(parent_ref, diff, 0, 0);
                }
            }
            UsnDelta::Upsert {
                file_ref,
                parent_ref,
                name,
                size,
                modified_secs,
                flags,
            } => {
                if file_ref == NTFS_ROOT_RECORD_ID || name.is_empty() || parent_ref == file_ref {
                    return;
                }
                let existing = self.nodes.get(&file_ref).copied();
                let resolved_size = if (flags & FLAG_IS_DIR) != 0 || (flags & FLAG_IS_SYMLINK) != 0
                {
                    0
                } else {
                    size.or_else(|| existing.map(|e| e.size)).unwrap_or(0)
                };

                let new_node = self.intern_node(
                    parent_ref,
                    &name,
                    resolved_size,
                    modified_secs,
                    flags,
                );
                self.nodes.insert(file_ref, new_node);

                if let Some(old) = existing {
                    if old.parent_ref != parent_ref {
                        if let Some(old_kids) = self.children.get_mut(&old.parent_ref) {
                            old_kids.retain(|&id| id != file_ref);
                        }
                        self.children.entry(parent_ref).or_default().push(file_ref);

                        if old.is_dir() {
                            let sub = self.dir_sizes.get(&file_ref).copied().unwrap_or_default();
                            self.adjust_ancestors(
                                old.parent_ref,
                                -(sub.bytes as i128),
                                -(sub.files as i64),
                                -(sub.folders as i64) - 1,
                            );
                            self.adjust_ancestors(
                                parent_ref,
                                sub.bytes as i128,
                                sub.files as i64,
                                sub.folders as i64 + 1,
                            );
                        } else if !old.is_symlink() {
                            self.adjust_ancestors(old.parent_ref, -(old.size as i128), -1, 0);
                            self.adjust_ancestors(parent_ref, resolved_size as i128, 1, 0);
                        }
                    } else if !old.is_dir() && !old.is_symlink() && resolved_size != old.size {
                        let diff = resolved_size as i128 - old.size as i128;
                        self.adjust_ancestors(parent_ref, diff, 0, 0);
                    }
                } else {
                    self.children.entry(parent_ref).or_default().push(file_ref);
                    if new_node.is_dir() {
                        self.dir_sizes.entry(file_ref).or_default();
                        self.adjust_ancestors(parent_ref, 0, 0, 1);
                    } else if !new_node.is_symlink() {
                        self.adjust_ancestors(parent_ref, resolved_size as i128, 1, 0);
                    }
                }
            }
        }
    }

    fn adjust_ancestors(
        &mut self,
        start_parent: u64,
        bytes_delta: i128,
        files_delta: i64,
        folders_delta: i64,
    ) {
        let mut cur = start_parent;
        for _ in 0..128 {
            let entry = self.dir_sizes.entry(cur).or_default();
            entry.bytes = (entry.bytes as i128 + bytes_delta).max(0) as u64;
            entry.files = (entry.files as i64 + files_delta).max(0) as u64;
            entry.folders = (entry.folders as i64 + folders_delta).max(0) as u64;

            if cur == NTFS_ROOT_RECORD_ID {
                break;
            }
            let Some(parent) = self.nodes.get(&cur).map(|n| n.parent_ref) else {
                break;
            };
            if parent == cur {
                break;
            }
            cur = parent;
        }
    }

    /// Searches this volume against a parsed Everything-style query, appending ranked candidates.
    fn search_into(
        &self,
        query: &ParsedQuery,
        scope_lower: Option<&str>,
        out: &mut Vec<(u8, SearchHit)>,
        total_matches: &mut usize,
        collect_cap: usize,
    ) {
        for (&file_ref, node) in &self.nodes {
            if file_ref == NTFS_ROOT_RECORD_ID {
                continue;
            }
            let is_dir = node.is_dir();
            let effective_size = if is_dir {
                self.dir_sizes.get(&file_ref).map(|s| s.bytes).unwrap_or(0)
            } else {
                node.size
            };
            let name_lower = self.node_name_lower(node);
            if !query.matches_fast(QueryCandidate {
                name_lower,
                is_dir,
                size: effective_size,
            }) {
                continue;
            }

            let Some(full_path) = self.reconstruct_path(file_ref) else {
                continue;
            };
            if scope_lower.is_some()
                || !query.path_filters.is_empty()
                || !query.exclude_terms.is_empty()
            {
                let path_lower = full_path.to_ascii_lowercase();
                if !query.matches_full_path(&path_lower, scope_lower) {
                    continue;
                }
            }

            *total_matches += 1;
            if out.len() < collect_cap {
                let rank = query.relevance_rank(name_lower, is_dir);
                out.push((
                    rank,
                    SearchHit {
                        name: self.node_name(node).to_string(),
                        full_path,
                        size: effective_size,
                        modified_secs: node.modified_secs,
                        flags: node.flags,
                    },
                ));
            }
        }
    }

    /// Exports the full volume (or a resolved subdirectory subtree) in topological DFS order so
    /// Disk Space Analyzer can reconstruct its arena tree in linear `O(N)` time without needing
    /// direct `$MFT` privileges. Every node's `parent_idx` strictly precedes its own stream index.
    pub fn export_dfs_nodes_from(
        &self,
        start_ref: u64,
        root_label: String,
        cluster_size: u64,
    ) -> Vec<ExportedMftNode> {
        let cluster = cluster_size.max(512);
        let est_cap = if start_ref == NTFS_ROOT_RECORD_ID {
            self.nodes.len()
        } else {
            self.dir_sizes
                .get(&start_ref)
                .map(|s| (s.files + s.folders + 1) as usize)
                .unwrap_or(256)
                .min(self.nodes.len())
        };
        let mut out = Vec::with_capacity(est_cap);
        let mut visited: HashSet<u64> = HashSet::with_capacity(est_cap);

        let root_modified = self
            .nodes
            .get(&start_ref)
            .map(|n| n.modified_secs)
            .unwrap_or(0);
        visited.insert(start_ref);
        out.push(ExportedMftNode {
            parent_idx: u32::MAX,
            logical_size: 0,
            allocated_size: 0,
            modified_secs: root_modified,
            flags: FLAG_IS_DIR,
            name: root_label,
        });

        let mut stack: Vec<(u64, u32, u16)> = vec![(start_ref, 0, 0)];
        while let Some((dir_ref, parent_stream_idx, depth)) = stack.pop() {
            if depth >= 128 {
                continue;
            }
            let Some(kids) = self.children.get(&dir_ref) else {
                continue;
            };
            for &kid_ref in kids {
                if !visited.insert(kid_ref) {
                    continue;
                }
                let Some(node) = self.nodes.get(&kid_ref) else {
                    continue;
                };
                let name = self.node_name(node);
                if name.is_empty() {
                    continue;
                }
                let is_dir = node.is_dir();
                let is_symlink = node.is_symlink();
                let (logical_size, allocated_size) = if is_dir || is_symlink || node.size == 0 {
                    (0, 0)
                } else {
                    let rem = node.size % cluster;
                    let alloc = if rem == 0 {
                        node.size
                    } else {
                        node.size.saturating_add(cluster - rem)
                    };
                    (node.size, alloc)
                };

                let stream_idx = out.len() as u32;
                out.push(ExportedMftNode {
                    parent_idx: parent_stream_idx,
                    logical_size,
                    allocated_size,
                    modified_secs: node.modified_secs,
                    flags: node.flags,
                    name: name.to_string(),
                });

                if is_dir && !is_symlink {
                    stack.push((kid_ref, stream_idx, depth + 1));
                }
            }
        }

        out
    }

    /// Exports the full in-RAM volume hierarchy from NTFS root `#5`.
    pub fn export_dfs_nodes(&self, cluster_size: u64) -> Vec<ExportedMftNode> {
        self.export_dfs_nodes_from(
            NTFS_ROOT_RECORD_ID,
            format!("{}:\\", self.drive_letter),
            cluster_size,
        )
    }
}

/// Multi-volume RAM index managed by `texplorer-service`.
#[derive(Default)]
pub struct MultiVolumeIndex {
    pub volumes: HashMap<char, VolumeIndex>,
    pub indexing: bool,
    pub ready: bool,
}

impl MultiVolumeIndex {
    pub fn insert_volume(&mut self, volume: VolumeIndex) {
        self.volumes
            .insert(volume.drive_letter.to_ascii_uppercase(), volume);
    }

    pub fn status(&self) -> IndexServiceStatus {
        let mut vols: Vec<IndexedVolumeStatus> = self
            .volumes
            .values()
            .map(|v| IndexedVolumeStatus {
                drive_letter: v.drive_letter,
                total_entries: v.nodes.len() as u64,
                usn_active: v.usn_active,
            })
            .collect();
        vols.sort_by_key(|v| v.drive_letter);
        let total_entries = vols.iter().map(|v| v.total_entries).sum();
        IndexServiceStatus {
            ready: self.ready,
            indexing: self.indexing,
            total_entries,
            volumes: vols,
        }
    }

    /// Executes an Everything-style search across all indexed NTFS volumes (or a scoped volume/subtree).
    pub fn search(&self, raw_query: &str, scope_path: Option<&str>, limit: usize) -> SearchResponse {
        let started = Instant::now();
        let query = parse_query(raw_query);
        if query.is_empty() {
            return SearchResponse {
                hits: Vec::new(),
                total_matches: 0,
                elapsed_ms: 0,
                from_mft: true,
                truncated: false,
            };
        }

        let scope_lower = scope_path
            .map(|s| s.trim().replace('/', "\\").to_ascii_lowercase())
            .filter(|s| !s.is_empty());
        let scoped_drive = scope_lower.as_deref().and_then(|s| {
            let b = s.as_bytes();
            if b.len() >= 2 && b[1] == b':' {
                Some((b[0] as char).to_ascii_uppercase())
            } else {
                None
            }
        });

        let limit = limit.clamp(1, 5_000);
        let collect_cap = (limit * 4).min(20_000);
        let mut ranked: Vec<(u8, SearchHit)> = Vec::with_capacity(limit.min(512));
        let mut total_matches = 0usize;

        for (&drive, vol) in &self.volumes {
            if let Some(target_drive) = scoped_drive
                && drive != target_drive
            {
                continue;
            }
            vol.search_into(
                &query,
                scope_lower.as_deref(),
                &mut ranked,
                &mut total_matches,
                collect_cap,
            );
        }

        ranked.sort_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then_with(|| a.name.len().cmp(&b.name.len()))
                .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
        });
        ranked.truncate(limit);

        let hits: Vec<SearchHit> = ranked.into_iter().map(|(_, h)| h).collect();
        let truncated = total_matches > hits.len();
        SearchResponse {
            hits,
            total_matches,
            elapsed_ms: started.elapsed().as_millis() as u64,
            from_mft: true,
            truncated,
        }
    }

    /// Resolves instant folder sizes for a batch of local directory paths.
    pub fn folder_sizes(&self, paths: &[String]) -> Vec<FolderSizeEntry> {
        paths
            .iter()
            .map(|path| {
                let bytes_drive = path.trim().as_bytes();
                let stats = if bytes_drive.len() >= 2 && bytes_drive[1] == b':' {
                    let drive = (bytes_drive[0] as char).to_ascii_uppercase();
                    self.volumes.get(&drive).and_then(|v| v.folder_stats(path))
                } else {
                    None
                };
                match stats {
                    Some(st) => FolderSizeEntry {
                        path: path.clone(),
                        bytes: Some(st.bytes),
                        files: st.files,
                        folders: st.folders,
                    },
                    None => FolderSizeEntry {
                        path: path.clone(),
                        bytes: None,
                        files: 0,
                        folders: 0,
                    },
                }
            })
            .collect()
    }

    /// Exports an indexed NTFS volume (or a scoped subfolder subtree) in DFS stream order.
    pub fn export_volume_tree(
        &self,
        drive_letter: char,
        scope_path: Option<&str>,
        cluster_size: u64,
    ) -> Result<Vec<ExportedMftNode>, String> {
        let upper = drive_letter.to_ascii_uppercase();
        let Some(vol) = self.volumes.get(&upper) else {
            if !self.ready && self.indexing {
                return Err("NTFS MFT index is still initializing...".to_string());
            }
            return Err(format!("Volume {upper}: is not in the MFT index."));
        };
        if let Some(scope) = scope_path
            && !scope.trim().is_empty()
        {
            let start_ref = vol
                .resolve_path_to_ref(scope)
                .ok_or_else(|| format!("Folder \"{scope}\" was not found in MFT index."))?;
            let Some(node) = vol.nodes.get(&start_ref) else {
                return Err(format!("Folder \"{scope}\" is missing in MFT index."));
            };
            if !node.is_dir() || node.is_symlink() {
                return Err(format!("Path \"{scope}\" is not a direct NTFS directory."));
            }
            let root_label = if start_ref == NTFS_ROOT_RECORD_ID {
                format!("{upper}:\\")
            } else {
                vol.node_name(node).to_string()
            };
            return Ok(vol.export_dfs_nodes_from(start_ref, root_label, cluster_size));
        }
        Ok(vol.export_dfs_nodes(cluster_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<MergedMftEntry> {
        vec![
            MergedMftEntry {
                file_ref: 10,
                parent_ref: NTFS_ROOT_RECORD_ID,
                name: "Projects".to_string(),
                size: 0,
                modified_secs: 100,
                flags: FLAG_IS_DIR,
            },
            MergedMftEntry {
                file_ref: 11,
                parent_ref: 10,
                name: "TExplorer".to_string(),
                size: 0,
                modified_secs: 101,
                flags: FLAG_IS_DIR,
            },
            MergedMftEntry {
                file_ref: 12,
                parent_ref: 11,
                name: "main.rs".to_string(),
                size: 4096,
                modified_secs: 200,
                flags: 0,
            },
            MergedMftEntry {
                file_ref: 13,
                parent_ref: 11,
                name: "Cargo.toml".to_string(),
                size: 1024,
                modified_secs: 201,
                flags: 0,
            },
        ]
    }

    #[test]
    fn computes_paths_folder_sizes_queries_and_live_usn_deltas() {
        let mut vol = VolumeIndex::from_mft_entries('X', sample_entries());
        assert_eq!(
            vol.reconstruct_path(12).as_deref(),
            Some(r"X:\Projects\TExplorer\main.rs")
        );
        assert_eq!(vol.folder_stats(r"X:\Projects").unwrap().bytes, 5120);
        assert_eq!(vol.folder_stats(r"X:\Projects\TExplorer").unwrap().files, 2);

        // Apply live USN create + resize + delete and verify folder sizes stay exact.
        vol.apply_usn_delta(UsnDelta::Upsert {
            file_ref: 14,
            parent_ref: 11,
            name: "engine.rs".to_string(),
            size: Some(2000),
            modified_secs: 300,
            flags: 0,
        });
        assert_eq!(vol.folder_stats(r"X:\Projects").unwrap().bytes, 7120);
        assert_eq!(vol.folder_stats(r"X:\Projects\TExplorer").unwrap().files, 3);

        vol.apply_usn_delta(UsnDelta::Delete { file_ref: 13 });
        assert_eq!(vol.folder_stats(r"X:\Projects").unwrap().bytes, 6096);
        assert_eq!(vol.folder_stats(r"X:\Projects\TExplorer").unwrap().files, 2);

        let mut multi = MultiVolumeIndex {
            ready: true,
            ..Default::default()
        };
        multi.insert_volume(vol);

        let res = multi.search("ext:rs size:>1kb", Some(r"X:\Projects"), 50);
        assert_eq!(res.hits.len(), 2);
        assert!(res.from_mft);

        let exported = multi.export_volume_tree('x', None, 4096).unwrap();
        assert_eq!(exported[0].parent_idx, u32::MAX);
        assert_eq!(exported[0].name, r"X:\");
        assert!(exported.len() >= 4);
        for (idx, node) in exported.iter().enumerate().skip(1) {
            assert!((node.parent_idx as usize) < idx);
        }

        let scoped = multi
            .export_volume_tree('x', Some(r"X:\Projects\TExplorer"), 4096)
            .unwrap();
        assert_eq!(scoped[0].parent_idx, u32::MAX);
        assert_eq!(scoped[0].name, "TExplorer");
        assert_eq!(scoped.len(), 3);
    }
}
