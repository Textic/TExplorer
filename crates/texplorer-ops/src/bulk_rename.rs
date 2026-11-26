//! Composable Bulk Rename engine with live preview, regex capture replacement,
//! template expansion (`{name}`, `{ext}`, `{n:03}`, `{date}`), casing transformations,
//! upfront collision detection, and atomic two-phase execution returning `UndoAction::BulkRename`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Local};
use regex::RegexBuilder;
use uuid::Uuid;

use crate::undo::UndoAction;

/// Text casing transformation applied to the filename stem during Bulk Rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenameCasing {
    #[default]
    Unchanged,
    Lowercase,
    Uppercase,
    TitleCase,
    SnakeCase,
    KebabCase,
}

impl RenameCasing {
    pub const ALL: [Self; 6] = [
        Self::Unchanged,
        Self::Lowercase,
        Self::Uppercase,
        Self::TitleCase,
        Self::SnakeCase,
        Self::KebabCase,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "Keep",
            Self::Lowercase => "lower",
            Self::Uppercase => "UPPER",
            Self::TitleCase => "Title",
            Self::SnakeCase => "snake_case",
            Self::KebabCase => "kebab-case",
        }
    }
}

/// Composable rule set controlling Bulk Rename transformations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkRenameSpec {
    pub find: String,
    pub replace: String,
    pub use_regex: bool,
    pub case_sensitive: bool,
    /// Optional template applied to the filename (`{name}`, `{ext}`, `{n}`, `{n:03}`, `{date}`).
    /// When empty, keeps the find/replace + casing output.
    pub template: String,
    pub counter_start: usize,
    pub counter_step: usize,
    pub casing: RenameCasing,
    /// Optional extension override (without leading dot; empty leaves original extension intact).
    pub extension_override: String,
}

impl Default for BulkRenameSpec {
    fn default() -> Self {
        Self {
            find: String::new(),
            replace: String::new(),
            use_regex: false,
            case_sensitive: false,
            template: String::new(),
            counter_start: 1,
            counter_step: 1,
            casing: RenameCasing::Unchanged,
            extension_override: String::new(),
        }
    }
}

/// Live preview row for one item in a Bulk Rename batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkRenamePreviewRow {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub old_name: String,
    pub new_name: String,
    pub changed: bool,
    pub conflict: Option<String>,
}

/// Evaluates `spec` across `sources` without touching disk, reporting exact collisions and validation errors.
pub fn preview_bulk_rename(
    sources: &[PathBuf],
    spec: &BulkRenameSpec,
) -> Result<Vec<BulkRenamePreviewRow>, String> {
    let compiled_regex = if !spec.find.is_empty() && spec.use_regex {
        Some(
            RegexBuilder::new(&spec.find)
                .case_insensitive(!spec.case_sensitive)
                .build()
                .map_err(|e| format!("Invalid regular expression: {e}"))?,
        )
    } else {
        None
    };

    let source_keys: HashSet<String> = sources
        .iter()
        .map(|p| p.to_string_lossy().to_ascii_lowercase())
        .collect();

    let mut rows = Vec::with_capacity(sources.len());
    let mut target_counts: HashMap<String, usize> = HashMap::with_capacity(sources.len());

    for (idx, src) in sources.iter().enumerate() {
        let old_name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_dir = src.is_dir();
        let (orig_stem, orig_ext) = split_name_ext(&old_name, is_dir);
        let counter_val = spec
            .counter_start
            .saturating_add(idx.saturating_mul(spec.counter_step.max(1)));
        let date_str = file_modified_date_ymd(src);

        // Step 1: Find & Replace on full filename or stem
        let mut working_name = if spec.find.is_empty() {
            old_name.clone()
        } else if let Some(re) = &compiled_regex {
            re.replace_all(&old_name, spec.replace.as_str()).into_owned()
        } else if spec.case_sensitive {
            old_name.replace(&spec.find, &spec.replace)
        } else {
            replace_ascii_case_insensitive(&old_name, &spec.find, &spec.replace)
        };

        // Step 2: Optional template expansion (`{name}`, `{ext}`, `{n}`, `{n:03}`, `{date}`)
        let (mut stem, mut ext) = split_name_ext(&working_name, is_dir);
        if !spec.template.trim().is_empty() {
            let expanded = expand_template(&spec.template, &stem, &ext, counter_val, &date_str);
            if spec.template.contains("{ext}") || is_dir {
                working_name = expanded;
                let (s, e) = split_name_ext(&working_name, is_dir);
                stem = s;
                ext = e;
            } else {
                stem = expanded;
            }
        }

        // Step 3: Casing on stem
        stem = apply_casing(&stem, spec.casing);

        // Step 4: Optional extension override (for files)
        let ext_override = spec.extension_override.trim().trim_start_matches('.');
        if !is_dir && !ext_override.is_empty() {
            ext = ext_override.to_string();
        } else if !is_dir && spec.template.trim().is_empty() && spec.find.is_empty() {
            ext = orig_ext;
        }
        let _ = orig_stem;

        let new_name = if !is_dir && !ext.is_empty() {
            format!("{stem}.{ext}")
        } else {
            stem
        };

        let parent = src.parent().unwrap_or_else(|| Path::new(""));
        let destination = parent.join(&new_name);
        let changed = new_name != old_name;
        let dest_key = destination.to_string_lossy().to_ascii_lowercase();
        *target_counts.entry(dest_key).or_insert(0) += 1;

        let mut conflict = validate_filename(&new_name).err();
        if conflict.is_none() && changed {
            let dest_lower = destination.to_string_lossy().to_ascii_lowercase();
            let src_lower = src.to_string_lossy().to_ascii_lowercase();
            // Case-only rename on Windows is valid; collision only if target exists and isn't in batch.
            if dest_lower != src_lower && destination.exists() && !source_keys.contains(&dest_lower)
            {
                conflict = Some("Target already exists on disk".to_string());
            }
        }

        rows.push(BulkRenamePreviewRow {
            source: src.clone(),
            destination,
            old_name,
            new_name,
            changed,
            conflict,
        });
    }

    // Mark duplicate destination names inside the same batch.
    for row in &mut rows {
        let dest_key = row.destination.to_string_lossy().to_ascii_lowercase();
        if row.conflict.is_none() && target_counts.get(&dest_key).copied().unwrap_or(0) > 1 {
            row.conflict = Some("Duplicate target name in batch".to_string());
        }
    }

    Ok(rows)
}

