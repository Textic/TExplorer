use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entry::{FileEntry, SortSpec, ViewMode};
use crate::location::Location;

/// Identifies one side of a split tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PaneSide {
    #[default]
    Left,
    Right,
}

impl PaneSide {
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Multi-selection state supporting single-click, Ctrl-toggle, Shift-range, and keyboard navigation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionSet {
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub cursor: Option<usize>,
}

impl SelectionSet {
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.selected.contains(&idx)
    }

    pub fn select_only(&mut self, idx: usize) {
        self.selected.clear();
        self.selected.insert(idx);
        self.anchor = Some(idx);
        self.cursor = Some(idx);
    }

    pub fn toggle(&mut self, idx: usize) {
        if !self.selected.remove(&idx) {
            self.selected.insert(idx);
        }
        self.anchor = Some(idx);
        self.cursor = Some(idx);
    }

    pub fn select_range(&mut self, to_idx: usize, keep_existing: bool) {
        let from_idx = self.anchor.unwrap_or(to_idx);
        if !keep_existing {
            self.selected.clear();
        }
        let (start, end) = if from_idx <= to_idx {
            (from_idx, to_idx)
        } else {
            (to_idx, from_idx)
        };
        for i in start..=end {
            self.selected.insert(i);
        }
        if self.anchor.is_none() {
            self.anchor = Some(from_idx);
        }
        self.cursor = Some(to_idx);
    }

    pub fn select_all(&mut self, total: usize) {
        self.selected = (0..total).collect();
        if total > 0 {
            self.anchor = Some(0);
            self.cursor = Some(total - 1);
        } else {
            self.anchor = None;
            self.cursor = None;
        }
    }

    /// Moves the selection cursor by `delta` rows, optionally extending the selection range.
    pub fn move_cursor(&mut self, delta: isize, total: usize, extend: bool) -> Option<usize> {
        if total == 0 {
            self.clear();
            return None;
        }
        let next = match self.cursor {
            Some(cur) => cur
                .saturating_add_signed(delta)
                .min(total.saturating_sub(1)),
            None => {
                if delta >= 0 {
                    0
                } else {
                    total.saturating_sub(1)
                }
            }
        };

        if extend {
            self.select_range(next, false);
        } else {
            self.select_only(next);
        }
        Some(next)
    }

    /// Jumps to the first (`to_end = false`) or last (`to_end = true`) item in the list.
    /// When `extend` is `true` (e.g. `Ctrl+Shift+Down` / `Ctrl+Shift+Up` / `Shift+End` / `Shift+Home`),
    /// selects every item between the current anchor/cursor and that boundary.
    pub fn select_to_boundary(&mut self, to_end: bool, total: usize, extend: bool) -> Option<usize> {
        if total == 0 {
            self.clear();
            return None;
        }
        let target = if to_end { total - 1 } else { 0 };
        if extend {
            if self.anchor.is_none() {
                self.anchor = self.cursor.or(Some(if to_end { 0 } else { total - 1 }));
            }
            self.select_range(target, false);
        } else {
            self.select_only(target);
        }
        Some(target)
    }

    /// Ensures indices stay valid after the visible list shrinks or reloads.
    pub fn clamp_to(&mut self, total: usize) {
        self.selected.retain(|&idx| idx < total);
        if let Some(a) = self.anchor
            && a >= total
        {
            self.anchor = total.checked_sub(1);
        }
        if let Some(c) = self.cursor
            && c >= total
        {
            self.cursor = total.checked_sub(1);
        }
    }
}

/// Pure navigation and listing model for a single explorer pane.
#[derive(Debug, Clone)]
pub struct PaneModel {
    pub id: Uuid,
    pub location: Location,
    pub history_back: Vec<Location>,
    pub history_forward: Vec<Location>,
    pub view_mode: ViewMode,
    pub sort: SortSpec,
    pub show_hidden: bool,
    pub quick_filter: String,
    pub entries: Vec<FileEntry>,
    pub filtered_indices: Vec<usize>,
    pub selection: SelectionSet,
    /// Cached byte total of selected entries so status-bar rendering stays O(1) on 100k+ items.
    pub selected_bytes: u64,
    pub loading: bool,
    pub error: Option<String>,
    pub load_generation: u64,
    /// Optional filename to select automatically once the current directory load finishes.
    pub pending_select_name: Option<String>,
}

