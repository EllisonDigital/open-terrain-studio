//! Undo/redo for every graph, parameter and world edit.
//!
//! Each step stores a full copy of the editable state from before the edit.
//! Graphs are small (parameters, not terrain data), so copies are cheap and
//! undo can never drift out of sync with the edits it reverses.

use crate::graph::Graph;
use crate::project::{ExportSpec, Project};
use crate::world::World;

/// Default number of undo steps kept.
pub const DEFAULT_LIMIT: usize = 500;

/// Edits of the same thing (e.g. dragging one slider) closer together than
/// this, in seconds, merge into one undo step.
pub const MERGE_WINDOW_S: f64 = 1.0;

/// The part of a project that undo restores. Build settings and editor state
/// (camera, viewed node) are deliberately not included.
#[derive(Clone, Debug, PartialEq)]
pub struct EditState {
    pub world: World,
    pub graph: Graph,
    pub exports: Vec<ExportSpec>,
}

impl EditState {
    pub fn of(project: &Project) -> Self {
        Self {
            world: project.world.clone(),
            graph: project.graph.clone(),
            exports: project.exports.clone(),
        }
    }

    pub fn apply_to(self, project: &mut Project) {
        project.world = self.world;
        project.graph = self.graph;
        project.exports = self.exports;
    }
}

struct Step {
    label: String,
    state: EditState,
    revision: u64,
}

