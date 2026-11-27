use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use rayon::prelude::*;
use texplorer_core::{DriveKind, VolumeInfo};
use texplorer_mft::mft_raw::{NTFS_ROOT_RECORD_ID, scan_volume_mft_entries};
use texplorer_mft::{
    ExportedMftNode, FLAG_IS_COMPRESSED, FLAG_IS_DIR, FLAG_IS_HIDDEN, FLAG_IS_SYMLINK,
    FLAG_IS_SYSTEM, export_volume_tree_via_service,
};
use texplorer_platform::is_elevated;

use crate::tree::{DiskTree, NodeFlags, NodeId, ScanEngineKind};

/// Target chosen by the user for Disk Space Analyzer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskScanTarget {
    /// Scan all fixed local drives into a unified "This PC" root.
    AllFixedDrives,
    /// Scan one mounted storage volume (`C:\`, `D:\`, ...).
    Volume(VolumeInfo),
    /// Scan an arbitrary local folder path.
    Folder(PathBuf),
}

impl DiskScanTarget {
    pub fn label(&self) -> String {
        match self {
            Self::AllFixedDrives => "All Drives (This PC)".to_string(),
            Self::Volume(v) => v.display_name(),
            Self::Folder(p) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn is_same_volume(&self, root_path: &Path) -> bool {
        match self {
            Self::Volume(v) => v.root_path == root_path,
            _ => false,
        }
    }
}

/// Live progress snapshot emitted periodically during a disk scan.
#[derive(Debug, Clone)]
pub struct DiskScanProgress {
    pub engine: ScanEngineKind,
    pub files_scanned: u64,
    pub dirs_scanned: u64,
    pub bytes_scanned: u64,
    pub denied_count: u64,
    pub current_path: String,
    pub elapsed_ms: u64,
}

/// Events emitted by a background disk scan job to the UI thread.
#[derive(Debug, Clone)]
pub enum DiskScanEvent {
    Progress(DiskScanProgress),
    Finished {
        tree: Arc<DiskTree>,
        elapsed_ms: u64,
    },
    Cancelled,
    Failed(String),
}

/// Dedicated high-concurrency I/O thread pool sized for NVMe/SSD cold-cache metadata queues
/// (`2x` logical cores, clamped to `8..=32`) so parallel walks saturate disk queues cleanly.
fn scan_io_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let threads = (cores.saturating_mul(2)).clamp(8, 32);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("texplorer-disk-io-{i}"))
            .build()
            .expect("failed creating disk scan IO thread pool")
    })
}

/// Queries the NTFS/filesystem cluster size (bytes per allocation unit) for `path`, defaulting to 4096.
pub fn cluster_size_for_path(path: &Path) -> u64 {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceW;

        let root_str = drive_letter_from_path(path)
            .map(|c| format!("{c}:\\"))
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let wide: Vec<u16> = std::ffi::OsStr::new(&root_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sectors_per_cluster: u32 = 0;
        let mut bytes_per_sector: u32 = 0;
        let mut free_clusters: u32 = 0;
        let mut total_clusters: u32 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceW(
                wide.as_ptr(),
                &mut sectors_per_cluster,
                &mut bytes_per_sector,
                &mut free_clusters,
                &mut total_clusters,
            )
        };
        if ok != 0 && sectors_per_cluster > 0 && bytes_per_sector > 0 {
            return u64::from(sectors_per_cluster).saturating_mul(u64::from(bytes_per_sector));
        }
        4096
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        4096
    }
}

/// Spawns a background scan thread for `target` and streams `DiskScanEvent` updates via `on_event`.
pub fn spawn_disk_scan<F>(
    target: DiskScanTarget,
    volumes: Vec<VolumeInfo>,
    cancel: Arc<AtomicBool>,
    on_event: F,
) where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    let callback = Arc::new(on_event);
    std::thread::Builder::new()
        .name("texplorer-disk-scan".to_string())
        .spawn(move || {
            let started = Instant::now();
            let result = match target {
                DiskScanTarget::AllFixedDrives => {
                    scan_all_fixed_drives(&volumes, &cancel, started, &callback)
                }
                DiskScanTarget::Volume(vol) => scan_single_volume(&vol, &cancel, started, &callback),
                DiskScanTarget::Folder(path) => {
                    scan_folder_target(&path, &cancel, started, &callback)
                }
            };

            if cancel.load(Ordering::Relaxed) {
                callback(DiskScanEvent::Cancelled);
                return;
            }

            match result {
                Ok(tree) => callback(DiskScanEvent::Finished {
                    tree: Arc::new(tree),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                }),
                Err(err) => {
                    if cancel.load(Ordering::Relaxed) {
                        callback(DiskScanEvent::Cancelled);
                    } else {
                        callback(DiskScanEvent::Failed(err));
                    }
                }
            }
        })
        .expect("failed to spawn disk scan thread");
}

