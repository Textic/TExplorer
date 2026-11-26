use std::collections::BTreeSet;

use uuid::Uuid;

use crate::entry::{FileEntry, SortSpec, ViewMode};
use crate::location::Location;

/// Identifies one side of a split tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
        self.selected.clear();
        for i in 0..total {
            self.selected.insert(i);
        }
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
        self.history_forward.push(cur);
        self.quick_filter.clear();
        self.selection.clear();
        self.error = None;
        Some(prev)
    }

    pub fn go_forward(&mut self) -> Option<Location> {
        let next = self.history_forward.pop()?;
        let cur = std::mem::replace(&mut self.location, next.clone());
        self.history_back.push(cur);
        self.quick_filter.clear();
        self.selection.clear();
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
    }

    /// Re-sorts existing entries in place and refreshes the filtered view.
    pub fn resort(&mut self) {
        let previously_selected: Vec<Location> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.location.clone())
            .collect();

        crate::entry::sort_entries(&mut self.entries, self.sort);
        self.rebuild_filter();

        self.selection.clear();
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

    /// Rebuilds `filtered_indices` using `show_hidden` and `quick_filter`.
    pub fn rebuild_filter(&mut self) {
        let needle = self.quick_filter.trim().to_ascii_lowercase();
        self.filtered_indices.clear();
        self.filtered_indices.reserve(self.entries.len());

        for (idx, entry) in self.entries.iter().enumerate() {
            if !self.show_hidden && entry.is_hidden() {
                continue;
            }
            if !needle.is_empty() && !entry.name.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            self.filtered_indices.push(idx);
        }

        self.selection.clamp_to(self.filtered_indices.len());
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

    /// Type-ahead search: finds the next visible item starting with `prefix` (case-insensitive).
    pub fn find_type_ahead(&self, prefix: &str) -> Option<usize> {
        let needle = prefix.to_ascii_lowercase();
        if needle.is_empty() || self.filtered_indices.is_empty() {
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
                && entry.name.to_ascii_lowercase().starts_with(&needle)
            {
                return Some(vis_idx);
            }
        }
        None
    }
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
}
