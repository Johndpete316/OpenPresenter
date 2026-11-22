//! Scripture presentations from the ESV Bible API (Settings → Integrations).
//!
//! *New presentation* offers the look "Scripture (ESV)" when the integration is on:
//! the operator picks a book, chapter and verses (only references that exist, from
//! [`opcore::bible::BOOKS`]); the passage is fetched, then becomes a title slide with
//! the reference and slides of verses, each with the full reference small in the
//! bottom right. Slides hold whole verses while they fit at the theme's text size;
//! a verse too long for one slide is split at sentence breaks.
//!
//! With a theme, the slides stay linked to it: re-styling from the theme keeps the
//! reference box's size and place (see `doc::Theme`'s handling of extra text boxes).

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, Sender};
use doc::Element;
use egui::{Color32, RichText};
use opcore::bible::{BOOKS, Passage, VerseRef, fits_words, slides};

use super::lyrics::{PLAIN_CANVAS, new_file, new_presentation, plain_theme, save_new};
use crate::ui::{UiAction, UiState};

/// Without fonts to measure with: about what fits at Bookman Old Style 54 on 1024×768.
const WORDS_PER_SLIDE: usize = 55;
/// The reference's text size, as a share of the main text's.
const REFERENCE_SCALE: f64 = 0.5;

enum Msg {
    Token(Result<Option<String>, String>),
    TokenSaved(Result<Option<String>, String>),
    Tested(Result<String, String>),
    Fetched(Result<Vec<String>, String>),
}

/// What the integration needs between frames: the token (once read from the
/// credential store), the New presentation pickers and a fetch in progress.
pub struct Scripture {
    /// Settings → Integrations, copied each frame.
    pub enabled: bool,
    pub theme: Option<String>,
    /// `None` = not read yet.
    token: Option<Option<String>>,
    token_error: Option<String>,
    busy_token: bool,
    token_input: String,
    /// Settings: the last Test or Save result.
    status: Option<Result<String, String>>,
    testing: bool,
    /// New presentation: the look "Scripture" is chosen.
    pub on: bool,
    book: usize,
    from: VerseRef,
    to: VerseRef,
    /// The name last filled in from the reference (replaced while the operator
    /// hasn't typed their own).
    auto_name: String,
    pending: Option<Pending>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

/// A passage being fetched, and where it goes.
struct Pending {
    passage: Passage,
    dir: PathBuf,
    base: doc::Document,
}

impl Default for Scripture {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Scripture {
            enabled: false,
            theme: None,
            token: None,
            token_error: None,
            busy_token: false,
            token_input: String::new(),
            status: None,
            testing: false,
            on: false,
            book: BOOKS.iter().position(|b| b.name == "John").unwrap_or(0),
            from: VerseRef::new(1, 1),
            to: VerseRef::new(1, 1),
            auto_name: String::new(),
            pending: None,
            tx,
            rx,
        }
    }
}

impl Scripture {
    fn passage(&self) -> Result<Passage, String> {
        Passage::new(self.book, self.from, self.to).map_err(|e| e.to_string())
    }
}

fn spawn(st: &Scripture, ctx: &egui::Context, f: impl FnOnce() -> Msg + Send + 'static) -> bool {
    let tx = st.tx.clone();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("esv".into())
        .spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        })
        .is_ok()
}

/// Read the token from the credential store once (on a worker thread).
fn ensure_token(sc: &mut Scripture, ctx: &egui::Context) {
    if sc.token.is_none() && !sc.busy_token && sc.token_error.is_none() {
        sc.busy_token = spawn(sc, ctx, || Msg::Token(crate::esv::load_token()));
    }
}

/// Handle finished background work; each frame.
pub fn poll(st: &mut UiState, ctx: &egui::Context) {
    while let Ok(m) = st.edit.scripture.rx.try_recv() {
        let sc = &mut st.edit.scripture;
        match m {
            Msg::Token(r) => {
                sc.busy_token = false;
                match r {
                    Ok(t) => sc.token = Some(t),
                    Err(e) => sc.token_error = Some(e),
                }
            }
            Msg::TokenSaved(r) => {
                sc.busy_token = false;
                match r {
                    Ok(t) => {
                        sc.status = Some(Ok(if t.is_some() {
                            "Token saved.".into()
                        } else {
                            "Token removed.".into()
                        }));
                        sc.token = Some(t);
                        sc.token_error = None;
                        sc.token_input.clear();
                    }
                    Err(e) => sc.status = Some(Err(e)),
                }
            }
            Msg::Tested(r) => {
                sc.testing = false;
                sc.status = Some(r);
            }
            Msg::Fetched(r) => {
                let Some(p) = sc.pending.take() else { continue };
                match r.and_then(|verses| build(st, &p, &verses)) {
                    Ok(n) => {
                        st.edit.scripture.on = false;
                        super::lyrics::close_blank(st);
                        save_new(st, ctx, p.dir, n, true);
                    }
                    Err(e) => st.toast(e),
                }
            }
        }
    }
}

