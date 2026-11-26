use std::path::{Path, PathBuf};

use futures_channel::oneshot;
use uuid::Uuid;

/// User choice when a destination file or directory already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    Overwrite,
    Skip,
    KeepBoth,
    Cancel,
}

/// Decision sent back from the UI conflict dialog to the paused transfer worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictDecision {
    pub resolution: ConflictResolution,
    pub apply_to_all: bool,
}

/// Interactive conflict prompt emitted by a running transfer job.
pub struct ConflictPrompt {
    pub job_id: Uuid,
    pub source_path: PathBuf,
    pub dest_path: PathBuf,
    pub source_size: u64,
    pub dest_size: u64,
    pub source_modified_secs: i64,
    pub dest_modified_secs: i64,
    pub is_dir: bool,
    responder: Option<oneshot::Sender<ConflictDecision>>,
}

impl ConflictPrompt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        job_id: Uuid,
        source_path: PathBuf,
        dest_path: PathBuf,
        source_size: u64,
        dest_size: u64,
        source_modified_secs: i64,
        dest_modified_secs: i64,
        is_dir: bool,
        responder: oneshot::Sender<ConflictDecision>,
    ) -> Self {
        Self {
            job_id,
            source_path,
            dest_path,
            source_size,
            dest_size,
            source_modified_secs,
            dest_modified_secs,
            is_dir,
            responder: Some(responder),
        }
    }

    /// Sends the user's choice back to the paused transfer worker.
    pub fn respond(&mut self, decision: ConflictDecision) {
        if let Some(tx) = self.responder.take() {
            let _ = tx.send(decision);
        }
    }
}

/// Generates a non-colliding sibling path using Windows Explorer's `"name (2).ext"` convention.
pub fn unique_keep_both_path(dest: &Path) -> PathBuf {
    let Some(parent) = dest.parent() else {
        return dest.to_path_buf();
    };
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());

    let is_dir = dest.is_dir();
    let (stem, ext) = if is_dir {
        (file_name.as_str(), "")
    } else {
        match file_name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() && !e.is_empty() => (s, e),
            _ => (file_name.as_str(), ""),
        }
    };

    for counter in 2u32..10_000 {
        let candidate_name = if ext.is_empty() {
            format!("{stem} ({counter})")
        } else {
            format!("{stem} ({counter}).{ext}")
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }

    parent.join(format!("{stem}-{}", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_unique_keep_both_names() {
        let root = std::env::temp_dir().join(format!("texplorer_conflict_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let p1 = root.join("report.pdf");
        std::fs::write(&p1, b"a").unwrap();
        let p2 = unique_keep_both_path(&p1);
        assert_eq!(p2.file_name().unwrap(), "report (2).pdf");

        std::fs::write(&p2, b"b").unwrap();
        let p3 = unique_keep_both_path(&p1);
        assert_eq!(p3.file_name().unwrap(), "report (3).pdf");

        let d1 = root.join("Assets");
        std::fs::create_dir(&d1).unwrap();
        let d2 = unique_keep_both_path(&d1);
        assert_eq!(d2.file_name().unwrap(), "Assets (2)");

        let _ = std::fs::remove_dir_all(&root);
    }
}
