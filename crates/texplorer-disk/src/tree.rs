use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bitflags::bitflags;

/// Compact 32-bit index into `DiskTree::nodes` (supports up to ~4 billion nodes).
pub type NodeId = u32;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct NodeFlags: u16 {
        const DIR           = 1 << 0;
        const HIDDEN        = 1 << 1;
        const SYSTEM        = 1 << 2;
        const SYMLINK       = 1 << 3;
        const REPARSE       = 1 << 4;
        const ACCESS_DENIED = 1 << 5;
        const COMPRESSED    = 1 << 6;
        const SPARSE        = 1 << 7;
    }
}

/// Arena-allocated filesystem node with intrusive child/sibling linked list.
#[derive(Debug, Clone)]
pub struct FileNode {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub name: Arc<str>,
    /// Interned lowercase extension ID (`0` = no extension / directory).
    pub ext_id: u16,
    /// Logical byte size (for directories: recursive sum of descendant files).
    pub logical_size: u64,
    /// Allocated byte size on disk (for directories: recursive sum of descendant files).
    pub allocated_size: u64,
    /// Last modification timestamp as Unix epoch seconds.
    pub modified_ts: i64,
    /// Recursive descendant file count (0 for files).
    pub file_count: u32,
    /// Recursive descendant subdirectory count (0 for files).
    pub dir_count: u32,
    pub flags: NodeFlags,
    /// Soft-delete tombstone set when the user moves an item to the Recycle Bin.
    pub removed: bool,
}

impl FileNode {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags.contains(NodeFlags::DIR)
    }

    #[inline]
    pub fn is_hidden_or_system(&self) -> bool {
        self.flags.intersects(NodeFlags::HIDDEN | NodeFlags::SYSTEM)
    }

    #[inline]
    pub fn is_access_denied(&self) -> bool {
        self.flags.contains(NodeFlags::ACCESS_DENIED)
    }

    #[inline]
    pub fn is_symlink(&self) -> bool {
        self.flags.intersects(NodeFlags::SYMLINK | NodeFlags::REPARSE)
    }

    #[inline]
    pub fn total_items(&self) -> u64 {
        u64::from(self.file_count) + u64::from(self.dir_count)
    }
}

/// Engine used to build a `DiskTree` snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanEngineKind {
    MftService,
    NtfsMft,
    #[default]
    ParallelWalk,
    Hybrid,
}

impl ScanEngineKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::MftService => "MFT Service",
            Self::NtfsMft => "NTFS MFT",
            Self::ParallelWalk => "Parallel Walk",
            Self::Hybrid => "MFT + Walk",
        }
    }
}

/// Sortable columns in the Disk Space Analyzer tree table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiskSortColumn {
    Name,
    #[default]
    LogicalSize,
    AllocatedSize,
    PercentOfParent,
    ItemCount,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSortSpec {
    pub column: DiskSortColumn,
    pub descending: bool,
}

impl Default for DiskSortSpec {
    fn default() -> Self {
        Self {
            column: DiskSortColumn::LogicalSize,
            descending: true,
        }
    }
}

/// Flattened visible row produced by traversing expanded directories in `DiskTree`.
#[derive(Debug, Clone, Copy)]
pub struct VisibleRow {
    pub id: NodeId,
    pub depth: u16,
    pub has_children: bool,
    pub is_expanded: bool,
    pub percent_of_parent: f32,
}

/// Aggregated statistics for a single file extension across the scanned tree.
#[derive(Debug, Clone)]
pub struct ExtensionStat {
    pub ext: String,
    pub logical_size: u64,
    pub allocated_size: u64,
    pub file_count: u64,
    pub percent_of_total: f32,
}

/// Interner mapping lowercase extension strings (`"mp4"`, `"rs"`, ...) to compact `u16` IDs.
#[derive(Debug, Clone)]
pub struct ExtInterner {
    to_id: HashMap<String, u16>,
    strings: Vec<String>,
}