/// The look's template and theme for scripture slides (the theme chosen in
/// Settings → Integrations, or the plain look).
fn base(st: &UiState, file: &Path) -> Result<doc::Document, String> {
    let theme = st
        .edit
        .scripture
        .theme
        .as_ref()
        .and_then(|u| st.edit.themes.find(u));
    let (template, theme) = match theme {
        Some(t) => (t.to_document(), Some(Box::new(t.for_document(file)))),
        None => (plain_theme(st, PLAIN_CANVAS).to_document(), None),
    };
    new_presentation(&template, file, "", Vec::new(), theme)
}

/// The slides' parts: a slide in the look, its main text box, and the frames of the
/// verse text and the reference under it.
struct Parts {
    slide: doc::Slide,
    main: usize,
    text_frame: doc::Rect,
    reference_frame: doc::Rect,
    reference_size: f64,
}

fn parts(base: &doc::Document) -> Result<Parts, String> {
    let slide = base
        .groups
        .first()
        .and_then(|g| g.slides.first())
        .cloned()
        .ok_or("The look has no slide.")?;
    let (main, t) = slide
        .elements
        .iter()
        .enumerate()
        .find_map(|(i, e)| match e {
            Element::Text(t) if t.problem.is_none() => Some((i, t)),
            _ => None,
        })
        .ok_or("The look has no text box.")?;
    let size = t.runs().next().map_or(40.0, |r| r.size);
    let reference_size = (size * REFERENCE_SCALE).round().max(10.0);
    let ref_h = (reference_size * 1.5).round();
    let gap = (reference_size * 0.5).round();
    let f = t.frame;
    Ok(Parts {
        main,
        text_frame: doc::Rect {
            height: (f.height - ref_h - gap).max(size),
            ..f
        },
        reference_frame: doc::Rect {
            y: f.y + f.height - ref_h,
            height: ref_h,
            ..f
        },
        reference_size,
        slide,
    })
}

fn set_text(t: &mut doc::TextBox, text: &str) {
    t.paragraphs = doc::rich::replace_plain(&t.paragraphs, &t.plain_text(), text);
}

fn new_slide(p: &Parts) -> doc::Slide {
    let mut s = p.slide.clone();
    s.uuid = doc::new_uuid();
    for e in &mut s.elements {
        e.set_uuid(doc::new_uuid());
    }
    s
}

fn main_box<'a>(s: &'a mut doc::Slide, p: &Parts) -> Option<&'a mut doc::TextBox> {
    match s.elements.get_mut(p.main) {
        Some(Element::Text(t)) => Some(t),
        _ => None,
    }
}

fn title_slide(p: &Parts, label: &str) -> doc::Slide {
    let mut s = new_slide(p);
    if let Some(t) = main_box(&mut s, p) {
        set_text(t, label);
    }
    s
}

fn verse_slide(p: &Parts, label: &str, text: &str) -> doc::Slide {
    let mut s = new_slide(p);
    let Some(t) = main_box(&mut s, p) else {
        return s;
    };
    t.frame = p.text_frame;
    set_text(t, text);
    let mut r = Box::new(t.clone());
    r.uuid = doc::new_uuid();
    r.name = "Reference".into();
    r.frame = p.reference_frame;
    r.valign = doc::VAlign::Bottom;
    set_text(&mut r, label);
    for para in &mut r.paragraphs {
        para.align = doc::Align::Right;
        para.line_spacing = 1.0;
    }
    doc::rich::restyle_all(&mut r.paragraphs, |run| run.size = p.reference_size);
    s.elements.insert(p.main + 1, Element::Text(r));
    s
}

