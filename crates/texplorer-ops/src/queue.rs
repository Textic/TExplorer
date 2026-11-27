use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use futures_channel::mpsc::UnboundedSender;
use futures_channel::oneshot;
use uuid::Uuid;

use crate::conflict::{ConflictDecision, ConflictPrompt, ConflictResolution, unique_keep_both_path};
use crate::undo::UndoAction;

/// High-level transfer operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Copy,
    Move,
}

impl TransferKind {
    pub fn verb_present(self) -> &'static str {
        match self {
            Self::Copy => "Copying",
            Self::Move => "Moving",
        }
    }
}

/// Lifecycle state of an active or completed transfer job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    Running,
    Paused,
    WaitingConflict,
    Completed,
    Cancelled,
    Failed(String),
}

impl TransferStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed(_))
    }
}

/// Immutable progress snapshot pushed to the UI at a controlled cadence.
#[derive(Debug, Clone)]
pub struct TransferSnapshot {
    pub job_id: Uuid,
    pub kind: TransferKind,
    pub title: String,
    pub target_dir: PathBuf,
    pub status: TransferStatus,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bytes_per_sec: f64,
    pub current_item: String,
}

impl TransferSnapshot {
    pub fn fraction(&self) -> f32 {
        if self.bytes_total > 0 {
            (self.bytes_done as f64 / self.bytes_total as f64).clamp(0.0, 1.0) as f32
        } else if self.files_total > 0 {
            (self.files_done as f64 / self.files_total as f64).clamp(0.0, 1.0) as f32
        } else if matches!(self.status, TransferStatus::Completed) {
            1.0
        } else {
            0.0
        }
    }
}

/// Events streamed from transfer workers to the UI coordinator.
pub enum TransferEvent {
    Progress(TransferSnapshot),
    Conflict(ConflictPrompt),
    Finished {
        snapshot: TransferSnapshot,
        undo_action: Option<UndoAction>,
    },
}

/// Thread-safe handle allowing the UI to pause, resume, or cancel a running transfer job.
#[derive(Default)]
pub struct TransferControl {
    paused: AtomicBool,
    cancelled: AtomicBool,
}

impl TransferControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn toggle_pause(&self) -> bool {
        let prev = self.paused.fetch_xor(true, Ordering::AcqRel);
        !prev
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.paused.store(false, Ordering::Release);
    }
}

/// Spawns an asynchronous buffered copy/move job with conflict prompts, pause/cancel,
/// real EMA throughput tracking, and Undo record generation.
pub fn spawn_transfer_job(
    kind: TransferKind,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    event_tx: UnboundedSender<TransferEvent>,
) -> (Uuid, Arc<TransferControl>) {
    let job_id = Uuid::new_v4();
    let control = Arc::new(TransferControl::new());
    let control_bg = control.clone();

    let title = if sources.len() == 1 {
        let name = sources[0]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| sources[0].display().to_string());
        format!("{} \"{name}\"", kind.verb_present())
    } else {
        format!("{} {} items", kind.verb_present(), sources.len())
    };

    let initial = TransferSnapshot {
        job_id,
        kind,
        title: title.clone(),
        target_dir: dest_dir.clone(),
        status: TransferStatus::Queued,
        files_done: 0,
        files_total: sources.len() as u64,
        bytes_done: 0,
        bytes_total: 0,
        speed_bytes_per_sec: 0.0,
        current_item: String::new(),
    };
    let _ = event_tx.unbounded_send(TransferEvent::Progress(initial));

    std::thread::Builder::new()
        .name(format!("texplorer-xfer-{}", &job_id.to_string()[..8]))
        .spawn(move || {
            run_transfer_worker(job_id, kind, title, sources, dest_dir, control_bg, event_tx);
        })
        .expect("failed to spawn transfer worker thread");

    (job_id, control)
}

