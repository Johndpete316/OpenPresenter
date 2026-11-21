//! Undo / redo by snapshot (John, 2026-11-21): documents are immutable `Arc`s and
//! every edit makes a new one, so a step just keeps the document from before it.
//! Reverting is exact by construction, and a multi-slide action is one step.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::model::Document;

/// Steps kept per document.
pub const UNDO_STEPS: usize = 200;
/// Edits with the same merge key this close together are one undo step.
pub const MERGE_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct Step {
    label: String,
    doc: Arc<Document>,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Merge key and time of the last merging edit.
    last_merge: Option<(String, Instant)>,
}

impl History {
    /// Record that `before` was replaced by an edit called `label`. Clears redo.
    pub fn record(&mut self, label: impl Into<String>, before: Arc<Document>) {
        self.last_merge = None;
        self.push(label.into(), before);
    }

    /// Like [`History::record`], but an edit with the same `key` as the previous one,
    /// within [`MERGE_WINDOW`] of it, joins that step (arrow-key nudges, a colour
    /// being dragged in a picker).
    pub fn record_merging(
        &mut self,
        label: impl Into<String>,
        before: Arc<Document>,
        key: &str,
        now: Instant,
    ) {
        let merge =
            self.last_merge.as_ref().is_some_and(|(k, at)| {
                k == key && now.saturating_duration_since(*at) <= MERGE_WINDOW
            }) && !self.undo.is_empty();
        if merge {
            self.redo.clear();
        } else {
            self.push(label.into(), before);
        }
        self.last_merge = Some((key.to_owned(), now));
    }

    fn push(&mut self, label: String, before: Arc<Document>) {
        self.undo.push(Step { label, doc: before });
        if self.undo.len() > UNDO_STEPS {
            let excess = self.undo.len() - UNDO_STEPS;
            self.undo.drain(..excess);
        }
        self.redo.clear();
    }

    /// Step back from `current`; returns the document to show.
    pub fn undo(&mut self, current: Arc<Document>) -> Option<Arc<Document>> {
        self.last_merge = None;
        let step = self.undo.pop()?;
        self.redo.push(Step {
            label: step.label,
            doc: current,
        });
        Some(step.doc)
    }

    /// Step forward from `current`; returns the document to show.
    pub fn redo(&mut self, current: Arc<Document>) -> Option<Arc<Document>> {
        self.last_merge = None;
        let step = self.redo.pop()?;
        self.undo.push(Step {
            label: step.label,
            doc: current,
        });
        Some(step.doc)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::Mode;
    use crate::model::CanvasSize;

    fn doc(name: &str) -> Arc<Document> {
        Arc::new(Document {
            path: name.into(),
            format_version: crate::FORMAT_VERSION,
            mode: Mode::Pro6,
            canvas: CanvasSize {
                width: 1,
                height: 1,
            },
            background: None,
            selected_arrangement: None,
            groups: Vec::new(),
            arrangements: Vec::new(),
            cue_report: Default::default(),
            theme: None,
            pro6: None,
        })
    }

    #[test]
    fn undo_redo_walks_snapshots() {
        let (a, b, c) = (doc("a"), doc("b"), doc("c"));
        let mut h = History::default();
        assert!(h.undo(a.clone()).is_none());
        h.record("first", a.clone());
        h.record("second", b.clone());
        assert_eq!(h.undo_label(), Some("second"));
        let back = h.undo(c.clone()).unwrap();
        assert!(Arc::ptr_eq(&back, &b));
        assert_eq!(h.redo_label(), Some("second"));
        let back = h.undo(back).unwrap();
        assert!(Arc::ptr_eq(&back, &a));
        assert!(h.undo(back.clone()).is_none(), "nothing left to undo");
        let fwd = h.redo(back).unwrap();
        assert!(Arc::ptr_eq(&fwd, &b));
        let fwd = h.redo(fwd).unwrap();
        assert!(Arc::ptr_eq(&fwd, &c));
        assert!(h.redo(fwd.clone()).is_none());
        // A new edit after undo clears redo.
        let back = h.undo(fwd).unwrap();
        h.record("third", back);
        assert!(h.redo_label().is_none());
        assert_eq!(h.undo_label(), Some("third"));
    }

    #[test]
    fn merging_edits_join_one_step() {
        let t0 = Instant::now();
        let mut h = History::default();
        let first = doc("before nudges");
        h.record_merging("Move", first.clone(), "nudge:1", t0);
        h.record_merging("Move", doc("x"), "nudge:1", t0 + Duration::from_millis(500));
        h.record_merging("Move", doc("y"), "nudge:1", t0 + Duration::from_millis(900));
        assert_eq!(h.undo_len(), 1);
        assert!(Arc::ptr_eq(&h.undo(doc("now")).unwrap(), &first));
        // Another key, or a pause, starts a new step.
        let mut h = History::default();
        h.record_merging("Move", doc("a"), "nudge:1", t0);
        h.record_merging("Move", doc("b"), "nudge:2", t0);
        h.record_merging("Move", doc("c"), "nudge:2", t0 + MERGE_WINDOW * 2);
        assert_eq!(h.undo_len(), 3);
        // A plain edit in between breaks the run.
        let mut h = History::default();
        h.record_merging("Move", doc("a"), "k", t0);
        h.record("Edit text", doc("b"));
        h.record_merging("Move", doc("c"), "k", t0);
        assert_eq!(h.undo_len(), 3);
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..UNDO_STEPS + 10 {
            h.record(format!("{i}"), doc("x"));
        }
        assert_eq!(h.undo_len(), UNDO_STEPS);
        assert_eq!(h.undo_label(), Some(&*format!("{}", UNDO_STEPS + 9)));
    }
}
