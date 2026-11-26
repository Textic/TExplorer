use std::path::PathBuf;

use texplorer_platform::move_to_trash;

/// Reversible filesystem transaction recorded for `Ctrl+Z` / `Ctrl+Y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoAction {
    Rename { from: PathBuf, to: PathBuf },
    Move { pairs: Vec<(PathBuf, PathBuf)> },
    Copy { created: Vec<PathBuf> },
    CreateDir { path: PathBuf },
    CreateFile { path: PathBuf },
}

impl UndoAction {
    pub fn label(&self) -> String {
        match self {
            Self::Rename { to, .. } => {
                let name = to
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| to.display().to_string());
                format!("Rename \"{name}\"")
            }
            Self::Move { pairs } => format!("Move {} item(s)", pairs.len()),
            Self::Copy { created } => format!("Copy {} item(s)", created.len()),
            Self::CreateDir { path } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("Create folder \"{name}\"")
            }
            Self::CreateFile { path } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("Create file \"{name}\"")
            }
        }
    }

    fn execute_undo(&self) -> Result<Option<Self>, String> {
        match self {
            Self::Rename { from, to } => {
                if !to.exists() {
                    return Err(format!("Cannot undo rename: \"{}\" no longer exists.", to.display()));
                }
                if from.exists() {
                    return Err(format!("Cannot undo rename: \"{}\" already exists.", from.display()));
                }
                std::fs::rename(to, from)
                    .map_err(|e| format!("Failed to undo rename: {e}"))?;
                Ok(Some(Self::Rename {
                    from: from.clone(),
                    to: to.clone(),
                }))
            }
            Self::Move { pairs } => {
                let mut undone = Vec::new();
                for (orig, dest) in pairs.iter().rev() {
                    if !dest.exists() {
                        continue;
                    }
                    if orig.exists() {
                        return Err(format!(
                            "Cannot undo move: \"{}\" already exists.",
                            orig.display()
                        ));
                    }
                    if let Some(parent) = orig.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::rename(dest, orig).is_err() {
                        copy_recursively_simple(dest, orig)?;
                        remove_path_simple(dest)?;
                    }
                    undone.push((orig.clone(), dest.clone()));
                }
                undone.reverse();
                Ok(Some(Self::Move { pairs: undone }))
            }
            Self::Copy { created } => {
                let existing: Vec<PathBuf> =
                    created.iter().filter(|p| p.exists()).cloned().collect();
                if !existing.is_empty() {
                    move_to_trash(&existing)?;
                }
                // Copy undo moves copies to Recycle Bin and is not directly redoable.
                Ok(None)
            }
            Self::CreateDir { path } => {
                if path.is_dir() && std::fs::remove_dir(path).is_err() {
                    move_to_trash(std::slice::from_ref(path))?;
                }
                Ok(Some(Self::CreateDir { path: path.clone() }))
            }
            Self::CreateFile { path } => {
                if path.is_file() {
                    move_to_trash(std::slice::from_ref(path))?;
                }
                Ok(Some(Self::CreateFile { path: path.clone() }))
            }
        }
    }

    fn execute_redo(&self) -> Result<Self, String> {
        match self {
            Self::Rename { from, to } => {
                if !from.exists() {
                    return Err(format!(
                        "Cannot redo rename: \"{}\" no longer exists.",
                        from.display()
                    ));
                }
                if to.exists() {
                    return Err(format!(
                        "Cannot redo rename: \"{}\" already exists.",
                        to.display()
                    ));
                }
                std::fs::rename(from, to)
                    .map_err(|e| format!("Failed to redo rename: {e}"))?;
                Ok(self.clone())
            }
            Self::Move { pairs } => {
                for (orig, dest) in pairs {
                    if !orig.exists() {
                        continue;
                    }
                    if dest.exists() {
                        return Err(format!(
                            "Cannot redo move: \"{}\" already exists.",
                            dest.display()
                        ));
                    }
                    if let Some(parent) = dest.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::rename(orig, dest).is_err() {
                        copy_recursively_simple(orig, dest)?;
                        remove_path_simple(orig)?;
                    }
                }
                Ok(self.clone())
            }
            Self::CreateDir { path } => {
                std::fs::create_dir_all(path)
                    .map_err(|e| format!("Failed to redo folder creation: {e}"))?;
                Ok(self.clone())
            }
            Self::CreateFile { path } => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|e| format!("Failed to redo file creation: {e}"))?;
                Ok(self.clone())
            }
            Self::Copy { .. } => Err("Redoing an undone copy is not supported.".to_string()),
        }
    }
}