fn scan_folder_target<F>(
    path: &Path,
    cancel: &Arc<AtomicBool>,
    started: Instant,
    on_event: &Arc<F>,
) -> Result<DiskTree, String>
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    // 1. If the folder is on a Windows drive letter, try instant scoped export from MFT Service!
    if let Some(drive_letter) = drive_letter_from_path(path) {
        let scope = if is_drive_root(path).is_some() {
            None
        } else {
            Some(path.to_string_lossy().into_owned())
        };
        let root_name = folder_display_name(path);
        if let Ok(tree) = build_tree_from_mft_service(
            path,
            root_name,
            drive_letter,
            scope.clone(),
            cancel,
            started,
            on_event,
        ) {
            return Ok(tree);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Scan cancelled.".to_string());
        }

        // 2. If targeting a full drive root while elevated, fall back to direct $MFT parsing.
        if scope.is_none() && is_elevated() {
            on_event(DiskScanEvent::Progress(DiskScanProgress {
                engine: ScanEngineKind::NtfsMft,
                files_scanned: 0,
                dirs_scanned: 0,
                bytes_scanned: 0,
                denied_count: 0,
                current_path: format!("Reading {drive_letter}:\\ $MFT directly..."),
                elapsed_ms: started.elapsed().as_millis() as u64,
            }));
            if let Ok(entries) = scan_volume_mft_entries(drive_letter, cancel) {
                let cluster = cluster_size_for_path(path);
                return Ok(build_tree_from_raw_mft(path, drive_letter, entries, cluster));
            }
        }
    }

    scan_directory_parallel(path, cancel, started, on_event)
}

fn scan_single_volume<F>(
    vol: &VolumeInfo,
    cancel: &Arc<AtomicBool>,
    started: Instant,
    on_event: &Arc<F>,
) -> Result<DiskTree, String>
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    let is_ntfs = vol.file_system.is_empty() || vol.file_system.eq_ignore_ascii_case("NTFS");
    if is_ntfs
        && let Some(drive_letter) = drive_letter_from_path(&vol.root_path)
        && let Some(tree) = try_fast_mft_volume(vol, drive_letter, cancel, started, on_event)
    {
        return Ok(tree);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Scan cancelled.".to_string());
    }
    scan_directory_parallel(&vol.root_path, cancel, started, on_event)
}

fn scan_all_fixed_drives<F>(
    volumes: &[VolumeInfo],
    cancel: &Arc<AtomicBool>,
    started: Instant,
    on_event: &Arc<F>,
) -> Result<DiskTree, String>
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    let mut targets: Vec<VolumeInfo> = volumes
        .iter()
        .filter(|v| v.kind == DriveKind::Fixed && v.root_path.exists())
        .cloned()
        .collect();
    if targets.is_empty() {
        targets = volumes
            .iter()
            .filter(|v| v.root_path.exists())
            .cloned()
            .collect();
    }
    if targets.is_empty() {
        return Err("No accessible local drives were found.".to_string());
    }

    on_event(DiskScanEvent::Progress(DiskScanProgress {
        engine: ScanEngineKind::Hybrid,
        files_scanned: 0,
        dirs_scanned: 0,
        bytes_scanned: 0,
        denied_count: 0,
        current_path: format!("Scanning {} drives in parallel...", targets.len()),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }));

    // Scan all fixed physical drives concurrently so multi-drive systems finish in max(drive_time)
    // instead of sum(drive_time).
    let scanned_results: Vec<(String, DiskTree)> = scan_io_pool().install(|| {
        targets
            .par_iter()
            .filter_map(|vol| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let tree = scan_single_volume(vol, cancel, started, on_event).ok()?;
                let label = vol.root_path.to_string_lossy().into_owned();
                Some((label, tree))
            })
            .collect()
    });

    if cancel.load(Ordering::Relaxed) {
        return Err("Scan cancelled.".to_string());
    }

    let mut combined = DiskTree::new_root(
        "This PC",
        PathBuf::new(),
        true,
        ScanEngineKind::ParallelWalk,
    );
    let mut saw_mft = false;
    let mut saw_walk = false;

    for (drive_label, sub_tree) in scanned_results {
        match sub_tree.engine {
            ScanEngineKind::MftService | ScanEngineKind::NtfsMft => saw_mft = true,
            ScanEngineKind::ParallelWalk => saw_walk = true,
            ScanEngineKind::Hybrid => {
                saw_mft = true;
                saw_walk = true;
            }
        }
        let combined_root = combined.root;
        attach_subtree(&mut combined, combined_root, &drive_label, &sub_tree);
    }

    combined.engine = match (saw_mft, saw_walk) {
        (true, true) => ScanEngineKind::Hybrid,
        (true, false) => ScanEngineKind::MftService,
        _ => ScanEngineKind::ParallelWalk,
    };
    combined.finalize_rollups();
    Ok(combined)
}