impl Default for ExtInterner {
    fn default() -> Self {
        Self {
            to_id: HashMap::new(),
            strings: vec![String::new()],
        }
    }
}

impl ExtInterner {
    pub fn intern(&mut self, ext: &str) -> u16 {
        if ext.is_empty() {
            return 0;
        }
        let lower = ext.to_ascii_lowercase();
        if let Some(&id) = self.to_id.get(&lower) {
            return id;
        }
        if self.strings.len() >= u16::MAX as usize {
            return 0;
        }
        let id = self.strings.len() as u16;
        self.strings.push(lower.clone());
        self.to_id.insert(lower, id);
        id
    }

    #[inline]
    pub fn resolve(&self, id: u16) -> &str {
        self.strings
            .get(usize::from(id))
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// Arena-backed filesystem snapshot for Disk Space Analyzer.
#[derive(Debug, Clone)]
pub struct DiskTree {
    pub nodes: Vec<FileNode>,
    pub root: NodeId,
    /// Base filesystem path corresponding to `root`. Empty for multi-drive virtual roots (`This PC`).
    pub root_path: PathBuf,
    pub is_virtual_multi_root: bool,
    pub engine: ScanEngineKind,
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_denied: u64,
    pub extensions: ExtInterner,
}

impl DiskTree {
    pub fn new_root(
        name: impl Into<Arc<str>>,
        root_path: PathBuf,
        is_virtual_multi_root: bool,
        engine: ScanEngineKind,
    ) -> Self {
        let root_node = FileNode {
            parent: None,
            first_child: None,
            next_sibling: None,
            name: name.into(),
            ext_id: 0,
            logical_size: 0,
            allocated_size: 0,
            modified_ts: 0,
            file_count: 0,
            dir_count: 0,
            flags: NodeFlags::DIR,
            removed: false,
        };
        Self {
            nodes: vec![root_node],
            root: 0,
            root_path,
            is_virtual_multi_root,
            engine,
            total_files: 0,
            total_dirs: 0,
            total_denied: 0,
            extensions: ExtInterner::default(),
        }
    }

    /// Reserves space in the node arena to minimize reallocations on large volumes.
    pub fn reserve_nodes(&mut self, additional: usize) {
        self.nodes.reserve(additional);
    }

    /// Appends a child node under `parent_id` and links it into the parent's sibling chain.
    #[allow(clippy::too_many_arguments)]
    pub fn add_node(
        &mut self,
        parent_id: NodeId,
        name: impl Into<Arc<str>>,
        logical_size: u64,
        allocated_size: u64,
        modified_ts: i64,
        flags: NodeFlags,
    ) -> NodeId {
        let name_arc: Arc<str> = name.into();
        let ext_id = if flags.contains(NodeFlags::DIR) {
            0
        } else {
            extract_extension(&name_arc)
                .map(|e| self.extensions.intern(e))
                .unwrap_or(0)
        };

        let id = self.nodes.len() as NodeId;
        let prev_first = self.nodes[parent_id as usize].first_child;
        self.nodes[parent_id as usize].first_child = Some(id);

        self.nodes.push(FileNode {
            parent: Some(parent_id),
            first_child: None,
            next_sibling: prev_first,
            name: name_arc,
            ext_id,
            logical_size,
            allocated_size,
            modified_ts,
            file_count: 0,
            dir_count: 0,
            flags,
            removed: false,
        });
        id
    }

    #[inline]
    pub fn get(&self, id: NodeId) -> Option<&FileNode> {
        self.nodes.get(id as usize)
    }

    #[inline]
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut FileNode> {
        self.nodes.get_mut(id as usize)
    }

    #[inline]
    pub fn ext_str(&self, id: NodeId) -> &str {
        self.get(id)
            .map(|n| self.extensions.resolve(n.ext_id))
            .unwrap_or("")
    }

    /// Iterates direct non-removed child IDs of `parent_id`.
    pub fn children(&self, parent_id: NodeId) -> ChildIter<'_> {
        let next = self.get(parent_id).and_then(|n| n.first_child);
        ChildIter {
            nodes: &self.nodes,
            next,
        }
    }

    /// Reconstructs the full OS path for `id`. Returns `None` for the virtual multi-drive root.
    pub fn full_path(&self, id: NodeId) -> Option<PathBuf> {
        if id == self.root {
            if self.is_virtual_multi_root {
                return None;
            }
            return Some(self.root_path.clone());
        }

        let mut segments: Vec<&str> = Vec::with_capacity(12);
        let mut cursor = Some(id);
        let mut guard = 0u32;
        while let Some(cid) = cursor {
            guard += 1;
            if guard > 256 {
                break;
            }
            let node = self.get(cid)?;
            if cid == self.root {
                break;
            }
            segments.push(&node.name);
            cursor = node.parent;
        }
        segments.reverse();

        if self.is_virtual_multi_root {
            let first = segments.first()?;
            let mut path = PathBuf::from(normalize_drive_segment(first));
            for seg in segments.iter().skip(1) {
                path.push(seg);
            }
            Some(path)
        } else {
            let mut path = self.root_path.clone();
            for seg in segments {
                path.push(seg);
            }
            Some(path)
        }
    }

    /// Computes percentage of parent's logical size (`0.0..=100.0`).
    pub fn percent_of_parent(&self, id: NodeId) -> f32 {
        let Some(node) = self.get(id) else {
            return 0.0;
        };
        let Some(parent_id) = node.parent else {
            return 100.0;
        };
        let Some(parent) = self.get(parent_id) else {
            return 0.0;
        };
        if parent.logical_size == 0 {
            0.0
        } else {
            ((node.logical_size as f64 / parent.logical_size as f64) * 100.0).clamp(0.0, 100.0)
                as f32
        }
    }

    /// Computes bottom-up recursive logical/allocated sizes, file/dir counts, and totals in O(N).
    pub fn finalize_rollups(&mut self) {
        if self.nodes.is_empty() {
            return;
        }

        let mut order: Vec<NodeId> = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<NodeId> = Vec::with_capacity(256);
        stack.push(self.root);

        while let Some(id) = stack.pop() {
            order.push(id);
            let mut child = self.nodes[id as usize].first_child;
            while let Some(cid) = child {
                let next = self.nodes[cid as usize].next_sibling;
                if !self.nodes[cid as usize].removed {
                    stack.push(cid);
                }
                child = next;
            }
        }

        let mut total_files = 0u64;
        let mut total_dirs = 0u64;
        let mut total_denied = 0u64;

        for &id in order.iter().rev() {
            let node_flags = self.nodes[id as usize].flags;
            if node_flags.contains(NodeFlags::ACCESS_DENIED) {
                total_denied += 1;
            }
            if !node_flags.contains(NodeFlags::DIR) {
                total_files += 1;
                continue;
            }
            if id != self.root {
                total_dirs += 1;
            }

            let mut sum_logical = 0u64;
            let mut sum_allocated = 0u64;
            let mut sum_files = 0u32;
            let mut sum_dirs = 0u32;
            let mut latest_ts = self.nodes[id as usize].modified_ts;

            let mut child = self.nodes[id as usize].first_child;
            while let Some(cid) = child {
                let c = &self.nodes[cid as usize];
                let next = c.next_sibling;
                if !c.removed {
                    sum_logical = sum_logical.saturating_add(c.logical_size);
                    sum_allocated = sum_allocated.saturating_add(c.allocated_size);
                    if c.modified_ts > latest_ts {
                        latest_ts = c.modified_ts;
                    }
                    if c.is_dir() {
                        sum_dirs = sum_dirs.saturating_add(1).saturating_add(c.dir_count);
                        sum_files = sum_files.saturating_add(c.file_count);
                    } else {
                        sum_files = sum_files.saturating_add(1);
                    }
                }
                child = next;
            }

            let dir = &mut self.nodes[id as usize];
            dir.logical_size = sum_logical;
            dir.allocated_size = sum_allocated;
            dir.file_count = sum_files;
            dir.dir_count = sum_dirs;
            dir.modified_ts = latest_ts;
        }

        self.total_files = total_files;
        self.total_dirs = total_dirs;
        self.total_denied = total_denied;
    }

    /// Flattens expanded nodes into display rows sorted per `sort`, honoring hidden/filter options.
    pub fn flatten_visible(
        &self,
        expanded: &HashSet<NodeId>,
        sort: DiskSortSpec,
        show_hidden: bool,
        filter_query: &str,
        denied_only: bool,
    ) -> Vec<VisibleRow> {
        let needle = filter_query.trim().to_ascii_lowercase();
        if !needle.is_empty() || denied_only {
            return self.flatten_filtered(&needle, denied_only, show_hidden, sort);
        }

        let mut rows = Vec::with_capacity(512);
        let mut stack: Vec<(NodeId, u16)> = vec![(self.root, 0)];

        while let Some((id, depth)) = stack.pop() {
            let Some(node) = self.get(id) else {
                continue;
            };
            if node.removed {
                continue;
            }
            if id != self.root && !show_hidden && node.is_hidden_or_system() {
                continue;
            }

            let has_children = node.is_dir()
                && self
                    .children(id)
                    .any(|cid| show_hidden || !self.nodes[cid as usize].is_hidden_or_system());
            let is_expanded = has_children && expanded.contains(&id);

            rows.push(VisibleRow {
                id,
                depth,
                has_children,
                is_expanded,
                percent_of_parent: self.percent_of_parent(id),
            });

            if is_expanded {
                let mut kids: Vec<NodeId> = self
                    .children(id)
                    .filter(|&cid| show_hidden || !self.nodes[cid as usize].is_hidden_or_system())
                    .collect();
                self.sort_ids(&mut kids, sort);
                for &cid in kids.iter().rev() {
                    stack.push((cid, depth.saturating_add(1)));
                }
            }
        }

        rows
    }

    fn flatten_filtered(
        &self,
        needle: &str,
        denied_only: bool,
        show_hidden: bool,
        sort: DiskSortSpec,
    ) -> Vec<VisibleRow> {
        let mut matched: Vec<NodeId> = Vec::with_capacity(256);
        let mut stack = vec![self.root];

        while let Some(id) = stack.pop() {
            let Some(node) = self.get(id) else {
                continue;
            };
            if node.removed {
                continue;
            }
            if id != self.root && !show_hidden && node.is_hidden_or_system() {
                continue;
            }

            let mut pass = id != self.root;
            if pass && denied_only && !node.is_access_denied() {
                pass = false;
            }
            if pass && !needle.is_empty() && !node.name.to_ascii_lowercase().contains(needle) {
                pass = false;
            }
            if pass {
                matched.push(id);
            }

            if node.is_dir() {
                for cid in self.children(id) {
                    stack.push(cid);
                }
            }
        }

        self.sort_ids(&mut matched, sort);
        matched.truncate(20_000);
        matched
            .into_iter()
            .map(|id| VisibleRow {
                id,
                depth: 0,
                has_children: false,
                is_expanded: false,
                percent_of_parent: self.percent_of_parent(id),
            })
            .collect()
    }

    /// Returns up to `limit` largest files matching optional substring, extension, and denied filters.
    pub fn top_files(
        &self,
        limit: usize,
        show_hidden: bool,
        filter_query: &str,
        ext_filter: Option<&str>,
        denied_only: bool,
    ) -> Vec<NodeId> {
        let needle = filter_query.trim().to_ascii_lowercase();
        let ext_needle = ext_filter
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty());

        let mut files: Vec<NodeId> = Vec::with_capacity(limit.min(4096));
        let mut stack = vec![self.root];

        while let Some(id) = stack.pop() {
            let Some(node) = self.get(id) else {
                continue;
            };
            if node.removed {
                continue;
            }
            if id != self.root && !show_hidden && node.is_hidden_or_system() {
                continue;
            }

            if node.is_dir() {
                if denied_only
                    && node.is_access_denied()
                    && id != self.root
                    && (needle.is_empty() || node.name.to_ascii_lowercase().contains(&needle))
                {
                    files.push(id);
                }
                for cid in self.children(id) {
                    stack.push(cid);
                }
            } else {
                if denied_only && !node.is_access_denied() {
                    continue;
                }
                if let Some(ref ext) = ext_needle
                    && self.extensions.resolve(node.ext_id) != ext.as_str()
                {
                    continue;
                }
                if !needle.is_empty() && !node.name.to_ascii_lowercase().contains(&needle) {
                    continue;
                }
                files.push(id);
            }
        }

        if files.len() > limit {
            files.select_nth_unstable_by(limit, |&a, &b| {
                self.nodes[b as usize]
                    .logical_size
                    .cmp(&self.nodes[a as usize].logical_size)
                    .then_with(|| self.nodes[a as usize].name.cmp(&self.nodes[b as usize].name))
            });
            files.truncate(limit);
        }
        files.sort_unstable_by(|&a, &b| {
            self.nodes[b as usize]
                .logical_size
                .cmp(&self.nodes[a as usize].logical_size)
                .then_with(|| self.nodes[a as usize].name.cmp(&self.nodes[b as usize].name))
        });
        files
    }