struct WorkerState {
    job_id: Uuid,
    kind: TransferKind,
    title: String,
    target_dir: PathBuf,
    control: Arc<TransferControl>,
    tx: UnboundedSender<TransferEvent>,
    files_done: u64,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
    ema_speed: f64,
    last_emit: Instant,
    last_bytes: u64,
    current_item: String,
    sticky_conflict: Option<ConflictResolution>,
}

impl WorkerState {
    fn snapshot(&self, status: TransferStatus) -> TransferSnapshot {
        TransferSnapshot {
            job_id: self.job_id,
            kind: self.kind,
            title: self.title.clone(),
            target_dir: self.target_dir.clone(),
            status,
            files_done: self.files_done,
            files_total: self.files_total,
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
            speed_bytes_per_sec: self.ema_speed,
            current_item: self.current_item.clone(),
        }
    }

    fn emit_progress(&mut self, status: TransferStatus, force: bool) {
        let elapsed = self.last_emit.elapsed();
        if !force && elapsed < Duration::from_millis(50) {
            return;
        }
        let secs = elapsed.as_secs_f64();
        if secs > 0.005 {
            let delta = self.bytes_done.saturating_sub(self.last_bytes) as f64;
            let inst_rate = delta / secs;
            self.ema_speed = if self.ema_speed <= 0.0 {
                inst_rate
            } else {
                self.ema_speed * 0.72 + inst_rate * 0.28
            };
        }
        self.last_bytes = self.bytes_done;
        self.last_emit = Instant::now();
        let _ = self.tx.unbounded_send(TransferEvent::Progress(self.snapshot(status)));
    }