impl PaneModel {
    pub fn new(location: Location, view_mode: ViewMode, show_hidden: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            location,
            history_back: Vec::new(),
            history_forward: Vec::new(),
            view_mode,
            sort: SortSpec::default(),
            show_hidden,
            quick_filter: String::new(),
            entries: Vec::new(),
            filtered_indices: Vec::new(),
            selection: SelectionSet::default(),
            selected_bytes: 0,
            loading: false,
            error: None,
            load_generation: 0,
            pending_select_name: None,
        }
    }

    /// Navigates to a new location, recording history unless already at that location.
    pub fn navigate_to(&mut self, target: Location) -> bool {
        if self.location == target {
            return false;
        }
        let previous = std::mem::replace(&mut self.location, target);
        self.history_back.push(previous);
        if self.history_back.len() > 128 {
            self.history_back.remove(0);
        }
        self.history_forward.clear();
        self.quick_filter.clear();
        self.selection.clear();
        self.selected_bytes = 0;
        self.error = None;
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.history_back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.history_forward.is_empty()
    }

    pub fn can_go_up(&self) -> bool {
        self.location.parent().is_some()
    }

    pub fn go_back(&mut self) -> Option<Location> {
        let prev = self.history_back.pop()?;
        let cur = std::mem::replace(&mut self.location, prev.clone());
        // If stepping back to the immediate parent folder, highlight the child folder we came from
        // so keyboard users can seamlessly press Down/Right into sibling folders.
        let child_name = if cur.parent().as_ref() == Some(&prev) {
            match &cur {
                Location::Local(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()),
                Location::Archive {
                    archive_path,
                    inner_path,
                } => {
                    if inner_path.is_empty() {
                        archive_path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                    } else {
                        inner_path
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .map(|s| s.to_string())
                    }
                }
                Location::Remote { remote_path, .. } => remote_path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                _ => None,
            }
        } else {
            None
        };
        self.history_forward.push(cur);
        self.quick_filter.clear();
        self.selection.clear();
        self.selected_bytes = 0;
        self.error = None;
        self.pending_select_name = child_name;
        Some(prev)
    }

    pub fn go_forward(&mut self) -> Option<Location> {
        let next = self.history_forward.pop()?;
        let cur = std::mem::replace(&mut self.location, next.clone());
        self.history_back.push(cur);
        self.quick_filter.clear();
        self.selection.clear();
        self.selected_bytes = 0;
        self.error = None;
        Some(next)
    }

    pub fn go_up(&mut self) -> Option<Location> {
        let parent = self.location.parent()?;
        let previous_name = match &self.location {
            Location::Local(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            _ => None,
        };
        if self.navigate_to(parent.clone()) {
            self.pending_select_name = previous_name;
            Some(parent)
        } else {
            None
        }
    }

    /// Replaces the raw entries, applies sorting and filtering, and restores pending selection if any.
    pub fn set_entries(&mut self, mut entries: Vec<FileEntry>) {
        crate::entry::sort_entries(&mut entries, self.sort);
        self.set_presorted_entries(entries);
    }

    /// Replaces the raw entries assuming they were already sorted off the UI thread by `self.sort`.
    pub fn set_presorted_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.loading = false;
        self.error = None;
        self.rebuild_filter();

        if let Some(target_name) = self.pending_select_name.take()
            && let Some(visible_idx) = self.filtered_indices.iter().position(|&idx| {
                self.entries[idx]
                    .name
                    .eq_ignore_ascii_case(&target_name)
            })
        {
            self.selection.select_only(visible_idx);
        }
        self.recompute_selection_bytes();
    }

    /// Recomputes `selected_bytes` after any selection or folder-size change so UI renders in O(1).
    pub fn recompute_selection_bytes(&mut self) {
        self.selected_bytes = self
            .selection
            .selected
            .iter()
            .filter_map(|&vis_idx| {
                let entry_idx = *self.filtered_indices.get(vis_idx)?;
                self.entries.get(entry_idx)?.size
            })
            .sum();
    }

    /// Re-sorts existing entries in place and refreshes the filtered view in O(N log N) time.
    pub fn resort(&mut self) {
        let previously_selected: HashSet<Location> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.location.clone())
            .collect();

        crate::entry::sort_entries(&mut self.entries, self.sort);
        self.rebuild_filter();

        self.selection.clear();
        if !previously_selected.is_empty() {
            for (vis_idx, &entry_idx) in self.filtered_indices.iter().enumerate() {
                if previously_selected.contains(&self.entries[entry_idx].location) {
                    self.selection.selected.insert(vis_idx);
                    if self.selection.anchor.is_none() {
                        self.selection.anchor = Some(vis_idx);
                    }
                    self.selection.cursor = Some(vis_idx);
                }
            }
        }
        self.recompute_selection_bytes();
    }

    /// Rebuilds `filtered_indices` using `show_hidden` and `quick_filter` with zero per-entry heap allocations.
    pub fn rebuild_filter(&mut self) {
        let needle = self.quick_filter.trim().to_ascii_lowercase();
        let needle_bytes = needle.as_bytes();
        self.filtered_indices.clear();
        self.filtered_indices.reserve(self.entries.len());

        for (idx, entry) in self.entries.iter().enumerate() {
            if !self.show_hidden && entry.is_hidden() {
                continue;
            }
            if !needle_bytes.is_empty()
                && !contains_ascii_case_insensitive(entry.name.as_bytes(), needle_bytes)
            {
                continue;
            }
            self.filtered_indices.push(idx);
        }

        self.selection.clamp_to(self.filtered_indices.len());
        self.recompute_selection_bytes();
    }

    /// Returns the visible entry at `visible_idx`.
    pub fn visible_entry(&self, visible_idx: usize) -> Option<&FileEntry> {
        let entry_idx = *self.filtered_indices.get(visible_idx)?;
        self.entries.get(entry_idx)
    }

    /// Returns all currently selected entries in visible order.
    pub fn selected_entries(&self) -> Vec<&FileEntry> {
        self.selection
            .selected
            .iter()
            .filter_map(|&vis_idx| self.visible_entry(vis_idx))
            .collect()
    }

    /// Returns the primary focused entry (cursor first, otherwise first selected).
    pub fn primary_selected_entry(&self) -> Option<&FileEntry> {
        if let Some(cur) = self.selection.cursor
            && self.selection.contains(cur)
        {
            return self.visible_entry(cur);
        }
        self.selection
            .selected
            .iter()
            .next()
            .and_then(|&idx| self.visible_entry(idx))
    }

    /// Type-ahead search: finds the next visible item starting with `prefix` (zero heap allocations).
    pub fn find_type_ahead(&self, prefix: &str) -> Option<usize> {
        let needle = prefix.to_ascii_lowercase();
        let needle_bytes = needle.as_bytes();
        if needle_bytes.is_empty() || self.filtered_indices.is_empty() {
            return None;
        }
        let start = self
            .selection
            .cursor
            .map(|c| (c + 1) % self.filtered_indices.len())
            .unwrap_or(0);

        for offset in 0..self.filtered_indices.len() {
            let vis_idx = (start + offset) % self.filtered_indices.len();
            if let Some(entry) = self.visible_entry(vis_idx)
                && starts_with_ascii_case_insensitive(entry.name.as_bytes(), needle_bytes)
            {
                return Some(vis_idx);
            }
        }
        None
    }
}