    /// Computes per-extension disk usage sorted by descending logical size.
    pub fn extension_stats(&self, show_hidden: bool) -> Vec<ExtensionStat> {
        let mut buckets: HashMap<u16, (u64, u64, u64)> = HashMap::new();
        let mut total_bytes = 0u64;
        let mut stack = vec![self.root];

        while let Some(id) = stack.pop() {
            let Some(node) = self.get(id) else {
                continue;
            };
            if node.removed {
                continue;
            }
            if id != self.root && !show_hidden && node.is_hidden_or_system() {
                continue;
            }
            if node.is_dir() {
                for cid in self.children(id) {
                    stack.push(cid);
                }
            } else {
                let entry = buckets.entry(node.ext_id).or_insert((0, 0, 0));
                entry.0 = entry.0.saturating_add(node.logical_size);
                entry.1 = entry.1.saturating_add(node.allocated_size);
                entry.2 = entry.2.saturating_add(1);
                total_bytes = total_bytes.saturating_add(node.logical_size);
            }
        }

        let mut stats: Vec<ExtensionStat> = buckets
            .into_iter()
            .map(|(ext_id, (logical_size, allocated_size, file_count))| {
                let raw = self.extensions.resolve(ext_id);
                let ext = if raw.is_empty() {
                    "(no extension)".to_string()
                } else {
                    format!(".{raw}")
                };
                let percent_of_total = if total_bytes == 0 {
                    0.0
                } else {
                    (logical_size as f64 / total_bytes as f64 * 100.0).clamp(0.0, 100.0) as f32
                };
                ExtensionStat {
                    ext,
                    logical_size,
                    allocated_size,
                    file_count,
                    percent_of_total,
                }
            })
            .collect();

        stats.sort_unstable_by(|a, b| {
            b.logical_size
                .cmp(&a.logical_size)
                .then_with(|| b.file_count.cmp(&a.file_count))
                .then_with(|| a.ext.cmp(&b.ext))
        });
        stats
    }

