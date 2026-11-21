//! Undo / redo by snapshot (John, 2026-11-21): documents are immutable `Arc`s and
//! every edit makes a new one, so a step just keeps the document from before it.
//! Reverting is exact by construction, and a multi-slide action is one step.

use std::sync::Arc;

use crate::model::Document;

/// Steps kept per document.
pub const UNDO_STEPS: usize = 200;

#[derive(Debug, Clone)]
struct Step {
    label: String,
    doc: Arc<Document>,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// Record that `before` was replaced by an edit called `label`. Clears redo.
    pub fn record(&mut self, label: impl Into<String>, before: Arc<Document>) {
        self.undo.push(Step {
            label: label.into(),
            doc: before,
        });
        if self.undo.len() > UNDO_STEPS {
            let excess = self.undo.len() - UNDO_STEPS;
            self.undo.drain(..excess);
        }
        self.redo.clear();
    }

    /// Step back from `current`; returns the document to show.
    pub fn undo(&mut self, current: Arc<Document>) -> Option<Arc<Document>> {
        let step = self.undo.pop()?;
        self.redo.push(Step {
            label: step.label,
            doc: current,
        });
        Some(step.doc)
    }

    /// Step forward from `current`; returns the document to show.
    pub fn redo(&mut self, current: Arc<Document>) -> Option<Arc<Document>> {
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
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..UNDO_STEPS + 10 {
            h.record(format!("{i}"), doc("x"));
        }
        assert_eq!(h.undo_len(), UNDO_STEPS);
        assert_eq!(h.undo_label(), Some(&*format!("{}", UNDO_STEPS + 9)));
    }
}