/// Attempts fast NTFS MFT scanning: first via `TExplorerIndexService` IPC (zero UAC),
/// then via direct raw `$MFT` reading when running elevated.
fn try_fast_mft_volume<F>(
    vol: &VolumeInfo,
    drive_letter: char,
    cancel: &Arc<AtomicBool>,
    started: Instant,
    on_event: &Arc<F>,
) -> Option<DiskTree>
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    let root_name = format!("{}:\\", drive_letter.to_ascii_uppercase());
    if let Ok(tree) = build_tree_from_mft_service(
        &vol.root_path,
        root_name,
        drive_letter,
        None,
        cancel,
        started,
        on_event,
    ) {
        return Some(tree);
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    if is_elevated() {
        on_event(DiskScanEvent::Progress(DiskScanProgress {
            engine: ScanEngineKind::NtfsMft,
            files_scanned: 0,
            dirs_scanned: 0,
            bytes_scanned: 0,
            denied_count: 0,
            current_path: format!("Reading {drive_letter}:\\ $MFT directly..."),
            elapsed_ms: started.elapsed().as_millis() as u64,
        }));
        if let Ok(entries) = scan_volume_mft_entries(drive_letter, cancel) {
            let cluster = cluster_size_for_path(&vol.root_path);
            return Some(build_tree_from_raw_mft(
                &vol.root_path,
                drive_letter,
                entries,
                cluster,
            ));
        }
    }

    None
}

fn build_tree_from_mft_service<F>(
    root_path: &Path,
    root_name: String,
    drive_letter: char,
    scope_path: Option<String>,
    cancel: &Arc<AtomicBool>,
    started: Instant,
    on_event: &Arc<F>,
) -> Result<DiskTree, String>
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    let mut tree = DiskTree::new_root(
        root_name,
        root_path.to_path_buf(),
        false,
        ScanEngineKind::MftService,
    );
    let mut stream_to_node: Vec<NodeId> = Vec::with_capacity(65_536);
    let mut files_seen = 0u64;
    let mut dirs_seen = 0u64;
    let mut bytes_seen = 0u64;

    export_volume_tree_via_service(drive_letter, scope_path, cancel, |chunk: Vec<ExportedMftNode>| {
        tree.reserve_nodes(chunk.len());
        stream_to_node.reserve(chunk.len());
        for rec in chunk {
            if stream_to_node.is_empty() && rec.parent_idx == u32::MAX {
                if let Some(root) = tree.get_mut(tree.root) {
                    root.modified_ts = rec.modified_secs;
                }
                stream_to_node.push(tree.root);
                continue;
            }
            let parent_id = stream_to_node
                .get(rec.parent_idx as usize)
                .copied()
                .unwrap_or(tree.root);
            let flags = mft_flags_to_node_flags(rec.flags);
            if flags.contains(NodeFlags::DIR) {
                dirs_seen += 1;
            } else {
                files_seen += 1;
                bytes_seen = bytes_seen.saturating_add(rec.logical_size);
            }
            let nid = tree.add_node(
                parent_id,
                rec.name,
                rec.logical_size,
                rec.allocated_size,
                rec.modified_secs,
                flags,
            );
            stream_to_node.push(nid);
        }

        on_event(DiskScanEvent::Progress(DiskScanProgress {
            engine: ScanEngineKind::MftService,
            files_scanned: files_seen,
            dirs_scanned: dirs_seen,
            bytes_scanned: bytes_seen,
            denied_count: 0,
            current_path: format!(
                "Streaming {} ({} items)...",
                root_path.display(),
                files_seen + dirs_seen
            ),
            elapsed_ms: started.elapsed().as_millis() as u64,
        }));
    })?;

    if stream_to_node.is_empty() {
        return Err("MFT service returned an empty tree.".to_string());
    }

    tree.finalize_rollups();
    Ok(tree)
}

fn build_tree_from_raw_mft(
    root_path: &Path,
    drive_letter: char,
    entries: Vec<texplorer_mft::mft_raw::MergedMftEntry>,
    cluster_size: u64,
) -> DiskTree {
    let cluster = cluster_size.max(512);
    let root_name = format!("{}:\\", drive_letter.to_ascii_uppercase());
    let mut tree = DiskTree::new_root(
        root_name,
        root_path.to_path_buf(),
        false,
        ScanEngineKind::NtfsMft,
    );
    tree.reserve_nodes(entries.len());

    let mut children: HashMap<u64, Vec<usize>> = HashMap::with_capacity(entries.len() / 8);
    for (idx, e) in entries.iter().enumerate() {
        if e.file_ref == NTFS_ROOT_RECORD_ID {
            if let Some(root) = tree.get_mut(tree.root) {
                root.modified_ts = e.modified_secs;
            }
            continue;
        }
        if e.name.is_empty() || e.parent_ref == e.file_ref {
            continue;
        }
        children.entry(e.parent_ref).or_default().push(idx);
    }

    let mut stack: Vec<(u64, NodeId, u16)> = vec![(NTFS_ROOT_RECORD_ID, tree.root, 0)];
    let mut visited: HashMap<u64, ()> = HashMap::with_capacity(entries.len());
    visited.insert(NTFS_ROOT_RECORD_ID, ());

    while let Some((dir_ref, parent_id, depth)) = stack.pop() {
        if depth >= 128 {
            continue;
        }
        let Some(kids) = children.get(&dir_ref) else {
            continue;
        };
        for &entry_idx in kids {
            let e = &entries[entry_idx];
            if visited.insert(e.file_ref, ()).is_some() {
                continue;
            }
            let flags = mft_flags_to_node_flags(e.flags);
            let is_dir = flags.contains(NodeFlags::DIR);
            let is_sym = flags.intersects(NodeFlags::SYMLINK | NodeFlags::REPARSE);
            let (logical, allocated) = if is_dir || is_sym || e.size == 0 {
                (0, 0)
            } else {
                let rem = e.size % cluster;
                let alloc = if rem == 0 {
                    e.size
                } else {
                    e.size.saturating_add(cluster - rem)
                };
                (e.size, alloc)
            };
            let nid = tree.add_node(
                parent_id,
                e.name.as_str(),
                logical,
                allocated,
                e.modified_secs,
                flags,
            );
            if is_dir && !is_sym {
                stack.push((e.file_ref, nid, depth + 1));
            }
        }
    }

    tree.finalize_rollups();
    tree
}