fn copy_recursively_simple(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(src)
        .map_err(|e| format!("Failed reading \"{}\": {e}", src.display()))?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst)
            .map_err(|e| format!("Failed creating \"{}\": {e}", dst.display()))?;
        for entry in std::fs::read_dir(src)
            .map_err(|e| format!("Failed listing \"{}\": {e}", src.display()))?
        {
            let entry = entry.map_err(|e| format!("Failed reading dir entry: {e}"))?;
            copy_recursively_simple(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(src, dst)
            .map_err(|e| format!("Failed copying \"{}\": {e}", src.display()))?;
    }
    Ok(())
}

fn remove_path_simple(path: &std::path::Path) -> Result<(), String> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
            .map_err(|e| format!("Failed removing \"{}\": {e}", path.display()))
    } else {
        std::fs::remove_file(path)
            .map_err(|e| format!("Failed removing \"{}\": {e}", path.display()))
    }
}

/// Bounded stack storing up to 100 undoable/redoable filesystem operations.
#[derive(Debug, Default)]
pub struct UndoHistory {
    undo_stack: Vec<UndoAction>,
    redo_stack: Vec<UndoAction>,
}

impl UndoHistory {
    const MAX_ENTRIES: usize = 100;

    pub fn push(&mut self, action: UndoAction) {
        self.undo_stack.push(action);
        if self.undo_stack.len() > Self::MAX_ENTRIES {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> Result<String, String> {
        let Some(action) = self.undo_stack.pop() else {
            return Err("Nothing to undo.".to_string());
        };
        let label = action.label();
        match action.execute_undo() {
            Ok(Some(redo_action)) => {
                self.redo_stack.push(redo_action);
                Ok(format!("Undid {label}"))
            }
            Ok(None) => Ok(format!("Undid {label}")),
            Err(err) => {
                // Keep stack intact on failure so user can resolve the blocker and retry.
                self.undo_stack.push(action);
                Err(err)
            }
        }
    }

    pub fn redo(&mut self) -> Result<String, String> {
        let Some(action) = self.redo_stack.pop() else {
            return Err("Nothing to redo.".to_string());
        };
        let label = action.label();
        match action.execute_redo() {
            Ok(undo_action) => {
                self.undo_stack.push(undo_action);
                Ok(format!("Redid {label}"))
            }
            Err(err) => {
                self.redo_stack.push(action);
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_rename_and_move_round_trip() {
        let root = std::env::temp_dir().join(format!("texplorer_undo_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir_a = root.join("A");
        let dir_b = root.join("B");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let f1 = dir_a.join("old.txt");
        let f2 = dir_a.join("new.txt");
        std::fs::write(&f1, b"hello").unwrap();
        std::fs::rename(&f1, &f2).unwrap();

        let mut hist = UndoHistory::default();
        hist.push(UndoAction::Rename {
            from: f1.clone(),
            to: f2.clone(),
        });

        assert!(hist.can_undo());
        hist.undo().unwrap();
        assert!(f1.exists() && !f2.exists());

        assert!(hist.can_redo());
        hist.redo().unwrap();
        assert!(f2.exists() && !f1.exists());

        let moved = dir_b.join("new.txt");
        std::fs::rename(&f2, &moved).unwrap();
        hist.push(UndoAction::Move {
            pairs: vec![(f2.clone(), moved.clone())],
        });
        hist.undo().unwrap();
        assert!(f2.exists() && !moved.exists());
        hist.redo().unwrap();
        assert!(moved.exists() && !f2.exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