/// Split `verses` into slide texts that fit the verse box (measured with the real
/// fonts when they're loaded).
fn split(
    fonts: Option<&mut render::Fonts>,
    base: &doc::Document,
    p: &Parts,
    verses: &[String],
) -> Vec<String> {
    let Some(fonts) = fonts else {
        return slides(verses, fits_words(WORDS_PER_SLIDE));
    };
    let mut probe = new_slide(p);
    let uuid = probe.uuid.clone();
    let Some(t) = main_box(&mut probe, p) else {
        return slides(verses, fits_words(WORDS_PER_SLIDE));
    };
    t.frame = p.text_frame;
    // A little room to spare, so a font that wraps differently still fits.
    let limit = (p.text_frame.height * 0.96) as f32;
    let mut words = fits_words(WORDS_PER_SLIDE);
    slides(verses, |text| {
        set_text(t, text);
        match render::layout_box(fonts, &base.path, &uuid, t, 1.0) {
            Ok(l) => l.text_height() <= limit,
            Err(_) => words(text),
        }
    })
}

/// The scripture presentation: a title slide, then the verse slides.
fn build(st: &mut UiState, pending: &Pending, verses: &[String]) -> Result<doc::Document, String> {
    let label = pending.passage.label();
    let mut n = pending.base.clone();
    let p = parts(&n)?;
    let texts = split(st.fonts.as_mut(), &n, &p, verses);
    n.groups = vec![
        doc::Group {
            uuid: doc::new_uuid(),
            name: "Title".into(),
            color: None,
            slides: vec![title_slide(&p, &label)],
            src: None,
        },
        doc::Group {
            uuid: doc::new_uuid(),
            name: label.clone(),
            color: None,
            slides: texts.iter().map(|t| verse_slide(&p, &label, t)).collect(),
            src: None,
        },
    ];
    Ok(n)
}

/// Start fetching the chosen passage into a new presentation called `name`.
pub fn create(st: &mut UiState, ctx: &egui::Context, name: &str) {
    let sc = &st.edit.scripture;
    if sc.pending.is_some() {
        return;
    }
    let passage = match sc.passage() {
        Ok(p) => p,
        Err(e) => return st.toast(e),
    };
    let Some(Some(token)) = sc.token.clone() else {
        return st.toast("Add an ESV API token in Settings → Integrations first.");
    };
    let Some((dir, file)) = new_file(st, name) else {
        return;
    };
    let base = match base(st, &file) {
        Ok(b) => b,
        Err(e) => return st.toast(e),
    };
    let sc = &mut st.edit.scripture;
    if spawn(sc, ctx, move || {
        Msg::Fetched(crate::esv::fetch(&token, &passage))
    }) {
        sc.pending = Some(Pending { passage, dir, base });
    } else {
        st.toast("Couldn't start fetching the passage.");
    }
}

/// Stop waiting for a fetch (the prompt was closed): its answer is dropped.
pub fn cancel(st: &mut UiState) {
    st.edit.scripture.pending = None;
}

fn number_combo(ui: &mut egui::Ui, id: &str, value: &mut u16, max: u16) -> bool {
    let before = *value;
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.to_string())
        .width(56.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            for n in 1..=max {
                ui.selectable_value(value, n, n.to_string());
            }
        });
    *value != before
}

