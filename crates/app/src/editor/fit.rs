//! Fit Text (docs/EDITOR-PLAN.md E6): shrink text until it fits its box, on demand
//! only, for one box or every overflowing box in a group or the presentation. One
//! undo step either way.

use std::path::Path;

use doc::{Edit as Op, Element};

use super::text::send_checked;
use crate::ui::{UiAction, UiState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Box(usize, usize, usize),
    Group(usize),
    Presentation,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub fitted: usize,
    /// Still too big at the minimum size.
    pub too_big: usize,
    /// Couldn't be measured (missing font, damaged text).
    pub failed: usize,
    pub already: usize,
}

fn boxes(d: &doc::Document, scope: Scope) -> Vec<(usize, usize, usize)> {
    let slide_boxes = |g: usize, s: usize| -> Vec<(usize, usize, usize)> {
        d.groups[g].slides[s]
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Element::Text(t) if t.problem.is_none() && !t.hidden))
            .map(|(e, _)| (g, s, e))
            .collect()
    };
    match scope {
        Scope::Box(g, s, e) => vec![(g, s, e)],
        Scope::Group(g) => (0..d.groups.get(g).map_or(0, |gr| gr.slides.len()))
            .flat_map(|s| slide_boxes(g, s))
            .collect(),
        Scope::Presentation => (0..d.groups.len())
            .flat_map(|g| (0..d.groups[g].slides.len()).map(move |s| (g, s)))
            .flat_map(|(g, s)| slide_boxes(g, s))
            .collect(),
    }
}

/// The edit fitting every box in `scope`, and what happened.
pub fn plan(
    fonts: &mut render::Fonts,
    d: &doc::Document,
    scope: Scope,
    min_size: f64,
) -> (Option<Op>, Outcome) {
    let mut out = Outcome::default();
    let mut edits = Vec::new();
    for (g, s, e) in boxes(d, scope) {
        let slide = &d.groups[g].slides[s];
        let Some(Element::Text(t)) = slide.elements.get(e) else {
            continue;
        };
        let set = |paragraphs| Op::SetRichText {
            group: g,
            slide: s,
            element: e,
            paragraphs,
        };
        match render::fit_text(fonts, &d.path, &slide.uuid, t, min_size) {
            Ok(render::Fit::AlreadyFits) => out.already += 1,
            Ok(render::Fit::Fitted { paragraphs, .. }) => {
                out.fitted += 1;
                edits.push(set(paragraphs));
            }
            Ok(render::Fit::TooBig { paragraphs, .. }) => {
                out.too_big += 1;
                if let Some(p) = paragraphs {
                    edits.push(set(p));
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "fit text: couldn't measure");
                out.failed += 1;
            }
        }
    }
    let op = match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Op::Batch {
            label: "Fit text".into(),
            edits,
        }),
    };
    (op, out)
}

pub fn run(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    scope: Scope,
) {
    let min = st.edit.fit_min_size;
    let Some(fonts) = st.fonts.as_mut() else {
        st.toast("Fonts are still loading; try Fit Text again in a moment.");
        return;
    };
    let (op, out) = plan(fonts, d, scope, min);
    if let Some(op) = op {
        send_checked(st, actions, path, d, op, None);
    }
    let single = matches!(scope, Scope::Box(..));
    if out.too_big > 0 {
        st.toast(if single {
            format!(
                "This text doesn't fit even at the minimum size ({min:.0} px). Shorten it or make the box bigger. (The minimum is in Settings → General.)"
            )
        } else {
            format!(
                "{} text box(es) don't fit even at the minimum size ({min:.0} px). Shorten their text or make the boxes bigger.",
                out.too_big
            )
        });
    }
    if out.failed > 0 {
        st.toast(format!(
            "{} text box(es) couldn't be measured (a font isn't installed, or the text is damaged), so they weren't changed.",
            out.failed
        ));
    }
    if single && out.already > 0 {
        st.toast("This text already fits its box.");
    } else if !single && out.fitted == 0 && out.too_big == 0 && out.failed == 0 {
        st.toast("Every text box there already fits.");
    }
}

/// Fit Text and its ▾ menu (group / presentation), in the text toolbar.
pub fn buttons(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s, e): (usize, usize, usize),
) {
    let mut scope = None;
    if ui
        .button("Fit Text")
        .on_hover_text("Shrink this box's text until it fits (sizes stay in proportion)")
        .clicked()
    {
        scope = Some(Scope::Box(g, s, e));
    }
    ui.menu_button("▾", |ui| {
        if ui
            .button("Fit every overflowing box in this group")
            .clicked()
        {
            scope = Some(Scope::Group(g));
            ui.close();
        }
        if ui
            .button("Fit every overflowing box in this presentation")
            .clicked()
        {
            scope = Some(Scope::Presentation);
            ui.close();
        }
    });
    if let Some(sc) = scope {
        super::text::commit(st, actions, path, d);
        run(st, actions, path, d, sc);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn overflowing(d: &doc::Document, fonts: &mut render::Fonts) -> usize {
        d.slides()
            .filter(|s| {
                render::render_slide(fonts, d, s, 1024, 768)
                    .map(|f| f.warnings.iter().any(|w| w.overflow))
                    .unwrap_or(false)
            })
            .count()
    }

    /// E6 acceptance: the badges clear after Fit Text; one undo reverts it.
    #[test]
    fn fitting_a_presentation_clears_every_overflow_in_one_step() {
        let mut fonts = render::Fonts::from_dirs(&[&fixtures().join("fonts")]);
        let mut d = doc::load(&fixtures().join("render_1024x768.pro6")).unwrap();
        // Make the first three slides' boxes overflow.
        for slide in d.groups[0].slides.iter_mut().take(3) {
            for e in &mut slide.elements {
                let base = match e {
                    Element::Text(t) => t.runs().next().cloned(),
                    _ => None,
                };
                if let Element::Text(t) = e
                    && let Some(base) = base
                {
                    t.paragraphs = (0..8)
                        .map(|i| doc::Paragraph {
                            align: doc::Align::Center,
                            line_spacing: 1.0,
                            runs: vec![doc::Run {
                                text: format!("A long line of words, number {i}"),
                                size: 64.0,
                                ..base.clone()
                            }],
                        })
                        .collect();
                }
            }
        }
        let before = d.clone();
        assert_eq!(overflowing(&d, &mut fonts), 3);
        let (op, out) = plan(&mut fonts, &d, Scope::Presentation, 12.0);
        let op = op.unwrap();
        assert!(matches!(op, Op::Batch { .. }));
        assert!(out.fitted >= 3, "{out:?}");
        let mut h = doc::History::default();
        h.record(op.label(), Arc::new(d.clone()));
        op.apply(&mut d).unwrap();
        assert_eq!(overflowing(&d, &mut fonts), 0, "badges clear");
        assert_eq!(*h.undo(Arc::new(d.clone())).unwrap(), before, "one undo");
        // Nothing left to do.
        let (op, out) = plan(&mut fonts, &d, Scope::Presentation, 12.0);
        assert!(op.is_none());
        assert_eq!(out.fitted, 0);
    }
}