    /// Blocks while paused; returns `Err(Cancelled)` if cancelled.
    fn check_pause_or_cancel(&mut self) -> Result<(), WorkerStop> {
        if self.control.is_cancelled() {
            return Err(WorkerStop::Cancelled);
        }
        if self.control.is_paused() {
            self.ema_speed = 0.0;
            self.emit_progress(TransferStatus::Paused, true);
            while self.control.is_paused() {
                if self.control.is_cancelled() {
                    return Err(WorkerStop::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            self.last_emit = Instant::now();
            self.last_bytes = self.bytes_done;
            self.emit_progress(TransferStatus::Running, true);
        }
        Ok(())
    }

    fn resolve_conflict(
        &mut self,
        src: &Path,
        dst: &Path,
        is_dir: bool,
    ) -> Result<ConflictResolution, WorkerStop> {
        if let Some(sticky) = self.sticky_conflict {
            return Ok(sticky);
        }
        let src_meta = fs::symlink_metadata(src).ok();
        let dst_meta = fs::symlink_metadata(dst).ok();
        let (resp_tx, resp_rx) = oneshot::channel::<ConflictDecision>();
        let prompt = ConflictPrompt::new(
            self.job_id,
            src.to_path_buf(),
            dst.to_path_buf(),
            src_meta.as_ref().map(|m| m.len()).unwrap_or(0),
            dst_meta.as_ref().map(|m| m.len()).unwrap_or(0),
            src_meta.as_ref().map(mtime_secs).unwrap_or(0),
            dst_meta.as_ref().map(mtime_secs).unwrap_or(0),
            is_dir,
            resp_tx,
        );
        self.ema_speed = 0.0;
        self.emit_progress(TransferStatus::WaitingConflict, true);
        let _ = self.tx.unbounded_send(TransferEvent::Conflict(prompt));

        let decision = futures_executor_block_on_oneshot(resp_rx, &self.control)?;
        if decision.apply_to_all && decision.resolution != ConflictResolution::Cancel {
            self.sticky_conflict = Some(decision.resolution);
        }
        if decision.resolution == ConflictResolution::Cancel {
            return Err(WorkerStop::Cancelled);
        }
        self.last_emit = Instant::now();
        self.last_bytes = self.bytes_done;
        self.emit_progress(TransferStatus::Running, true);
        Ok(decision.resolution)
    }
}

enum WorkerStop {
    Cancelled,
    PermissionDenied(String),
    Failed(String),
}

fn io_stop(msg: String, err: std::io::Error) -> WorkerStop {
    if err.kind() == std::io::ErrorKind::PermissionDenied || err.raw_os_error() == Some(5) {
        WorkerStop::PermissionDenied(msg)
    } else {
        WorkerStop::Failed(msg)
    }
}

fn futures_executor_block_on_oneshot(
    mut rx: oneshot::Receiver<ConflictDecision>,
    control: &TransferControl,
) -> Result<ConflictDecision, WorkerStop> {
    loop {
        if control.is_cancelled() {
            return Err(WorkerStop::Cancelled);
        }
        match rx.try_recv() {
            Ok(Some(dec)) => return Ok(dec),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => return Err(WorkerStop::Cancelled),
        }
    }
}

fn run_transfer_worker(
    job_id: Uuid,
    kind: TransferKind,
    title: String,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    control: Arc<TransferControl>,
    tx: UnboundedSender<TransferEvent>,
) {
    let (total_files, total_bytes) = estimate_totals(&sources);
    let mut state = WorkerState {
        job_id,
        kind,
        title,
        target_dir: dest_dir.clone(),
        control,
        tx: tx.clone(),
        files_done: 0,
        files_total: total_files.max(sources.len() as u64),
        bytes_done: 0,
        bytes_total: total_bytes,
        ema_speed: 0.0,
        last_emit: Instant::now(),
        last_bytes: 0,
        current_item: String::new(),
        sticky_conflict: None,
    };

    state.emit_progress(TransferStatus::Running, true);

    let mut copied_roots: Vec<PathBuf> = Vec::new();
    let mut moved_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();

    let result: Result<(), WorkerStop> = (|| {
        if !dest_dir.is_dir() {
            return Err(WorkerStop::Failed(format!(
                "Destination folder \"{}\" does not exist.",
                dest_dir.display()
            )));
        }

        for idx in 0..sources.len() {
            let src = &sources[idx];
            let Some((final_dest, needs_dir_remove)) =
                resolve_transfer_target(&mut state, kind, src, &dest_dir)?
            else {
                continue;
            };
            let name = src
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            let item_res: Result<(), WorkerStop> = (|| {
                if needs_dir_remove {
                    fs::remove_dir_all(&final_dest).map_err(|e| {
                        io_stop(
                            format!("Failed replacing \"{}\": {e}", final_dest.display()),
                            e,
                        )
                    })?;
                }

                match kind {
                    TransferKind::Move => {
                        // Fast-path atomic same-volume rename when destination does not yet exist.
                        let fast_renamed = if !final_dest.exists() {
                            match fs::rename(src, &final_dest) {
                                Ok(()) => true,
                                Err(e)
                                    if e.kind() == std::io::ErrorKind::PermissionDenied
                                        || e.raw_os_error() == Some(5) =>
                                {
                                    return Err(WorkerStop::PermissionDenied(format!(
                                        "Failed moving \"{}\": {e}",
                                        src.display()
                                    )));
                                }
                                Err(_) => false,
                            }
                        } else {
                            false
                        };

                        if fast_renamed {
                            let (sub_files, sub_bytes) =
                                estimate_totals(std::slice::from_ref(&final_dest));
                            state.files_done = state.files_done.saturating_add(sub_files.max(1));
                            state.bytes_done = state.bytes_done.saturating_add(sub_bytes);
                            state.current_item = name.clone();
                            state.emit_progress(TransferStatus::Running, false);
                            moved_pairs.push((src.clone(), final_dest.clone()));
                        } else {
                            copy_entry(&mut state, src, &final_dest)?;
                            remove_source_after_move(src)?;
                            moved_pairs.push((src.clone(), final_dest.clone()));
                        }
                    }
                    TransferKind::Copy => {
                        copy_entry(&mut state, src, &final_dest)?;
                        copied_roots.push(final_dest.clone());
                    }
                }
                Ok(())
            })();

            match item_res {
                Ok(()) => {}
                Err(WorkerStop::PermissionDenied(orig_err))
                    if !texplorer_platform::is_elevated() =>
                {
                    // Collect the denied item plus all remaining items in this job so Windows
                    // shows a single native UAC prompt (`IFileOperation`) for the entire batch.
                    let mut elevated_pairs = vec![(src.clone(), final_dest)];
                    for rem_src in &sources[(idx + 1)..] {
                        if let Some((rem_dest, _)) =
                            resolve_transfer_target(&mut state, kind, rem_src, &dest_dir)?
                        {
                            elevated_pairs.push((rem_src.clone(), rem_dest));
                        }
                    }
                    state.current_item = "Waiting for Administrator permission...".to_string();
                    state.emit_progress(TransferStatus::Running, true);

                    match texplorer_platform::elevated_transfer_paths(
                        kind == TransferKind::Move,
                        &elevated_pairs,
                    ) {
                        Ok(()) => {
                            for (e_src, e_dst) in elevated_pairs {
                                match kind {
                                    TransferKind::Move => moved_pairs.push((e_src, e_dst)),
                                    TransferKind::Copy => copied_roots.push(e_dst),
                                }
                            }
                            state.files_done = state.files_total;
                            state.bytes_done = state.bytes_total;
                            state.emit_progress(TransferStatus::Running, true);
                            break;
                        }
                        Err(texplorer_platform::ElevatedOpError::Cancelled) => {
                            return Err(WorkerStop::Cancelled);
                        }
                        Err(texplorer_platform::ElevatedOpError::Failed(elev_err)) => {
                            return Err(WorkerStop::Failed(if elev_err.is_empty() {
                                orig_err
                            } else {
                                elev_err
                            }));
                        }
                    }
                }
                Err(WorkerStop::PermissionDenied(orig_err)) => {
                    return Err(WorkerStop::Failed(orig_err));
                }
                Err(other) => return Err(other),
            }
        }
        Ok(())
    })();

    let (status, undo_action) = match result {
        Ok(()) => {
            let undo = match kind {
                TransferKind::Copy if !copied_roots.is_empty() => {
                    Some(UndoAction::Copy { created: copied_roots })
                }
                TransferKind::Move if !moved_pairs.is_empty() => {
                    Some(UndoAction::Move { pairs: moved_pairs })
                }
                _ => None,
            };
            (TransferStatus::Completed, undo)
        }
        Err(WorkerStop::Cancelled) => (TransferStatus::Cancelled, None),
        Err(WorkerStop::PermissionDenied(err) | WorkerStop::Failed(err)) => {
            (TransferStatus::Failed(err), None)
        }
    };

    state.ema_speed = 0.0;
    let final_snap = state.snapshot(status);
    let _ = tx.unbounded_send(TransferEvent::Finished {
        snapshot: final_snap,
        undo_action,
    });
}

fn resolve_transfer_target(
    state: &mut WorkerState,
    kind: TransferKind,
    src: &Path,
    dest_dir: &Path,
) -> Result<Option<(PathBuf, bool)>, WorkerStop> {
    state.check_pause_or_cancel()?;
    let Some(name) = src.file_name() else {
        return Ok(None);
    };
    let initial_dest = dest_dir.join(name);

    // Guard against copying or moving a directory inside itself.
    if src.is_dir() && is_same_or_descendant(dest_dir, src) {
        return Err(WorkerStop::Failed(format!(
            "Cannot {} \"{}\" into itself.",
            if kind == TransferKind::Copy {
                "copy"
            } else {
                "move"
            },
            src.display()
        )));
    }

    // Moving onto the exact same path is a harmless no-op.
    if kind == TransferKind::Move && paths_equal(src, &initial_dest) {
        return Ok(None);
    }

    // Copying into the same parent auto-renames to `"name (2).ext"` like Windows Explorer.
    let target_dest = if kind == TransferKind::Copy && paths_equal(src, &initial_dest) {
        unique_keep_both_path(&initial_dest)
    } else {
        initial_dest
    };

    let src_meta = fs::symlink_metadata(src).map_err(|e| {
        io_stop(format!("Cannot inspect \"{}\": {e}", src.display()), e)
    })?;
    let is_dir = src_meta.is_dir();

    if target_dest.exists() {
        if is_dir && target_dest.is_dir() {
            // Merging folders resolves conflicts per file inside.
            Ok(Some((target_dest, false)))
        } else {
            match state.resolve_conflict(src, &target_dest, is_dir)? {
                ConflictResolution::Overwrite => Ok(Some((target_dest.clone(), target_dest.is_dir()))),
                ConflictResolution::Skip => {
                    state.files_done = state.files_done.saturating_add(1);
                    state.bytes_done = state.bytes_done.saturating_add(src_meta.len());
                    state.emit_progress(TransferStatus::Running, false);
                    Ok(None)
                }
                ConflictResolution::KeepBoth => Ok(Some((unique_keep_both_path(&target_dest), false))),
                ConflictResolution::Cancel => Err(WorkerStop::Cancelled),
            }
        }
    } else {
        Ok(Some((target_dest, false)))
    }
}

fn copy_entry(state: &mut WorkerState, src: &Path, dst: &Path) -> Result<(), WorkerStop> {
    state.check_pause_or_cancel()?;
    let meta = fs::symlink_metadata(src)
        .map_err(|e| io_stop(format!("Failed inspecting \"{}\": {e}", src.display()), e))?;

    if meta.is_dir() {
        fs::create_dir_all(dst).map_err(|e| {
            io_stop(format!("Failed creating folder \"{}\": {e}", dst.display()), e)
        })?;
        let entries = fs::read_dir(src).map_err(|e| {
            io_stop(format!("Failed listing folder \"{}\": {e}", src.display()), e)
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                io_stop(format!("Failed reading entry in \"{}\": {e}", src.display()), e)
            })?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            if child_dst.exists() && !child_src.is_dir() {
                match state.resolve_conflict(&child_src, &child_dst, false)? {
                    ConflictResolution::Overwrite => {
                        copy_file_chunked(state, &child_src, &child_dst)?;
                    }
                    ConflictResolution::Skip => {
                        state.files_done = state.files_done.saturating_add(1);
                        let sz = fs::symlink_metadata(&child_src).map(|m| m.len()).unwrap_or(0);
                        state.bytes_done = state.bytes_done.saturating_add(sz);
                        state.emit_progress(TransferStatus::Running, false);
                    }
                    ConflictResolution::KeepBoth => {
                        let unique = unique_keep_both_path(&child_dst);
                        copy_file_chunked(state, &child_src, &unique)?;
                    }
                    ConflictResolution::Cancel => return Err(WorkerStop::Cancelled),
                }
            } else {
                copy_entry(state, &child_src, &child_dst)?;
            }
        }
        Ok(())
    } else {
        copy_file_chunked(state, src, dst)
    }
}

fn copy_file_chunked(state: &mut WorkerState, src: &Path, dst: &Path) -> Result<(), WorkerStop> {
    const CHUNK_SIZE: usize = 1024 * 1024; // 1 MiB buffered stream

    state.current_item = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| src.display().to_string());
    state.emit_progress(TransferStatus::Running, false);

    let mut reader = File::open(src)
        .map_err(|e| io_stop(format!("Failed opening \"{}\": {e}", src.display()), e))?;
    let mut writer = File::create(dst)
        .map_err(|e| io_stop(format!("Failed creating \"{}\": {e}", dst.display()), e))?;

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        state.check_pause_or_cancel()?;
        let read = reader
            .read(&mut buf)
            .map_err(|e| io_stop(format!("Failed reading \"{}\": {e}", src.display()), e))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buf[..read])
            .map_err(|e| io_stop(format!("Failed writing \"{}\": {e}", dst.display()), e))?;
        state.bytes_done = state.bytes_done.saturating_add(read as u64);
        state.emit_progress(TransferStatus::Running, false);
    }

    writer
        .flush()
        .map_err(|e| io_stop(format!("Failed flushing \"{}\": {e}", dst.display()), e))?;
    state.files_done = state.files_done.saturating_add(1);
    state.emit_progress(TransferStatus::Running, false);
    Ok(())
}