    /// Marks `id` and all its descendants as removed and subtracts their byte/item totals
    /// from every ancestor immediately in O(subtree + depth) without re-scanning disk.
    pub fn remove_subtree_soft(&mut self, id: NodeId) -> bool {
        if id == self.root {
            return false;
        }
        let Some(node) = self.get(id) else {
            return false;
        };
        if node.removed {
            return false;
        }

        let logical_delta = node.logical_size;
        let allocated_delta = node.allocated_size;
        let is_dir = node.is_dir();
        let files_delta = if is_dir { node.file_count } else { 1 };
        let dirs_delta = if is_dir {
            node.dir_count.saturating_add(1)
        } else {
            0
        };
        let parent_start = node.parent;

        let mut stack = vec![id];
        let mut denied_delta = 0u64;
        while let Some(cur) = stack.pop() {
            let n = &mut self.nodes[cur as usize];
            if n.removed {
                continue;
            }
            n.removed = true;
            if n.flags.contains(NodeFlags::ACCESS_DENIED) {
                denied_delta += 1;
            }
            let mut child = n.first_child;
            while let Some(cid) = child {
                let next = self.nodes[cid as usize].next_sibling;
                stack.push(cid);
                child = next;
            }
        }

        let mut cursor = parent_start;
        let mut guard = 0u32;
        while let Some(pid) = cursor {
            guard += 1;
            if guard > 256 {
                break;
            }
            let p = &mut self.nodes[pid as usize];
            p.logical_size = p.logical_size.saturating_sub(logical_delta);
            p.allocated_size = p.allocated_size.saturating_sub(allocated_delta);
            p.file_count = p.file_count.saturating_sub(files_delta);
            p.dir_count = p.dir_count.saturating_sub(dirs_delta);
            cursor = p.parent;
        }

        self.total_files = self.total_files.saturating_sub(u64::from(files_delta));
        self.total_dirs = self.total_dirs.saturating_sub(u64::from(dirs_delta));
        self.total_denied = self.total_denied.saturating_sub(denied_delta);
        true
    }