/// Undo and redo stacks, plus a revision counter that tells whether the
/// project differs from the last saved version.
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    limit: usize,
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
    /// Merge key and time of the last recorded edit.
    last: Option<(String, f64)>,
    /// Label of the open group, and whether it has recorded a step yet.
    group: Option<(String, bool)>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_LIMIT)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            revision: 0,
            next_revision: 1,
            saved_revision: 0,
            last: None,
            group: None,
        }
    }

    /// Forget everything (e.g. after loading a project). The current state
    /// counts as saved.
    pub fn reset(&mut self) {
        *self = Self::new(self.limit);
    }

    fn bump(&mut self) {
        self.revision = self.next_revision;
        self.next_revision += 1;
    }

    /// Record an edit that has just been made. `before` is the state from
    /// before it. Consecutive edits with the same `merge_key` within
    /// [`MERGE_WINDOW_S`] of each other become one step; so do all edits inside
    /// a group (see [`History::begin_group`]).
    pub fn record(&mut self, label: &str, merge_key: Option<&str>, now_s: f64, before: EditState) {
        let merge = match (&self.group, merge_key, &self.last) {
            (Some((_, recorded)), _, _) => *recorded,
            (None, Some(k), Some((last_key, t))) => {
                last_key == k && now_s - t <= MERGE_WINDOW_S && self.redo.is_empty()
            }
            _ => false,
        };
        if !merge {
            let label = match &self.group {
                Some((group_label, _)) => group_label.clone(),
                None => label.to_string(),
            };
            self.undo.push(Step {
                label,
                state: before,
                revision: self.revision,
            });
            if self.undo.len() > self.limit {
                self.undo.remove(0);
            }
            if let Some(g) = &mut self.group {
                g.1 = true;
            }
        }
        self.redo.clear();
        self.last = merge_key.map(|k| (k.to_string(), now_s));
        self.bump();
    }

    /// A change that is saved with the project but not undoable (build settings).
    pub fn touch(&mut self) {
        self.bump();
    }

    /// Start grouping edits into one undo step called `label` (e.g. deleting
    /// several nodes). Nested calls are ignored.
    pub fn begin_group(&mut self, label: &str) {
        if self.group.is_none() {
            self.group = Some((label.to_string(), false));
        }
    }

    pub fn end_group(&mut self) {
        self.group = None;
        self.last = None;
    }

    /// Stop the next edit from merging with the previous one (e.g. when a
    /// slider drag ends).
    pub fn break_merge(&mut self) {
        self.last = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What Undo would undo, e.g. "Set Height".
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Undo the last step: returns the state to restore and the step's label.
    /// `current` is the state now (kept for redo).
    pub fn undo(&mut self, current: EditState) -> Option<(EditState, String)> {
        let step = self.undo.pop()?;
        self.redo.push(Step {
            label: step.label.clone(),
            state: current,
            revision: self.revision,
        });
        self.revision = step.revision;
        self.last = None;
        Some((step.state, step.label))
    }

    pub fn redo(&mut self, current: EditState) -> Option<(EditState, String)> {
        let step = self.redo.pop()?;
        self.undo.push(Step {
            label: step.label.clone(),
            state: current,
            revision: self.revision,
        });
        self.revision = step.revision;
        self.last = None;
        Some((step.state, step.label))
    }

    /// The current state has been saved.
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    /// Mark the current state as unsaved even though nothing was recorded.
    pub fn mark_unsaved(&mut self) {
        self.saved_revision = u64::MAX;
    }

    /// True if the state differs from the last save (undoing back to the
    /// saved state makes this false again).
    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(seed: u64) -> EditState {
        EditState {
            world: World {
                seed,
                ..World::default()
            },
            graph: Graph::new(),
            exports: Vec::new(),
        }
    }

    /// Apply an edit (change the seed) and record it.
    fn edit(h: &mut History, cur: &mut EditState, seed: u64, key: Option<&str>, t: f64) {
        let before = cur.clone();
        cur.world.seed = seed;
        h.record("Set seed", key, t, before);
    }

    #[test]
    fn undo_redo_150_steps() {
        let mut h = History::default();
        let mut cur = state(0);
        for i in 1..=150 {
            edit(&mut h, &mut cur, i, None, i as f64);
        }
        assert!(h.is_modified());
        for i in (0..150).rev() {
            let (s, label) = h.undo(cur.clone()).unwrap();
            assert_eq!(label, "Set seed");
            cur = s;
            assert_eq!(cur.world.seed, i);
        }
        assert!(!h.can_undo());
        assert!(!h.is_modified(), "back at the saved state");
        for i in 1..=150 {
            cur = h.redo(cur.clone()).unwrap().0;
            assert_eq!(cur.world.seed, i);
        }
        assert!(!h.can_redo());
    }

    #[test]
    fn merges_rapid_edits_of_the_same_thing() {
        let mut h = History::default();
        let mut cur = state(0);
        edit(&mut h, &mut cur, 1, Some("n1.height"), 0.0);
        edit(&mut h, &mut cur, 2, Some("n1.height"), 0.3);
        edit(&mut h, &mut cur, 3, Some("n1.height"), 0.6);
        assert_eq!(h.undo_len(), 1);
        edit(&mut h, &mut cur, 4, Some("n1.height"), 5.0); // too late: new step
        edit(&mut h, &mut cur, 5, Some("n1.other"), 5.1); // different key
        assert_eq!(h.undo_len(), 3);
        h.break_merge();
        edit(&mut h, &mut cur, 6, Some("n1.other"), 5.2);
        assert_eq!(h.undo_len(), 4);
        let s = h.undo(cur.clone()).unwrap().0;
        assert_eq!(s.world.seed, 5);
    }

    #[test]
    fn groups_make_one_step_and_redo_clears() {
        let mut h = History::default();
        let mut cur = state(0);
        h.begin_group("Delete nodes");
        edit(&mut h, &mut cur, 1, None, 0.0);
        edit(&mut h, &mut cur, 2, None, 0.0);
        h.end_group();
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo_label(), Some("Delete nodes"));
        cur = h.undo(cur.clone()).unwrap().0;
        assert_eq!(cur.world.seed, 0);
        assert!(h.can_redo());
        edit(&mut h, &mut cur, 9, None, 1.0);
        assert!(!h.can_redo(), "a new edit clears redo");
    }

    #[test]
    fn saved_state_tracking() {
        let mut h = History::default();
        let mut cur = state(0);
        edit(&mut h, &mut cur, 1, None, 0.0);
        h.mark_saved();
        assert!(!h.is_modified());
        edit(&mut h, &mut cur, 2, None, 1.0);
        assert!(h.is_modified());
        cur = h.undo(cur.clone()).unwrap().0;
        assert!(!h.is_modified());
        cur = h.undo(cur.clone()).unwrap().0;
        assert!(h.is_modified());
        let _ = h.redo(cur).unwrap();
        assert!(!h.is_modified());
        h.touch();
        assert!(h.is_modified());
    }

    #[test]
    fn limit_drops_oldest() {
        let mut h = History::new(10);
        let mut cur = state(0);
        for i in 1..=25 {
            edit(&mut h, &mut cur, i, None, i as f64);
        }
        assert_eq!(h.undo_len(), 10);
        while let Some((s, _)) = h.undo(cur.clone()) {
            cur = s;
        }
        assert_eq!(cur.world.seed, 15);
    }
}