fn remove_source_after_move(src: &Path) -> Result<(), WorkerStop> {
    if src.is_dir() {
        fs::remove_dir_all(src).map_err(|e| {
            io_stop(
                format!("Failed cleaning source \"{}\": {e}", src.display()),
                e,
            )
        })
    } else {
        fs::remove_file(src).map_err(|e| {
            io_stop(
                format!("Failed cleaning source \"{}\": {e}", src.display()),
                e,
            )
        })
    }
}

fn estimate_totals(paths: &[PathBuf]) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack: Vec<PathBuf> = paths.to_vec();

    while let Some(cur) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&cur) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            files += 1;
        } else if meta.is_dir() {
            if let Ok(rd) = fs::read_dir(&cur) {
                for entry in rd.flatten() {
                    stack.push(entry.path());
                }
            }
        } else {
            files += 1;
            bytes = bytes.saturating_add(meta.len());
        }
    }
    (files, bytes)
}

fn mtime_secs(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        a.to_string_lossy()
            .replace('/', "\\")
            .eq_ignore_ascii_case(&b.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        a == b
    }
}

fn is_same_or_descendant(candidate: &Path, ancestor: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        let c = candidate.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        let a = ancestor
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase();
        c == a || c.starts_with(&format!("{a}\\"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        candidate == ancestor || candidate.starts_with(ancestor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_channel::mpsc::unbounded;

    #[test]
    fn copies_moves_and_resolves_keep_both_conflicts() {
        let root = std::env::temp_dir().join(format!("texplorer_queue_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let src_dir = root.join("src");
        let dst_dir = root.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();

        let src_file = src_dir.join("sample.txt");
        fs::write(&src_file, b"payload-12345").unwrap();
        fs::write(dst_dir.join("sample.txt"), b"existing").unwrap();

        let (tx, mut rx) = unbounded();
        let (_id, _ctrl) = spawn_transfer_job(
            TransferKind::Copy,
            vec![src_file.clone()],
            dst_dir.clone(),
            tx,
        );

        let mut finished_ok = false;
        let mut got_conflict = false;
        while let Ok(Some(ev)) = wait_event(&mut rx) {
            match ev {
                TransferEvent::Conflict(mut prompt) => {
                    got_conflict = true;
                    prompt.respond(ConflictDecision {
                        resolution: ConflictResolution::KeepBoth,
                        apply_to_all: false,
                    });
                }
                TransferEvent::Finished { snapshot, .. } => {
                    finished_ok = matches!(snapshot.status, TransferStatus::Completed);
                    break;
                }
                TransferEvent::Progress(_) => {}
            }
        }

        assert!(got_conflict);
        assert!(finished_ok);
        assert_eq!(fs::read(dst_dir.join("sample.txt")).unwrap(), b"existing");
        assert_eq!(
            fs::read(dst_dir.join("sample (2).txt")).unwrap(),
            b"payload-12345"
        );

        let _ = fs::remove_dir_all(&root);
    }

    fn wait_event(
        rx: &mut futures_channel::mpsc::UnboundedReceiver<TransferEvent>,
    ) -> Result<Option<TransferEvent>, ()> {
        for _ in 0..200 {
            if let Ok(ev) = rx.try_recv() {
                return Ok(Some(ev));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err(())
    }
}