    /// Resolves a filesystem path to its `NodeId` inside this tree (case-insensitive on Windows).
    pub fn find_node_by_path(&self, target: &Path) -> Option<NodeId> {
        let mut rel_segments: Vec<String> = Vec::new();
        if self.is_virtual_multi_root {
            let raw = target.to_string_lossy();
            let trimmed = raw.trim_end_matches(['\\', '/']);
            if trimmed.is_empty() {
                return Some(self.root);
            }
            let parts: Vec<&str> = trimmed.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
            if parts.is_empty() {
                return Some(self.root);
            }
            let drive_part = parts[0];
            let mut drive_child = None;
            for cid in self.children(self.root) {
                let name = self.nodes[cid as usize]
                    .name
                    .trim_end_matches(['\\', '/']);
                if name.eq_ignore_ascii_case(drive_part) {
                    drive_child = Some(cid);
                    break;
                }
            }
            let mut cur = drive_child?;
            for seg in parts.iter().skip(1) {
                let next = self.find_child_by_name(cur, seg)?;
                cur = next;
            }
            return Some(cur);
        }

        let root_str = self
            .root_path
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string();
        let target_str = target
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string();

        if eq_path_str(&root_str, &target_str) {
            return Some(self.root);
        }

        let prefix = format!(
            "{}{}",
            root_str,
            if cfg!(target_os = "windows") { '\\' } else { '/' }
        );
        let suffix = strip_path_prefix(&target_str, &prefix)?;
        for seg in suffix.split(['\\', '/']).filter(|s| !s.is_empty()) {
            rel_segments.push(seg.to_string());
        }

        let mut cur = self.root;
        for seg in &rel_segments {
            cur = self.find_child_by_name(cur, seg)?;
        }
        Some(cur)
    }