/// Executes a conflict-free Bulk Rename batch atomically using a two-phase temporary staging
/// strategy so overlapping/swapped names (`a -> b`, `b -> a`) never collide.
pub fn execute_bulk_rename(rows: &[BulkRenamePreviewRow]) -> Result<UndoAction, String> {
    if let Some(bad) = rows.iter().find(|r| r.conflict.is_some()) {
        return Err(format!(
            "Cannot apply Bulk Rename: \"{}\" — {}",
            bad.old_name,
            bad.conflict.as_deref().unwrap_or("conflict")
        ));
    }

    let active: Vec<&BulkRenamePreviewRow> = rows.iter().filter(|r| r.changed).collect();
    if active.is_empty() {
        return Err("No items would be renamed with the current rules.".to_string());
    }

    // Phase 1: Move each source to a unique temporary sibling name.
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::with_capacity(active.len());
    for row in &active {
        let parent = row.source.parent().unwrap_or_else(|| Path::new(""));
        let temp_name = format!(".texplorer_ren_{}", Uuid::new_v4().simple());
        let temp_path = parent.join(temp_name);
        if let Err(err) = std::fs::rename(&row.source, &temp_path) {
            // Roll back already staged items before failing.
            for (orig, tmp, _) in staged.iter().rev() {
                let _ = std::fs::rename(tmp, orig);
            }
            return Err(format!(
                "Failed staging rename for \"{}\": {err}",
                row.old_name
            ));
        }
        staged.push((row.source.clone(), temp_path, row.destination.clone()));
    }

    // Phase 2: Move each staged temp item to its final destination.
    let mut completed: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(staged.len());
    for (i, (orig, temp_path, dest)) in staged.iter().enumerate() {
        if let Err(err) = std::fs::rename(temp_path, dest) {
            // Roll back both completed renames and remaining staged files.
            for (o, d) in completed.iter().rev() {
                let _ = std::fs::rename(d, o);
            }
            for (o, tmp, _) in &staged[i..] {
                let _ = std::fs::rename(tmp, o);
            }
            return Err(format!(
                "Failed finalizing rename to \"{}\": {err}",
                dest.display()
            ));
        }
        completed.push((orig.clone(), dest.clone()));
    }

    Ok(UndoAction::BulkRename { pairs: completed })
}

fn split_name_ext(name: &str, is_dir: bool) -> (String, String) {
    if is_dir {
        return (name.to_string(), String::new());
    }
    if let Some((stem, ext)) = name.rsplit_once('.')
        && !stem.is_empty()
    {
        return (stem.to_string(), ext.to_string());
    }
    (name.to_string(), String::new())
}