/// The pickers under Look in the New presentation prompt. Fills in `name` from the
/// reference unless the operator typed their own. Returns whether Create can go ahead.
pub fn pickers(ui: &mut egui::Ui, st: &mut UiState, name: &mut String) -> bool {
    ensure_token(&mut st.edit.scripture, ui.ctx());
    let sc = &mut st.edit.scripture;
    let busy = sc.pending.is_some();
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            ui.label("Book");
            let before = sc.book;
            egui::ComboBox::from_id_salt("scripture-book")
                .selected_text(BOOKS[sc.book].name)
                .width(150.0)
                .height(360.0)
                .show_ui(ui, |ui| {
                    for (i, b) in BOOKS.iter().enumerate() {
                        if i == 39 {
                            ui.separator();
                        }
                        ui.selectable_value(&mut sc.book, i, b.name);
                    }
                });
            if sc.book != before {
                sc.from = VerseRef::new(1, 1);
                sc.to = sc.from;
            }
        });
        let book = &BOOKS[sc.book];
        ui.horizontal(|ui| {
            ui.label("From");
            let mut changed = false;
            if !book.single_chapter() {
                ui.label("chapter");
                if number_combo(
                    ui,
                    "scripture-from-ch",
                    &mut sc.from.chapter,
                    book.chapter_count(),
                ) {
                    sc.from.verse = 1;
                    changed = true;
                }
            }
            ui.label("verse");
            changed |= number_combo(
                ui,
                "scripture-from-v",
                &mut sc.from.verse,
                book.verses(sc.from.chapter),
            );
            if changed && sc.to < sc.from {
                sc.to = sc.from;
            }
            ui.add_space(12.0);
            ui.label("to");
            if !book.single_chapter() {
                ui.label("chapter");
                let mut ch = sc.to.chapter;
                let first = sc.from.chapter;
                egui::ComboBox::from_id_salt("scripture-to-ch")
                    .selected_text(ch.to_string())
                    .width(56.0)
                    .height(320.0)
                    .show_ui(ui, |ui| {
                        for n in first..=book.chapter_count() {
                            ui.selectable_value(&mut ch, n, n.to_string());
                        }
                    });
                if ch != sc.to.chapter {
                    sc.to = VerseRef::new(ch, if ch == first { sc.from.verse } else { 1 });
                }
            }
            ui.label("verse");
            let low = if sc.to.chapter == sc.from.chapter {
                sc.from.verse
            } else {
                1
            };
            let mut v = sc.to.verse;
            egui::ComboBox::from_id_salt("scripture-to-v")
                .selected_text(v.to_string())
                .width(56.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    for n in low..=book.verses(sc.to.chapter) {
                        ui.selectable_value(&mut v, n, n.to_string());
                    }
                });
            sc.to.verse = v;
            if ui
                .small_button("Whole chapter")
                .on_hover_text("From verse 1 to the last verse of the chapter")
                .clicked()
            {
                sc.from.verse = 1;
                sc.to = VerseRef::new(sc.from.chapter, book.verses(sc.from.chapter));
            }
        });
    });
    let passage = sc.passage();
    match &passage {
        Ok(p) => {
            let label = p.label();
            if name.trim().is_empty() || *name == sc.auto_name {
                name.clone_from(&label);
            }
            sc.auto_name = label;
            ui.label(
                RichText::new(format!(
                    "{} verse(s): a title slide with the reference, then the verses, with the reference in the bottom right of each.",
                    p.verse_count()
                ))
                .small()
                .weak(),
            );
        }
        Err(e) => {
            ui.label(RichText::new(e).color(Color32::YELLOW));
        }
    }
    let ready = match (&sc.token, &sc.token_error) {
        (_, Some(e)) => {
            ui.label(RichText::new(e).color(Color32::YELLOW));
            false
        }
        (None, _) => {
            ui.label(RichText::new("Reading the ESV API token…").small().weak());
            false
        }
        (Some(None), _) => {
            ui.label(
                RichText::new("Add an ESV API token in Settings → Integrations first.")
                    .color(Color32::YELLOW),
            );
            false
        }
        (Some(Some(_)), _) => true,
    };
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Fetching the passage from the ESV API…");
        });
    }
    ready && passage.is_ok() && !busy
}

/// Settings → Integrations.
pub fn settings(
    ui: &mut egui::Ui,
    st: &mut UiState,
    settings: &opcore::Settings,
    actions: &mut Vec<UiAction>,
) {
    let esv = &settings.integrations.esv;
    ui.heading("ESV Bible API");
    ui.label(
        RichText::new("Makes scripture presentations from a reference: choose Look \"Scripture (ESV)\" in New presentation. Needs an API token from api.esv.org (free for non-commercial use, within its usage limits).")
            .small()
            .weak(),
    );
    ui.hyperlink_to(
        "Get a token at api.esv.org",
        "https://api.esv.org/account/create-application/",
    );
    let mut on = esv.enabled;
    if ui.checkbox(&mut on, "Enabled").changed() {
        actions.push(UiAction::SetEsvEnabled(on));
    }
    if !esv.enabled {
        return;
    }
    if st.edit.themes.is_empty() {
        super::themes::refresh(st, ui.ctx());
    }
    let names = st.edit.themes.names();
    let sc = &mut st.edit.scripture;
    ensure_token(sc, ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("API token");
        match (&sc.token, &sc.token_error) {
            (_, Some(e)) => {
                ui.label(RichText::new(e).color(Color32::YELLOW));
            }
            (None, _) => {
                ui.spinner();
            }
            (Some(Some(t)), _) => {
                let tail: String = t
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                ui.label(format!("saved (…{tail})"));
                if ui
                    .add_enabled(!sc.testing, egui::Button::new("Test"))
                    .on_hover_text("Fetch John 3:16 to check the token works")
                    .clicked()
                {
                    let token = t.clone();
                    sc.testing = spawn(sc, ui.ctx(), move || Msg::Tested(test(&token)));
                }
                if sc.testing {
                    ui.spinner();
                }
                if ui
                    .add_enabled(!sc.busy_token, egui::Button::new("Remove"))
                    .clicked()
                {
                    sc.busy_token = spawn(sc, ui.ctx(), || {
                        Msg::TokenSaved(crate::esv::delete_token().map(|()| None))
                    });
                }
            }
            (Some(None), _) => {}
        }
    });
    if matches!(sc.token, Some(None)) || sc.token_error.is_some() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut sc.token_input)
                    .password(true)
                    .hint_text("Paste the token")
                    .desired_width(300.0),
            );
            let ok = !sc.token_input.trim().is_empty() && !sc.busy_token;
            if ui.add_enabled(ok, egui::Button::new("Save")).clicked() {
                let token = sc.token_input.trim().to_owned();
                sc.busy_token = spawn(sc, ui.ctx(), move || {
                    Msg::TokenSaved(crate::esv::save_token(&token).map(|()| Some(token)))
                });
                sc.token_error = None;
            }
        });
        ui.label(
            RichText::new("Kept in the system's password store (Credential Manager on Windows), not in settings.toml.")
                .small()
                .weak(),
        );
    }
    match &sc.status {
        Some(Ok(m)) => {
            ui.label(RichText::new(m).color(Color32::from_rgb(120, 210, 140)));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(Color32::from_rgb(255, 130, 130)));
        }
        None => {}
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Look");
        let current = match &esv.theme {
            None => "plain (white text on black)".to_owned(),
            Some(u) => names.iter().find(|(id, _)| id == u).map_or_else(
                || "a theme that's no longer there (plain)".to_owned(),
                |(_, n)| format!("theme {n}"),
            ),
        };
        egui::ComboBox::from_id_salt("esv-theme")
            .selected_text(current)
            .width(280.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(esv.theme.is_none(), "plain (white text on black)")
                    .clicked()
                {
                    actions.push(UiAction::SetEsvTheme(None));
                }
                for (id, n) in &names {
                    if ui
                        .selectable_label(esv.theme.as_ref() == Some(id), format!("theme {n}"))
                        .clicked()
                    {
                        actions.push(UiAction::SetEsvTheme(Some(id.clone())));
                    }
                }
            });
    });
    ui.label(
        RichText::new("Scripture slides use this look. The reference in the bottom right is half the text size.")
            .small()
            .weak(),
    );
}