    fn find_child_by_name(&self, parent_id: NodeId, seg: &str) -> Option<NodeId> {
        self.children(parent_id)
            .find(|&cid| eq_path_str(&self.nodes[cid as usize].name, seg))
    }

    pub fn sort_ids(&self, ids: &mut [NodeId], sort: DiskSortSpec) {
        ids.sort_unstable_by(|&a, &b| {
            let na = &self.nodes[a as usize];
            let nb = &self.nodes[b as usize];

            let ord = match sort.column {
                DiskSortColumn::Name => na
                    .name
                    .to_ascii_lowercase()
                    .cmp(&nb.name.to_ascii_lowercase()),
                DiskSortColumn::LogicalSize | DiskSortColumn::PercentOfParent => {
                    na.logical_size.cmp(&nb.logical_size)
                }
                DiskSortColumn::AllocatedSize => na.allocated_size.cmp(&nb.allocated_size),
                DiskSortColumn::ItemCount => na.total_items().cmp(&nb.total_items()),
                DiskSortColumn::Modified => na.modified_ts.cmp(&nb.modified_ts),
            };

            let ord = if sort.descending { ord.reverse() } else { ord };
            ord.then_with(|| nb.logical_size.cmp(&na.logical_size))
                .then_with(|| {
                    na.name
                        .to_ascii_lowercase()
                        .cmp(&nb.name.to_ascii_lowercase())
                })
        });
    }
}