fn expand_template(
    template: &str,
    stem: &str,
    ext: &str,
    counter: usize,
    date: &str,
) -> String {
    let mut out = String::with_capacity(template.len() + stem.len() + 16);
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        if chars[i] == '{'
            && let Some(rel_end) = chars[i + 1..].iter().position(|&c| c == '}')
        {
            let token: String = chars[i + 1..i + 1 + rel_end].iter().collect();
            let replaced = if token.eq_ignore_ascii_case("name") {
                Some(stem.to_string())
            } else if token.eq_ignore_ascii_case("ext") {
                Some(ext.to_string())
            } else if token.eq_ignore_ascii_case("date") {
                Some(date.to_string())
            } else if token.eq_ignore_ascii_case("n") {
                Some(counter.to_string())
            } else if let Some(pad_spec) = token.strip_prefix("n:").or_else(|| token.strip_prefix("N:"))
                && let Ok(width) = pad_spec.parse::<usize>()
            {
                Some(format!("{counter:0width$}", width = width.min(12)))
            } else {
                None
            };
            if let Some(val) = replaced {
                out.push_str(&val);
                i += rel_end + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    out
}

fn apply_casing(s: &str, casing: RenameCasing) -> String {
    match casing {
        RenameCasing::Unchanged => s.to_string(),
        RenameCasing::Lowercase => s.to_lowercase(),
        RenameCasing::Uppercase => s.to_uppercase(),
        RenameCasing::TitleCase => {
            let mut out = String::with_capacity(s.len());
            let mut cap_next = true;
            for ch in s.chars() {
                if ch.is_alphanumeric() {
                    if cap_next {
                        out.extend(ch.to_uppercase());
                        cap_next = false;
                    } else {
                        out.extend(ch.to_lowercase());
                    }
                } else {
                    out.push(ch);
                    cap_next = true;
                }
            }
            out
        }
        RenameCasing::SnakeCase => join_words(s, '_'),
        RenameCasing::KebabCase => join_words(s, '-'),
    }
}

fn join_words(s: &str, sep: char) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;

    for ch in s.chars() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower_or_digit = false;
        } else {
            if ch.is_uppercase() && prev_lower_or_digit && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            cur.extend(ch.to_lowercase());
            prev_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words.join(&sep.to_string())
}

fn replace_ascii_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let hay_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    if hay_lower.len() != haystack.len() {
        return haystack.replace(needle, replacement);
    }
    let mut out = String::with_capacity(haystack.len());
    let mut pos = 0usize;
    while let Some(rel) = hay_lower[pos..].find(&needle_lower) {
        out.push_str(&haystack[pos..pos + rel]);
        out.push_str(replacement);
        pos += rel + needle_lower.len();
    }
    out.push_str(&haystack[pos..]);
    out
}

fn file_modified_date_ymd(path: &Path) -> String {
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .unwrap_or(UNIX_EPOCH);
    let dt: DateTime<Local> = mtime.into();
    dt.format("%Y-%m-%d").to_string()
}

fn validate_filename(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err("Resulting filename is empty".to_string());
    }
    const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    if trimmed.chars().any(|c| FORBIDDEN.contains(&c) || c.is_control()) {
        return Err("Contains illegal filename characters".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::undo::UndoHistory;

    #[test]
    fn previews_templates_regex_casing_and_detects_collisions() {
        let root = std::env::temp_dir().join(format!("texplorer_bulk_ren_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let f1 = root.join("PhotoOne.JPG");
        let f2 = root.join("PhotoTwo.JPG");
        std::fs::write(&f1, b"1").unwrap();
        std::fs::write(&f2, b"2").unwrap();

        let spec = BulkRenameSpec {
            template: "trip_{n:03}_{name}".to_string(),
            casing: RenameCasing::SnakeCase,
            extension_override: "jpg".to_string(),
            ..Default::default()
        };
        let rows = preview_bulk_rename(&[f1.clone(), f2.clone()], &spec).unwrap();
        assert_eq!(rows[0].new_name, "trip_001_photo_one.jpg");
        assert_eq!(rows[1].new_name, "trip_002_photo_two.jpg");
        assert!(rows.iter().all(|r| r.conflict.is_none()));

        let undo_act = execute_bulk_rename(&rows).unwrap();
        assert!(root.join("trip_001_photo_one.jpg").exists());
        assert!(root.join("trip_002_photo_two.jpg").exists());

        let mut hist = UndoHistory::default();
        hist.push(undo_act);
        hist.undo().unwrap();
        assert!(f1.exists() && f2.exists());
        hist.redo().unwrap();
        assert!(root.join("trip_001_photo_one.jpg").exists());

        // Verify duplicate collision detection
        let dup_spec = BulkRenameSpec {
            template: "same_name".to_string(),
            ..Default::default()
        };
        let dup_rows = preview_bulk_rename(
            &[
                root.join("trip_001_photo_one.jpg"),
                root.join("trip_002_photo_two.jpg"),
            ],
            &dup_spec,
        )
        .unwrap();
        assert!(dup_rows.iter().all(|r| r.conflict.is_some()));
        assert!(execute_bulk_rename(&dup_rows).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