fn attach_subtree(dst: &mut DiskTree, dst_parent: NodeId, root_name: &str, src: &DiskTree) {
    let Some(src_root) = src.get(src.root) else {
        return;
    };
    dst.reserve_nodes(src.nodes.len());
    let new_root = dst.add_node(
        dst_parent,
        root_name,
        0,
        0,
        src_root.modified_ts,
        src_root.flags,
    );

    let mut stack: Vec<(NodeId, NodeId)> = vec![(src.root, new_root)];
    while let Some((src_id, dst_id)) = stack.pop() {
        for child_id in src.children(src_id) {
            let Some(child) = src.get(child_id) else {
                continue;
            };
            let new_child = dst.add_node(
                dst_id,
                Arc::clone(&child.name),
                if child.is_dir() { 0 } else { child.logical_size },
                if child.is_dir() { 0 } else { child.allocated_size },
                child.modified_ts,
                child.flags,
            );
            if child.is_dir() {
                stack.push((child_id, new_child));
            }
        }
    }
}

fn mft_flags_to_node_flags(raw: u16) -> NodeFlags {
    let mut flags = NodeFlags::empty();
    if (raw & FLAG_IS_DIR) != 0 {
        flags |= NodeFlags::DIR;
    }
    if (raw & FLAG_IS_HIDDEN) != 0 {
        flags |= NodeFlags::HIDDEN;
    }
    if (raw & FLAG_IS_SYSTEM) != 0 {
        flags |= NodeFlags::SYSTEM;
    }
    if (raw & FLAG_IS_SYMLINK) != 0 {
        flags |= NodeFlags::SYMLINK | NodeFlags::REPARSE;
    }
    if (raw & FLAG_IS_COMPRESSED) != 0 {
        flags |= NodeFlags::COMPRESSED;
    }
    flags
}

