//! Virtual File System providers for T Explorer.

pub mod folder_size;
pub mod local;
pub mod search;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use texplorer_core::{FileEntry, Location};

/// Lists all entries for `location`.
///
/// `Location::Home` returns an empty entry list because the Home view renders dedicated
/// volume and quick-access cards instead of tabular file rows.
pub fn list_location(location: &Location) -> Result<Vec<FileEntry>, String> {
    match location {
        Location::Home => Ok(Vec::new()),
        Location::Local(path) => local::list_local_dir(path),
        Location::Archive { archive_path, .. } => Err(format!(
            "Archive virtual browsing for \"{}\" will be enabled in Phase 4.",
            archive_path.display()
        )),
        Location::Search { query, scope } => {
            let cancel = AtomicBool::new(false);
            search::execute_search(query, scope, true, 1_000, &cancel).map(|res| res.entries)
        }
    }
}

/// Creates a new empty subfolder inside `parent` and returns its full path.
pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let trimmed = validate_entry_name(name)?;
    let target = parent.join(trimmed);
    if target.exists() {
        return Err(format!("An item named \"{trimmed}\" already exists."));
    }
    std::fs::create_dir(&target)
        .map_err(|e| format!("Failed to create folder \"{trimmed}\": {e}"))?;
    Ok(target)
}

/// Creates a new empty file inside `parent` and returns its full path.
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let trimmed = validate_entry_name(name)?;
    let target = parent.join(trimmed);
    if target.exists() {
        return Err(format!("An item named \"{trimmed}\" already exists."));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| format!("Failed to create file \"{trimmed}\": {e}"))?;
    Ok(target)
}

/// Renames a local file or folder in place and returns its new path.
pub fn rename_item(old_path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let trimmed = validate_entry_name(new_name)?;
    let Some(parent) = old_path.parent() else {
        return Err("Cannot rename a root volume.".to_string());
    };
    let target = parent.join(trimmed);
    if target == old_path {
        return Ok(target);
    }
    if target.exists()
        && !old_path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(trimmed))
    {
        return Err(format!("An item named \"{trimmed}\" already exists."));
    }
    std::fs::rename(old_path, &target)
        .map_err(|e| format!("Failed to rename to \"{trimmed}\": {e}"))?;
    Ok(target)
}

/// Validates a filename against illegal path separators and Windows reserved characters.
pub fn validate_entry_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err("Name cannot be empty or relative.".to_string());
    }
    const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    if trimmed.chars().any(|c| FORBIDDEN.contains(&c) || c.is_control()) {
        return Err("Name contains unsupported characters (< > : \" / \\ | ? *).".to_string());
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_renames_and_lists_local_entries() {
        let root = std::env::temp_dir().join(format!("texplorer_vfs_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let dir = create_folder(&root, "Alpha").unwrap();
        let file = create_file(&root, "notes.rs").unwrap();
        std::fs::write(&file, b"fn main() {}").unwrap();

        let renamed = rename_item(&file, "main.rs").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "main.rs");

        let entries = list_location(&Location::local(&root)).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.name == "Alpha" && e.is_dir()));
        assert!(
            entries
                .iter()
                .any(|e| e.name == "main.rs" && !e.is_dir() && e.size == Some(12))
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = dir;
    }

    #[test]
    fn rejects_invalid_filenames() {
        assert!(validate_entry_name("").is_err());
        assert!(validate_entry_name("..").is_err());
        assert!(validate_entry_name("bad/name").is_err());
        assert!(validate_entry_name("file?.txt").is_err());
        assert_eq!(validate_entry_name("  Cargo.toml  ").unwrap(), "Cargo.toml");
    }
}