fn test(token: &str) -> Result<String, String> {
    let john = BOOKS.iter().position(|b| b.name == "John").unwrap_or(42);
    let p = Passage::new(john, VerseRef::new(3, 16), VerseRef::new(3, 16))
        .map_err(|e| e.to_string())?;
    crate::esv::fetch(token, &p).map(|_| "The token works.".to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use doc::Theme;

    #[test]
    fn slides_have_the_reference_bottom_right() {
        let tpl = Theme::plain(doc::CanvasSize {
            width: 1024,
            height: 768,
        })
        .to_document();
        let file = PathBuf::from("/x/John 3.opres");
        let base = new_presentation(&tpl, &file, "", Vec::new(), None).unwrap();
        let p = parts(&base).unwrap();
        let title = title_slide(&p, "John 3:16–17");
        assert_eq!(
            title.text_boxes().count(),
            1,
            "the title slide has only the reference"
        );
        assert_eq!(
            title.text_boxes().next().unwrap().plain_text(),
            "John 3:16–17"
        );

        let s = verse_slide(&p, "John 3:16–17", "For God so loved the world");
        let boxes: Vec<&doc::TextBox> = s.text_boxes().collect();
        assert_eq!(boxes.len(), 2);
        let (main, r) = (boxes[0], boxes[1]);
        assert_eq!(main.plain_text(), "For God so loved the world");
        assert_eq!(r.plain_text(), "John 3:16–17");
        assert_eq!(r.paragraphs[0].align, doc::Align::Right);
        let main_size = main.runs().next().unwrap().size;
        assert_eq!(
            r.runs().next().unwrap().size,
            (main_size * REFERENCE_SCALE).round()
        );
        assert!(
            r.frame.y >= main.frame.y + main.frame.height,
            "under the verse text"
        );
        assert!(r.frame.y + r.frame.height <= 768.0);
        assert_ne!(s.uuid, title.uuid);
    }

    #[test]
    fn verses_are_split_to_fit_without_fonts() {
        let tpl = Theme::plain(PLAIN_CANVAS).to_document();
        let base = new_presentation(&tpl, Path::new("/x/P.opres"), "", Vec::new(), None).unwrap();
        let p = parts(&base).unwrap();
        let verses: Vec<String> = (0..10)
            .map(|_| "word ".repeat(20).trim().to_owned())
            .collect();
        let s = split(None, &base, &p, &verses);
        assert_eq!(
            s.len(),
            5,
            "two 20-word verses per slide (55 words at most)"
        );
    }
}
