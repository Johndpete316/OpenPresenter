//! Copy Style / paste style (docs/EDITOR-PLAN.md E5): the toolbar buttons and
//! Ctrl+Alt+C / Ctrl+Alt+V. Pasting is one edit (one undo step) for any scope.

use std::path::Path;

use doc::{Scope, Style};
use egui::RichText;

use super::text::send_checked;
use crate::ui::{UiAction, UiState};

fn kind(s: &Style) -> &'static str {
    match s {
        Style::Text(_) => "text box",
        Style::Image(_) => "image",
    }
}

/// Copy the style of the primary selected element.
pub fn copy(st: &mut UiState, d: &doc::Document, (g, s): (usize, usize)) {
    let Some(e) = st.edit.element else { return };
    match d.groups[g].slides[s].elements.get(e).and_then(Style::of) {
        Some(style) => st.edit.style_clip = Some(style),
        None => st.toast("This element has no style to copy (its text can't be read)."),
    }
}

/// Paste the copied style onto `scope` as one step.
pub fn paste(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    scope: &Scope,
) {
    let Some(style) = st.edit.style_clip.clone() else {
        return;
    };
    match doc::style::paste(d, &style, scope) {
        Some((op, _)) => send_checked(st, actions, path, d, op, None),
        None if doc::style::targets(d, &style, scope).is_empty() => st.toast(format!(
            "There's no {} there to paste the style onto.",
            kind(&style)
        )),
        None => {}
    }
}

/// Copy Style and Paste Style ▾ in the contextual bar.
pub fn buttons(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    let sel = st.edit.selection();
    if sel.len() == 1
        && ui
            .button("Copy Style")
            .on_hover_text("Copy this element's look: font, size, colour, alignment, spacing, outline, shadow (images: adjustments, opacity). Ctrl+Alt+C")
            .clicked()
    {
        copy(st, d, (g, s));
    }
    let Some(style) = st.edit.style_clip.clone() else {
        return;
    };
    let mut scope = None;
    ui.menu_button("Paste Style ▾", |ui| {
        ui.label(RichText::new(format!("Copied: a {} style", kind(&style))).weak());
        let count = |sc: &Scope| doc::style::targets(d, &style, sc).len();
        let options = [
            (Scope::Elements(g, s, sel.clone()), "Selected (Ctrl+Alt+V)"),
            (Scope::Slide(g, s), "All on this slide"),
            (Scope::Group(g), "All slides in this group"),
            (Scope::Presentation, "All slides in this presentation"),
        ];
        for (sc, label) in options {
            let n = count(&sc);
            if ui
                .add_enabled(n > 0, egui::Button::new(format!("{label} — {n}")))
                .clicked()
            {
                scope = Some(sc);
                ui.close();
            }
        }
    });
    if let Some(sc) = scope {
        paste(st, actions, path, d, &sc);
    }
}