#[inline]
fn starts_with_ascii_case_insensitive(haystack: &[u8], needle_lower: &[u8]) -> bool {
    haystack.len() >= needle_lower.len()
        && haystack[..needle_lower.len()]
            .iter()
            .zip(needle_lower)
            .all(|(&h, &n)| h.to_ascii_lowercase() == n)
}

#[inline]
fn contains_ascii_case_insensitive(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.len() < needle_lower.len() {
        return false;
    }
    let first = needle_lower[0];
    let last_start = haystack.len() - needle_lower.len();
    for start in 0..=last_start {
        if haystack[start].to_ascii_lowercase() == first
            && haystack[start + 1..start + needle_lower.len()]
                .iter()
                .zip(&needle_lower[1..])
                .all(|(&h, &n)| h.to_ascii_lowercase() == n)
        {
            return true;
        }
    }
    false
}

/// A top-level browser tab containing either a single pane or a Dual-Pane split.
#[derive(Debug, Clone)]
pub struct TabModel {
    pub id: Uuid,
    pub split_enabled: bool,
    pub active_pane: PaneSide,
    pub left: PaneModel,
    pub right: Option<PaneModel>,
}

impl TabModel {
    pub fn new(location: Location, view_mode: ViewMode, show_hidden: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            split_enabled: false,
            active_pane: PaneSide::Left,
            left: PaneModel::new(location, view_mode, show_hidden),
            right: None,
        }
    }

    pub fn title(&self) -> String {
        self.active_pane_ref().location.tab_title()
    }

    pub fn pane(&self, side: PaneSide) -> &PaneModel {
        match side {
            PaneSide::Left => &self.left,
            PaneSide::Right => self.right.as_ref().unwrap_or(&self.left),
        }
    }

    pub fn pane_mut(&mut self, side: PaneSide) -> &mut PaneModel {
        match side {
            PaneSide::Left => &mut self.left,
            PaneSide::Right => self.right.as_mut().unwrap_or(&mut self.left),
        }
    }

    pub fn active_pane_ref(&self) -> &PaneModel {
        self.pane(self.active_pane)
    }

    pub fn active_pane_mut(&mut self) -> &mut PaneModel {
        let side = self.active_pane;
        self.pane_mut(side)
    }

    /// Toggles Dual-Pane split for this tab, cloning the left pane's location on first enable.
    pub fn toggle_split(&mut self) -> bool {
        if self.split_enabled {
            self.split_enabled = false;
            self.active_pane = PaneSide::Left;
            false
        } else {
            self.split_enabled = true;
            if self.right.is_none() {
                self.right = Some(PaneModel::new(
                    self.left.location.clone(),
                    self.left.view_mode,
                    self.left.show_hidden,
                ));
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_set_handles_single_toggle_range_and_arrows() {
        let mut sel = SelectionSet::default();
        sel.select_only(1);
        assert_eq!(sel.len(), 1);
        assert!(sel.contains(1));

        sel.select_range(4, false);
        assert_eq!(sel.selected.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3, 4]);

        sel.toggle(2);
        assert!(!sel.contains(2));
        assert_eq!(sel.len(), 3);

        sel.move_cursor(1, 5, false);
        assert_eq!(sel.selected.iter().copied().collect::<Vec<_>>(), vec![3]);

        // Ctrl+Shift+Down from index 3 selects 3..=7 (anchor stays at 3).
        assert_eq!(sel.select_to_boundary(true, 8, true), Some(7));
        assert_eq!(
            sel.selected.iter().copied().collect::<Vec<_>>(),
            vec![3, 4, 5, 6, 7]
        );
        assert_eq!(sel.anchor, Some(3));
        assert_eq!(sel.cursor, Some(7));

        // Flipping immediately to Ctrl+Shift+Up selects 0..=3 around the same anchor.
        assert_eq!(sel.select_to_boundary(false, 8, true), Some(0));
        assert_eq!(
            sel.selected.iter().copied().collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(sel.anchor, Some(3));
        assert_eq!(sel.cursor, Some(0));
    }

    #[test]
    fn pane_history_and_dual_pane_split_work() {
        let mut tab = TabModel::new(Location::Home, ViewMode::Details, false);
        assert_eq!(tab.title(), "Home");

        assert!(tab.left.navigate_to(Location::local(r"X:\Projects")));
        assert!(tab.left.can_go_back());
        assert_eq!(tab.left.go_back(), Some(Location::Home));
        assert!(tab.left.can_go_forward());
        assert_eq!(tab.left.go_forward(), Some(Location::local(r"X:\Projects")));

        assert!(tab.toggle_split());
        assert!(tab.split_enabled);
        assert_eq!(tab.pane(PaneSide::Right).location, Location::local(r"X:\Projects"));
    }

    #[test]
    fn handles_100k_entries_sort_filter_and_select_all_fast() {
        use crate::entry::{EntryFlags, FileKind, SortColumn};

        let mut pane = PaneModel::new(Location::local(r"X:\BigDir"), ViewMode::Details, true);
        let entries: Vec<FileEntry> = (0..100_000usize)
            .map(|i| FileEntry {
                name: format!("asset_{:06}.png", 99_999 - i),
                extension: "png".to_string(),
                location: Location::Local(std::path::PathBuf::from(format!(
                    r"X:\BigDir\asset_{:06}.png",
                    99_999 - i
                ))),
                size: Some((i as u64 + 1) * 64),
                modified_secs: 1_700_000_000 + i as i64,
                created_secs: 1_700_000_000 + i as i64,
                flags: EntryFlags::empty(),
                kind: FileKind::Image,
                type_label: "PNG Image".to_string(),
            })
            .collect();

        pane.set_entries(entries);
        assert_eq!(pane.filtered_indices.len(), 100_000);
        assert_eq!(pane.visible_entry(0).unwrap().name, "asset_000000.png");

        pane.selection.select_all(pane.filtered_indices.len());
        pane.recompute_selection_bytes();
        assert_eq!(pane.selection.len(), 100_000);
        assert!(pane.selected_bytes > 0);

        // Resort by Type and Size while all 100k items are selected (must stay O(N log N), not O(N^2)).
        pane.sort.toggle_column(SortColumn::Type);
        pane.resort();
        assert_eq!(pane.selection.len(), 100_000);

        pane.sort.toggle_column(SortColumn::Size);
        pane.resort();
        assert_eq!(pane.visible_entry(0).unwrap().name, "asset_000000.png");

        // Zero-allocation quick filter and type-ahead.
        pane.quick_filter = "054321".to_string();
        pane.rebuild_filter();
        assert_eq!(pane.filtered_indices.len(), 1);
        assert_eq!(pane.find_type_ahead("asset_054321"), Some(0));
    }
}