pub struct ChildIter<'a> {
    nodes: &'a [FileNode],
    next: Option<NodeId>,
}

impl Iterator for ChildIter<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(id) = self.next {
            let node = &self.nodes[id as usize];
            self.next = node.next_sibling;
            if !node.removed {
                return Some(id);
            }
        }
        None
    }
}

fn extract_extension(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None;
    }
    let ext = &name[dot + 1..];
    if ext.len() > 24 || ext.contains([' ', '\\', '/']) {
        return None;
    }
    Some(ext)
}

fn normalize_drive_segment(seg: &str) -> String {
    let trimmed = seg.trim_end_matches(['\\', '/']);
    if cfg!(target_os = "windows") && trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

#[inline]
fn eq_path_str(a: &str, b: &str) -> bool {
    if cfg!(target_os = "windows") {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn strip_path_prefix<'a>(target: &'a str, prefix: &str) -> Option<&'a str> {
    if target.len() < prefix.len() {
        return None;
    }
    if !target.is_char_boundary(prefix.len()) {
        return None;
    }
    let (head, tail) = target.split_at(prefix.len());
    if eq_path_str(head, prefix) {
        Some(tail)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolls_up_sizes_counts_and_soft_deletes() {
        let mut tree = DiskTree::new_root(
            "C:\\",
            PathBuf::from("C:\\"),
            false,
            ScanEngineKind::ParallelWalk,
        );
        let dir_a = tree.add_node(tree.root, "Windows", 0, 0, 100, NodeFlags::DIR);
        let dir_b = tree.add_node(tree.root, "Games", 0, 0, 200, NodeFlags::DIR);
        let file1 = tree.add_node(dir_a, "kernel.dll", 1_000, 4_096, 150, NodeFlags::empty());
        let file2 = tree.add_node(dir_b, "movie.mp4", 9_000, 12_288, 250, NodeFlags::empty());

        tree.finalize_rollups();

        assert_eq!(tree.get(tree.root).unwrap().logical_size, 10_000);
        assert_eq!(tree.get(tree.root).unwrap().allocated_size, 16_384);
        assert_eq!(tree.total_files, 2);
        assert_eq!(tree.total_dirs, 2);
        assert_eq!(
            tree.full_path(file1).unwrap(),
            PathBuf::from("C:\\").join("Windows").join("kernel.dll")
        );
        assert_eq!(tree.ext_str(file2), "mp4");

        assert!(tree.remove_subtree_soft(file2));
        assert_eq!(tree.get(dir_b).unwrap().logical_size, 0);
        assert_eq!(tree.get(tree.root).unwrap().logical_size, 1_000);
        assert_eq!(tree.total_files, 1);

        let exts = tree.extension_stats(true);
        assert_eq!(exts.len(), 1);
        assert_eq!(exts[0].ext, ".dll");
    }
}