fn folder_display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn drive_letter_from_path(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && (b[0] as char).is_ascii_alphabetic() {
        Some((b[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

fn is_drive_root(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let trimmed = s.trim_end_matches(['\\', '/']);
    let b = trimmed.as_bytes();
    if b.len() == 2 && b[1] == b':' && (b[0] as char).is_ascii_alphabetic() {
        Some((b[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

// ================================================================================================
// High-speed Parallel Win32 `FileFullDirectoryInfo` (64 KiB batch buffer, zero per-file opens)
// ================================================================================================

struct WalkCounters {
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    denied: AtomicU64,
    last_emit_ms: AtomicU64,
    current_path: Mutex<String>,
}

impl WalkCounters {
    fn new(initial_path: String) -> Self {
        Self {
            files: AtomicU64::new(0),
            dirs: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            denied: AtomicU64::new(0),
            last_emit_ms: AtomicU64::new(0),
            current_path: Mutex::new(initial_path),
        }
    }

    fn maybe_report<F>(&self, started: Instant, on_event: &F, path_hint: Option<&Path>)
    where
        F: Fn(DiskScanEvent),
    {
        let now_ms = started.elapsed().as_millis() as u64;
        let prev = self.last_emit_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(prev) < 60 {
            return;
        }
        if self
            .last_emit_ms
            .compare_exchange(prev, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        if let Some(p) = path_hint
            && let Ok(mut guard) = self.current_path.try_lock()
        {
            *guard = p.to_string_lossy().into_owned();
        }
        let cur = self
            .current_path
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        on_event(DiskScanEvent::Progress(DiskScanProgress {
            engine: ScanEngineKind::ParallelWalk,
            files_scanned: self.files.load(Ordering::Relaxed),
            dirs_scanned: self.dirs.load(Ordering::Relaxed),
            bytes_scanned: self.bytes.load(Ordering::Relaxed),
            denied_count: self.denied.load(Ordering::Relaxed),
            current_path: cur,
            elapsed_ms: now_ms,
        }));
    }
}

struct TempNode {
    name: String,
    logical_size: u64,
    allocated_size: u64,
    modified_ts: i64,
    flags: NodeFlags,
    children: Vec<TempNode>,
}

fn scan_directory_parallel<F>(
    root_path: &Path,
    cancel: &Arc<AtomicBool>,
    started: Instant,
    on_event: &Arc<F>,
) -> Result<DiskTree, String>
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    let cluster_size = cluster_size_for_path(root_path);
    let counters = Arc::new(WalkCounters::new(root_path.to_string_lossy().into_owned()));

    on_event(DiskScanEvent::Progress(DiskScanProgress {
        engine: ScanEngineKind::ParallelWalk,
        files_scanned: 0,
        dirs_scanned: 0,
        bytes_scanned: 0,
        denied_count: 0,
        current_path: root_path.to_string_lossy().into_owned(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }));

    let (root_children, _) = scan_io_pool().install(|| {
        walk_dir_recursive(
            root_path,
            cluster_size,
            cancel,
            &counters,
            started,
            on_event,
            0,
        )
    });

    if cancel.load(Ordering::Relaxed) {
        return Err("Scan cancelled.".to_string());
    }

    let root_name = folder_display_name(root_path);
    let mut tree = DiskTree::new_root(
        root_name,
        root_path.to_path_buf(),
        false,
        ScanEngineKind::ParallelWalk,
    );
    let est_total = (counters.files.load(Ordering::Relaxed)
        + counters.dirs.load(Ordering::Relaxed)) as usize;
    tree.reserve_nodes(est_total.min(2_000_000));

    let mut stack: Vec<(NodeId, Vec<TempNode>)> = vec![(tree.root, root_children)];
    while let Some((parent_id, children)) = stack.pop() {
        for temp in children {
            let is_dir = temp.flags.contains(NodeFlags::DIR);
            let nid = tree.add_node(
                parent_id,
                temp.name,
                if is_dir { 0 } else { temp.logical_size },
                if is_dir { 0 } else { temp.allocated_size },
                temp.modified_ts,
                temp.flags,
            );
            if !temp.children.is_empty() {
                stack.push((nid, temp.children));
            }
        }
    }

    tree.finalize_rollups();
    Ok(tree)
}

#[cfg(target_os = "windows")]
struct DirBatchResult {
    files: Vec<TempNode>,
    subdirs: Vec<(String, PathBuf, i64, NodeFlags)>,
    local_files: u64,
    local_bytes: u64,
    local_dirs: u64,
    access_denied: bool,
}

#[cfg(target_os = "windows")]
fn walk_dir_recursive<F>(
    dir_path: &Path,
    cluster_size: u64,
    cancel: &Arc<AtomicBool>,
    counters: &Arc<WalkCounters>,
    started: Instant,
    on_event: &Arc<F>,
    depth: u16,
) -> (Vec<TempNode>, bool)
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    if cancel.load(Ordering::Relaxed) || depth > 128 {
        return (Vec::new(), false);
    }

    // Read directory entries and close the directory handle + drop the 64 KiB batch buffer
    // BEFORE recursing so total memory stays strictly O(threads * 64 KiB) even on deep trees.
    let batch = read_dir_full_info_batch(dir_path, cluster_size, cancel);
    if batch.access_denied {
        counters.denied.fetch_add(1, Ordering::Relaxed);
        return (Vec::new(), true);
    }

    if batch.local_files > 0 {
        counters.files.fetch_add(batch.local_files, Ordering::Relaxed);
        counters.bytes.fetch_add(batch.local_bytes, Ordering::Relaxed);
    }
    if batch.local_dirs > 0 {
        counters.dirs.fetch_add(batch.local_dirs, Ordering::Relaxed);
    }
    counters.maybe_report(started, on_event.as_ref(), Some(dir_path));

    let mut files = batch.files;
    let subdirs = batch.subdirs;

    let scanned_subdirs: Vec<TempNode> = if subdirs.len() >= 2 && depth < 64 {
        subdirs
            .into_par_iter()
            .map(|(name, child_path, modified_ts, mut flags)| {
                let (children, denied) = walk_dir_recursive(
                    &child_path,
                    cluster_size,
                    cancel,
                    counters,
                    started,
                    on_event,
                    depth + 1,
                );
                if denied {
                    flags |= NodeFlags::ACCESS_DENIED;
                }
                TempNode {
                    name,
                    logical_size: 0,
                    allocated_size: 0,
                    modified_ts,
                    flags,
                    children,
                }
            })
            .collect()
    } else {
        subdirs
            .into_iter()
            .map(|(name, child_path, modified_ts, mut flags)| {
                let (children, denied) = walk_dir_recursive(
                    &child_path,
                    cluster_size,
                    cancel,
                    counters,
                    started,
                    on_event,
                    depth + 1,
                );
                if denied {
                    flags |= NodeFlags::ACCESS_DENIED;
                }
                TempNode {
                    name,
                    logical_size: 0,
                    allocated_size: 0,
                    modified_ts,
                    flags,
                    children,
                }
            })
            .collect()
    };

    files.extend(scanned_subdirs);
    (files, false)
}

/// Fast Win32 directory reader using `GetFileInformationByHandleEx(FileFullDirectoryInfo)` with a
/// 64 KiB aligned buffer. Retrieves `EndOfFile` + real `AllocationSize` in bulk with zero per-file
/// `CreateFile`/`GetCompressedFileSizeW` calls, falling back to `FindFirstFileExW` on virtual/FUSE
/// drives that do not implement `FileFullDirectoryInfo`.
#[cfg(target_os = "windows")]
fn read_dir_full_info_batch(
    dir_path: &Path,
    cluster_size: u64,
    cancel: &Arc<AtomicBool>,
) -> DirBatchResult {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
        ERROR_NO_MORE_FILES, ERROR_NOT_SUPPORTED, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE, FILE_ATTRIBUTE_SYSTEM,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FULL_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileFullDirectoryInfo, GetFileInformationByHandleEx,
        OPEN_EXISTING,
    };

    let extended_dir = make_win32_extended_path(dir_path);
    let mut wide_dir: Vec<u16> = extended_dir.encode_utf16().collect();
    wide_dir.push(0);

    let handle = unsafe {
        CreateFileW(
            wide_dir.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        if err == ERROR_ACCESS_DENIED {
            return DirBatchResult {
                files: Vec::new(),
                subdirs: Vec::new(),
                local_files: 0,
                local_bytes: 0,
                local_dirs: 0,
                access_denied: true,
            };
        }
        return read_dir_find_first_fallback(dir_path, cluster_size, cancel);
    }

    // 64 KiB buffer aligned to 8 bytes so casting to `FILE_FULL_DIR_INFO` is strictly aligned.
    let mut buf_u64 = vec![0u64; 8192];
    let buf_byte_len = buf_u64.len() * std::mem::size_of::<u64>();
    let name_field_offset = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);

    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    let mut local_files = 0u64;
    let mut local_bytes = 0u64;
    let mut local_dirs = 0u64;
    let mut first_batch = true;

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileFullDirectoryInfo,
                buf_u64.as_mut_ptr() as *mut core::ffi::c_void,
                buf_byte_len as u32,
            )
        };

        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NO_MORE_FILES {
                break;
            }
            if first_batch
                && matches!(
                    err,
                    ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER
                )
            {
                unsafe {
                    CloseHandle(handle);
                }
                drop(buf_u64);
                return read_dir_find_first_fallback(dir_path, cluster_size, cancel);
            }
            break;
        }
        first_batch = false;

        let base_ptr = buf_u64.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            if offset + name_field_offset > buf_byte_len {
                break;
            }
            let info = unsafe { &*(base_ptr.add(offset) as *const FILE_FULL_DIR_INFO) };
            let name_u16_len = (info.FileNameLength as usize) / 2;
            if offset + name_field_offset + name_u16_len * 2 > buf_byte_len {
                break;
            }

            let name_slice = unsafe {
                std::slice::from_raw_parts(info.FileName.as_ptr(), name_u16_len)
            };
            let is_dot = (name_u16_len == 1 && name_slice[0] == b'.' as u16)
                || (name_u16_len == 2
                    && name_slice[0] == b'.' as u16
                    && name_slice[1] == b'.' as u16);

            if !is_dot && name_u16_len > 0 {
                let name = OsString::from_wide(name_slice).to_string_lossy().into_owned();
                let attrs = info.FileAttributes;
                let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
                let is_reparse = (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;

                let mut flags = NodeFlags::empty();
                if is_dir {
                    flags |= NodeFlags::DIR;
                }
                if (attrs & FILE_ATTRIBUTE_HIDDEN) != 0 {
                    flags |= NodeFlags::HIDDEN;
                }
                if (attrs & FILE_ATTRIBUTE_SYSTEM) != 0 {
                    flags |= NodeFlags::SYSTEM;
                }
                if is_reparse {
                    flags |= NodeFlags::REPARSE | NodeFlags::SYMLINK;
                }
                if (attrs & FILE_ATTRIBUTE_COMPRESSED) != 0 {
                    flags |= NodeFlags::COMPRESSED;
                }
                if (attrs & FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
                    flags |= NodeFlags::SPARSE;
                }

                let modified_ts = i64_filetime_to_unix_secs(info.LastWriteTime);

                if is_dir {
                    local_dirs += 1;
                    if is_reparse {
                        files.push(TempNode {
                            name,
                            logical_size: 0,
                            allocated_size: 0,
                            modified_ts,
                            flags,
                            children: Vec::new(),
                        });
                    } else {
                        let child_path = dir_path.join(&name);
                        subdirs.push((name, child_path, modified_ts, flags));
                    }
                } else {
                    let logical_size = info.EndOfFile.max(0) as u64;
                    let raw_alloc = info.AllocationSize.max(0) as u64;
                    let allocated_size = if is_reparse || logical_size == 0 {
                        0
                    } else if raw_alloc > 0 {
                        raw_alloc
                    } else if logical_size >= cluster_size {
                        round_up_cluster(logical_size, cluster_size)
                    } else {
                        0
                    };

                    local_files += 1;
                    local_bytes = local_bytes.saturating_add(logical_size);
                    files.push(TempNode {
                        name,
                        logical_size,
                        allocated_size,
                        modified_ts,
                        flags,
                        children: Vec::new(),
                    });
                }
            }

            if info.NextEntryOffset == 0 {
                break;
            }
            offset = offset.saturating_add(info.NextEntryOffset as usize);
        }
    }

    unsafe {
        CloseHandle(handle);
    }

    DirBatchResult {
        files,
        subdirs,
        local_files,
        local_bytes,
        local_dirs,
        access_denied: false,
    }
}

/// Fallback Win32 directory enumerator for virtual/FUSE filesystems that do not support
/// `GetFileInformationByHandleEx(FileFullDirectoryInfo)`.
#[cfg(target_os = "windows")]
fn read_dir_find_first_fallback(
    dir_path: &Path,
    cluster_size: u64,
    cancel: &Arc<AtomicBool>,
) -> DirBatchResult {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE, FILE_ATTRIBUTE_SYSTEM,
        FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch,
        FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
    };

    let pattern = format!(r"{}\*", make_win32_extended_path(dir_path));
    let mut wide: Vec<u16> = pattern.encode_utf16().collect();
    wide.push(0);

    let mut find_data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe {
        FindFirstFileExW(
            wide.as_ptr(),
            FindExInfoBasic,
            &mut find_data as *mut WIN32_FIND_DATAW as *mut core::ffi::c_void,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        let denied = unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        return DirBatchResult {
            files: Vec::new(),
            subdirs: Vec::new(),
            local_files: 0,
            local_bytes: 0,
            local_dirs: 0,
            access_denied: denied,
        };
    }

    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    let mut local_files = 0u64;
    let mut local_bytes = 0u64;
    let mut local_dirs = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let name_len = find_data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(find_data.cFileName.len());
        let name_slice = &find_data.cFileName[..name_len];

        let is_dot = (name_len == 1 && name_slice[0] == b'.' as u16)
            || (name_len == 2 && name_slice[0] == b'.' as u16 && name_slice[1] == b'.' as u16);

        if !is_dot && name_len > 0 {
            let name = OsString::from_wide(name_slice).to_string_lossy().into_owned();
            let attrs = find_data.dwFileAttributes;
            let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
            let is_reparse = (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;

            let mut flags = NodeFlags::empty();
            if is_dir {
                flags |= NodeFlags::DIR;
            }
            if (attrs & FILE_ATTRIBUTE_HIDDEN) != 0 {
                flags |= NodeFlags::HIDDEN;
            }
            if (attrs & FILE_ATTRIBUTE_SYSTEM) != 0 {
                flags |= NodeFlags::SYSTEM;
            }
            if is_reparse {
                flags |= NodeFlags::REPARSE | NodeFlags::SYMLINK;
            }
            if (attrs & FILE_ATTRIBUTE_COMPRESSED) != 0 {
                flags |= NodeFlags::COMPRESSED;
            }
            if (attrs & FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
                flags |= NodeFlags::SPARSE;
            }

            let modified_ts = filetime_to_unix_secs(
                find_data.ftLastWriteTime.dwLowDateTime,
                find_data.ftLastWriteTime.dwHighDateTime,
            );

            if is_dir {
                local_dirs += 1;
                if is_reparse {
                    files.push(TempNode {
                        name,
                        logical_size: 0,
                        allocated_size: 0,
                        modified_ts,
                        flags,
                        children: Vec::new(),
                    });
                } else {
                    let child_path = dir_path.join(&name);
                    subdirs.push((name, child_path, modified_ts, flags));
                }
            } else {
                let logical_size = ((u64::from(find_data.nFileSizeHigh)) << 32)
                    | u64::from(find_data.nFileSizeLow);
                let allocated_size = if is_reparse || logical_size == 0 {
                    0
                } else {
                    round_up_cluster(logical_size, cluster_size)
                };

                local_files += 1;
                local_bytes = local_bytes.saturating_add(logical_size);
                files.push(TempNode {
                    name,
                    logical_size,
                    allocated_size,
                    modified_ts,
                    flags,
                    children: Vec::new(),
                });
            }
        }

        if unsafe { FindNextFileW(handle, &mut find_data) } == 0 {
            break;
        }
    }

    unsafe {
        FindClose(handle);
    }

    DirBatchResult {
        files,
        subdirs,
        local_files,
        local_bytes,
        local_dirs,
        access_denied: false,
    }
}

#[cfg(target_os = "windows")]
fn make_win32_extended_path(dir: &Path) -> String {
    let raw = dir.to_string_lossy();
    let trimmed = raw.trim_end_matches(['\\', '/']);
    let base = if trimmed.starts_with(r"\\?\") {
        trimmed.to_string()
    } else if let Some(unc) = trimmed.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{trimmed}")
    };
    // Drive roots like `\\?\C:` require a trailing backslash for CreateFileW.
    if base.len() == 6 && base.as_bytes()[5] == b':' {
        format!(r"{base}\")
    } else {
        base
    }
}

#[cfg(target_os = "windows")]
#[inline]
fn filetime_to_unix_secs(low: u32, high: u32) -> i64 {
    const WINDOWS_TO_UNIX_EPOCH_SECS: i64 = 11_644_473_600;
    let ticks = ((u64::from(high)) << 32) | u64::from(low);
    if ticks == 0 {
        return 0;
    }
    ((ticks / 10_000_000) as i64).saturating_sub(WINDOWS_TO_UNIX_EPOCH_SECS)
}

#[cfg(target_os = "windows")]
#[inline]
fn i64_filetime_to_unix_secs(ticks: i64) -> i64 {
    const WINDOWS_TO_UNIX_EPOCH_SECS: i64 = 11_644_473_600;
    if ticks <= 0 {
        return 0;
    }
    (ticks / 10_000_000).saturating_sub(WINDOWS_TO_UNIX_EPOCH_SECS)
}

#[cfg(not(target_os = "windows"))]
fn walk_dir_recursive<F>(
    dir_path: &Path,
    cluster_size: u64,
    cancel: &Arc<AtomicBool>,
    counters: &Arc<WalkCounters>,
    started: Instant,
    on_event: &Arc<F>,
    depth: u16,
) -> (Vec<TempNode>, bool)
where
    F: Fn(DiskScanEvent) + Send + Sync + 'static,
{
    use std::time::UNIX_EPOCH;

    if cancel.load(Ordering::Relaxed) || depth > 128 {
        return (Vec::new(), false);
    }

    let entries = match std::fs::read_dir(dir_path) {
        Ok(rd) => rd,
        Err(err) => {
            let denied = err.kind() == std::io::ErrorKind::PermissionDenied;
            if denied {
                counters.denied.fetch_add(1, Ordering::Relaxed);
            }
            return (Vec::new(), denied);
        }
    };

    let mut files = Vec::new();
    let mut subdirs = Vec::new();

    for entry in entries.flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified_ts = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut flags = NodeFlags::empty();
        if name.starts_with('.') {
            flags |= NodeFlags::HIDDEN;
        }
        if meta.file_type().is_symlink() {
            flags |= NodeFlags::SYMLINK;
        }
        if meta.is_dir() {
            flags |= NodeFlags::DIR;
            counters.dirs.fetch_add(1, Ordering::Relaxed);
            if flags.contains(NodeFlags::SYMLINK) {
                files.push(TempNode {
                    name,
                    logical_size: 0,
                    allocated_size: 0,
                    modified_ts,
                    flags,
                    children: Vec::new(),
                });
            } else {
                subdirs.push((name, entry.path(), modified_ts, flags));
            }
        } else {
            let sz = meta.len();
            counters.files.fetch_add(1, Ordering::Relaxed);
            counters.bytes.fetch_add(sz, Ordering::Relaxed);
            files.push(TempNode {
                name,
                logical_size: sz,
                allocated_size: round_up_cluster(sz, cluster_size),
                modified_ts,
                flags,
                children: Vec::new(),
            });
        }
    }

    counters.maybe_report(started, on_event.as_ref(), Some(dir_path));

    let scanned: Vec<TempNode> = subdirs
        .into_par_iter()
        .map(|(name, child_path, modified_ts, mut flags)| {
            let (children, denied) = walk_dir_recursive(
                &child_path,
                cluster_size,
                cancel,
                counters,
                started,
                on_event,
                depth + 1,
            );
            if denied {
                flags |= NodeFlags::ACCESS_DENIED;
            }
            TempNode {
                name,
                logical_size: 0,
                allocated_size: 0,
                modified_ts,
                flags,
                children,
            }
        })
        .collect();

    files.extend(scanned);
    (files, false)
}

#[inline]
fn round_up_cluster(size: u64, cluster: u64) -> u64 {
    if size == 0 {
        return 0;
    }
    let c = cluster.max(512);
    let rem = size % c;
    if rem == 0 {
        size
    } else {
        size.saturating_add(c - rem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn scans_temp_directory_accurately() {
        let root =
            std::env::temp_dir().join(format!("texplorer_disk_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("alpha/beta")).unwrap();
        std::fs::write(root.join("top.bin"), vec![0u8; 2_500]).unwrap();
        std::fs::write(root.join("alpha/beta/code.rs"), vec![b'a'; 1_200]).unwrap();

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_disk_scan(
            DiskScanTarget::Folder(root.clone()),
            Vec::new(),
            cancel,
            move |ev| {
                let _ = tx.send(ev);
            },
        );

        let mut finished_tree = None;
        for ev in rx {
            if let DiskScanEvent::Finished { tree, .. } = ev {
                finished_tree = Some(tree);
                break;
            }
        }

        let tree = finished_tree.expect("scan should finish");
        assert_eq!(tree.get(tree.root).unwrap().logical_size, 3_700);
        assert_eq!(tree.total_files, 2);
        assert_eq!(tree.total_dirs, 2);

        let _ = std::fs::remove_dir_all(&root);
    }
}
