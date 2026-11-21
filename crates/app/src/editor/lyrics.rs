//! Paste Lyrics (docs/EDITOR-PLAN.md E8): a full-height text view of the
//! presentation as lyrics (`[Group]` lines, blank lines between slides) with a live
//! preview beside it. Apply replaces or appends to the presentation (one undo
//! step), or makes a new presentation in the library.
//!
//! The same window opens from Import/Export as *New song from text* ([`open_new`],
//! [`new_window`]): it always makes a new `.opres`, styled like the selected
//! presentation, a theme, or the plain built-in look.
//!
//! *New presentation* (the library's + button, [`open_blank`], [`blank_dialog`]) only asks
//! for a name and a look, then opens a blank presentation in the editor.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use doc::lyrics::{LyricsMode, parse, to_text};
use doc::{Edit as Op, Theme};
use egui::{Color32, RichText, Vec2};

use super::text::send_checked;
use crate::thumbs::ThumbRequest;
use crate::ui::{UiAction, UiState, color32, paint_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    Replace,
    Append,
    New,
}

/// Rebuild the preview this long after the last keystroke.
const PREVIEW_AFTER: f64 = 0.35;

#[derive(Default)]
pub struct Lyrics {
    pub open: bool,
    /// The New presentation name prompt is open.
    blank: bool,
    /// Opened from the toolbar (New presentation), not from the editor.
    standalone: bool,
    /// Standalone: style new slides like the selected presentation.
    like_selected: bool,
    text: String,
    target: Target,
    new_name: String,
    /// Theme for new slides (by UUID); `None` = the presentation's own style.
    theme: Option<String>,
    /// Text the preview was built from, and when the text last changed.
    preview_for: Option<String>,
    changed_at: f64,
    preview: Option<Result<Arc<doc::Document>, String>>,
}

/// A new presentation being written, and whether to open it in the editor.
pub struct Saving {
    rx: crossbeam_channel::Receiver<Result<PathBuf, String>>,
    edit: bool,
}

/// Canvas for the plain look when there's no presentation to take one from.
const PLAIN_CANVAS: doc::CanvasSize = doc::CanvasSize {
    width: 1920,
    height: 1080,
};

/// Fills the text box when the operator asks for an example.
const EXAMPLE: &str = "[Verse 1]
Amazing grace how sweet the sound
That saved a wretch like me

I once was lost but now am found
Was blind but now I see

[Chorus]
My chains are gone
I've been set free

[Verse 2]
'Twas grace that taught my heart to fear
And grace my fears relieved

[Chorus]
";

/// Open with the presentation as text.
pub fn open(st: &mut UiState, ctx: &egui::Context, d: &doc::Document) {
    let l = &mut st.edit.lyrics;
    l.open = true;
    l.standalone = false;
    l.text = to_text(d);
    l.preview_for = None;
    l.preview = None;
    l.target = Target::Replace;
    l.new_name.clear();
    if st.edit.themes.is_empty() {
        super::themes::refresh(st, ctx);
    }
}

/// Open as *New presentation* (toolbar). `selected`: a presentation is selected, so
/// new slides can look like it (the default, so a new song matches the others).
pub fn open_new(st: &mut UiState, ctx: &egui::Context, selected: bool) {
    let l = &mut st.edit.lyrics;
    l.open = true;
    l.standalone = true;
    l.like_selected = selected;
    l.text.clear();
    l.preview_for = None;
    l.preview = None;
    l.target = Target::New;
    l.theme = None;
    l.new_name.clear();
    if st.edit.themes.is_empty() {
        super::themes::refresh(st, ctx);
    }
}

/// Like [`open_new`], with a name and text already filled in (Import, song words from
/// a text file).
pub fn open_new_with(
    st: &mut UiState,
    ctx: &egui::Context,
    selected: bool,
    name: &str,
    text: &str,
) {
    open_new(st, ctx, selected);
    let l = &mut st.edit.lyrics;
    l.new_name = name.trim().to_owned();
    l.text = text.replace("\r\n", "\n").replace('\r', "\n");
}

fn colors(st: &UiState) -> Vec<(String, doc::Rgba)> {
    st.edit
        .group_colors
        .iter()
        .map(|g| {
            let f = |v: u8| f64::from(v) / 255.0;
            (
                g.name.clone(),
                doc::Rgba {
                    r: f(g.color[0]),
                    g: f(g.color[1]),
                    b: f(g.color[2]),
                    a: 1.0,
                },
            )
        })
        .collect()
}

fn chosen_theme(st: &UiState, doc_path: &Path) -> Option<Box<Theme>> {
    let uuid = st.edit.lyrics.theme.as_ref()?;
    st.edit
        .themes
        .find(uuid)
        .map(|t| Box::new(t.for_document(doc_path)))
}

/// A new presentation: one slide styled like `d`'s first text slide (or the
/// theme), then the lyrics.
/// With no text: one empty slide in the template's style (a blank presentation).
fn new_presentation(
    d: &doc::Document,
    path: &Path,
    text: &str,
    colors: Vec<(String, doc::Rgba)>,
    theme: Option<Box<Theme>>,
) -> Result<doc::Document, String> {
    let template = d
        .slides()
        .find(|s| {
            s.text_boxes()
                .any(|t| t.problem.is_none() && t.runs().next().is_some())
        })
        .cloned()
        .ok_or("This presentation has no text slide to take the style from.")?;
    let mut slide = template;
    slide.src = None;
    slide.pro6_incompatible = false;
    slide.theme = None;
    for e in &mut slide.elements {
        e.set_uuid(doc::new_uuid());
        match e {
            doc::Element::Text(t) => t.src = None,
            doc::Element::Image(i) => i.src = None,
            doc::Element::Pro6(_) => {}
        }
    }
    // Raw .pro6 elements can't live in a new native file.
    slide
        .elements
        .retain(|e| !matches!(e, doc::Element::Pro6(_)));
    let mut n = doc::Document {
        path: path.to_path_buf(),
        format_version: doc::FORMAT_VERSION,
        mode: doc::Mode::Native,
        canvas: theme.as_ref().map_or(d.canvas, |t| t.canvas),
        background: d.background,
        selected_arrangement: None,
        groups: vec![doc::Group {
            uuid: doc::new_uuid(),
            name: "Template".into(),
            color: None,
            slides: vec![slide],
            src: None,
        }],
        arrangements: Vec::new(),
        cue_report: doc::CueReport::default(),
        theme: None,
        pro6: None,
    };
    if d.canvas != n.canvas {
        // Scale the copied slide to the theme's canvas.
        let mut tmp = n.clone();
        tmp.canvas = d.canvas;
        doc::theme::rescale(&mut tmp, n.canvas);
        n = tmp;
    }
    if text.trim().is_empty() {
        n.groups[0].name = "Slides".into();
        n.groups[0].src = None;
        // Empty, but keeping one empty run so the box remembers its font for typing.
        for e in &mut n.groups[0].slides[0].elements {
            if let doc::Element::Text(t) = e
                && t.problem.is_none()
            {
                let old = t.plain_text();
                t.paragraphs = doc::rich::replace_plain(&t.paragraphs, &old, "");
                t.paragraphs.truncate(1);
            }
        }
        if let Some(t) = theme {
            Op::ApplyTheme {
                theme: t,
                slides: Vec::new(),
                all: true,
                link: true,
            }
            .apply(&mut n)
            .map_err(|e| e.user_message())?;
        }
        return Ok(n);
    }
    Op::SetLyrics {
        text: text.to_owned(),
        mode: LyricsMode::Replace,
        colors,
        theme,
    }
    .apply(&mut n)
    .map_err(|e| e.user_message())?;
    Ok(n)
}

fn build_preview(
    st: &UiState,
    path: &Path,
    d: &doc::Document,
) -> Result<Arc<doc::Document>, String> {
    let l = &st.edit.lyrics;
    let theme = chosen_theme(st, path);
    let colors = colors(st);
    match l.target {
        Target::New => new_presentation(d, path, &l.text, colors, theme).map(Arc::new),
        Target::Replace | Target::Append => {
            let mut p = d.clone();
            Op::SetLyrics {
                text: l.text.clone(),
                mode: if l.target == Target::Append {
                    LyricsMode::Append
                } else {
                    LyricsMode::Replace
                },
                colors,
                theme,
            }
            .apply(&mut p)
            .map_err(|e| e.user_message())?;
            Ok(Arc::new(p))
        }
    }
}

/// The Paste Lyrics window.
pub fn window(
    ctx: &egui::Context,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    if !st.edit.lyrics.open || st.edit.lyrics.standalone {
        return;
    }
    show(ctx, st, thumbs, actions, path, d, None);
}

/// The *New presentation* window (toolbar), over the operator view. `selected`: the
/// presentation selected there, whose look a new song can copy.
pub fn new_window(
    ctx: &egui::Context,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    selected: Option<(&Path, &doc::Document)>,
) {
    let l = &st.edit.lyrics;
    if !l.open || !l.standalone {
        return;
    }
    let selected = selected.filter(|(_, d)| d.theme.is_none());
    let (path, template) = template(st, selected);
    let title = selected.map(|(p, _)| crate::ui::title(p));
    show(ctx, st, thumbs, actions, &path, &template, Some(title));
}

/// What new slides copy: the selected presentation, the theme, or the plain look.
fn template<'a>(
    st: &mut UiState,
    selected: Option<(&'a Path, &'a doc::Document)>,
) -> (PathBuf, Cow<'a, doc::Document>) {
    if selected.is_none() && st.edit.lyrics.like_selected {
        st.edit.lyrics.like_selected = false;
        st.edit.lyrics.preview_for = None;
    }
    // "Like" the selected presentation means its look (canvas, background, the main text
    // box's place and style), as a theme would carry it, not a copy of its slide.
    let like = selected
        .filter(|_| st.edit.lyrics.like_selected)
        .and_then(|(p, d)| look_of(d).map(|t| (p, t)));
    if let Some((p, t)) = like {
        return (p.to_path_buf(), Cow::Owned(t.to_document()));
    }
    match st
        .edit
        .lyrics
        .theme
        .as_ref()
        .and_then(|u| st.edit.themes.find(u))
    {
        Some(t) => (t.path.clone(), Cow::Owned(t.to_document())),
        None => {
            let canvas = selected.map_or(PLAIN_CANVAS, |(_, d)| d.canvas);
            (
                PathBuf::from("new.opres"),
                Cow::Owned(Theme::plain(canvas).to_document()),
            )
        }
    }
}

/// The look of `d`'s first slide with styled text, as a theme.
fn look_of(d: &doc::Document) -> Option<Theme> {
    d.groups.iter().enumerate().find_map(|(g, gr)| {
        gr.slides.iter().enumerate().find_map(|(s, sl)| {
            sl.text_boxes()
                .any(|t| t.problem.is_none() && t.runs().next().is_some())
                .then(|| Theme::from_slide("Look", d, (g, s)).ok())
                .flatten()
        })
    })
}

/// Open the New presentation prompt (name and look; then a blank presentation in the
/// editor). `selected`: a presentation is selected, so it can look like it.
pub fn open_blank(st: &mut UiState, ctx: &egui::Context, selected: bool) {
    let l = &mut st.edit.lyrics;
    l.blank = true;
    l.open = false;
    l.like_selected = selected;
    l.theme = None;
    l.text.clear();
    l.target = Target::New;
    l.new_name.clear();
    if st.edit.themes.is_empty() {
        super::themes::refresh(st, ctx);
    }
}

/// The New presentation prompt.
pub fn blank_dialog(
    ctx: &egui::Context,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    selected: Option<(&Path, &doc::Document)>,
) {
    if !st.edit.lyrics.blank {
        return;
    }
    let selected = selected.filter(|(_, d)| d.theme.is_none());
    let title = selected.map(|(p, _)| crate::ui::title(p));
    let mut create = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::unique("new-presentation")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.label(RichText::new("New presentation").size(18.0).strong());
        ui.add_space(4.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.edit.lyrics.new_name)
                .hint_text("Name, e.g. the song title")
                .desired_width(f32::INFINITY),
        );
        if !r.has_focus() && st.edit.lyrics.new_name.is_empty() {
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(4.0);
        new_header(ui, st, title.as_deref());
        match &st.edit.new_dir {
            Some(dir) => ui.label(
                RichText::new(format!(
                    "Saved as {}.opres in {} (change it in Settings, Folders). It opens in the editor, ready for slides.",
                    file_stem(st.edit.lyrics.new_name.trim()),
                    dir.display()
                ))
                .small()
                .weak(),
            ),
            None => ui.label(
                RichText::new("Choose a folder for new presentations in Settings, Folders first.")
                    .color(Color32::YELLOW),
            ),
        };
        ui.add_space(6.0);
        let ready = !st.edit.lyrics.new_name.trim().is_empty() && st.edit.new_dir.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new(RichText::new("Create").strong()))
                .clicked()
                || (enter && ready)
            {
                create = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if cancel || modal.should_close() {
        st.edit.lyrics.blank = false;
        return;
    }
    if create {
        let (path, template) = template(st, selected);
        st.edit.lyrics.text.clear();
        st.edit.lyrics.target = Target::New;
        apply(st, ctx, actions, &path, &template);
        // A name clash keeps the prompt open (the toast says why).
        if st.edit.lyrics_new.is_some() {
            st.edit.lyrics.blank = false;
        }
    }
}

/// The window. `standalone`: `Some(title of the selected presentation, if any)`.
fn show(
    ctx: &egui::Context,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    standalone: Option<Option<String>>,
) {
    let now = ctx.input(|i| i.time);
    let screen = ctx.content_rect();
    let mut open = true;
    let heading = if standalone.is_some() {
        "New song from text"
    } else {
        "Paste Lyrics"
    };
    egui::Window::new(heading)
        .id(egui::Id::unique("paste-lyrics"))
        .open(&mut open)
        .collapsible(false)
        .fixed_pos(screen.min + Vec2::new(40.0, 30.0))
        .fixed_size(Vec2::new(screen.width() - 80.0, screen.height() - 110.0))
        .show(ctx, |ui| {
            body(ui, st, thumbs, actions, path, d, now, standalone.as_ref())
        });
    if !open {
        st.edit.lyrics.open = false;
    }
    // Live preview, a moment after typing stops.
    let l = &st.edit.lyrics;
    if l.preview_for.as_deref() != Some(&l.text) {
        if now - l.changed_at >= PREVIEW_AFTER {
            let p = build_preview(st, path, d);
            let l = &mut st.edit.lyrics;
            l.preview = Some(p);
            l.preview_for = Some(l.text.clone());
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(PREVIEW_AFTER));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn body(
    ui: &mut egui::Ui,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    now: f64,
    standalone: Option<&Option<String>>,
) {
    if let Some(selected) = standalone {
        new_header(ui, st, selected.as_deref());
    } else {
        lyrics_header(ui, st);
    }
    if st.edit.lyrics.target == Target::New {
        name_row(ui, st);
    }
    format_help(ui, st, standalone.is_some());
    body_main(ui, st, thumbs, actions, path, d, now);
}

/// Top row from the toolbar: what new slides look like.
fn new_header(ui: &mut egui::Ui, st: &mut UiState, selected: Option<&str>) {
    ui.horizontal(|ui| {
        let names: Vec<(String, String)> = st.edit.themes.names();
        let l = &mut st.edit.lyrics;
        let before = (l.like_selected, l.theme.clone());
        ui.label("Look");
        let current = if l.like_selected {
            format!("like {}", selected.unwrap_or("the selected presentation"))
        } else {
            l.theme
                .as_ref()
                .and_then(|u| names.iter().find(|(id, _)| id == u))
                .map_or("plain (white text on black)".to_owned(), |(_, n)| {
                    format!("theme {n}")
                })
        };
        egui::ComboBox::from_id_salt("new-look")
            .selected_text(current)
            .width(280.0)
            .show_ui(ui, |ui| {
                if let Some(t) = selected
                    && ui
                        .selectable_label(l.like_selected, format!("like {t}"))
                        .on_hover_text("Same font, size, layout and background as the presentation selected in the library")
                        .clicked()
                {
                    l.like_selected = true;
                    l.theme = None;
                }
                if ui
                    .selectable_label(!l.like_selected && l.theme.is_none(), "plain (white text on black)")
                    .clicked()
                {
                    l.like_selected = false;
                    l.theme = None;
                }
                for (id, n) in &names {
                    if ui
                        .selectable_label(
                            !l.like_selected && l.theme.as_ref() == Some(id),
                            format!("theme {n}"),
                        )
                        .clicked()
                    {
                        l.like_selected = false;
                        l.theme = Some(id.clone());
                    }
                }
            });
        if (l.like_selected, l.theme.clone()) != before {
            l.preview_for = None;
        }
    });
}

fn lyrics_header(ui: &mut egui::Ui, st: &mut UiState) {
    ui.horizontal(|ui| {
        let l = &mut st.edit.lyrics;
        let before = (l.target, l.theme.clone());
        ui.label("Put the lyrics in:");
        ui.selectable_value(&mut l.target, Target::Replace, "This presentation (replace)")
            .on_hover_text("The text becomes the presentation. Groups that keep their name keep their slides and look.");
        ui.selectable_value(&mut l.target, Target::Append, "This presentation (add at the end)");
        ui.selectable_value(&mut l.target, Target::New, "A new presentation");
        ui.separator();
        ui.label("New slides styled by");
        let names: Vec<(String, String)> = st
            .edit
            .themes
            .names();
        let l = &mut st.edit.lyrics;
        let current = l
            .theme
            .as_ref()
            .and_then(|u| names.iter().find(|(id, _)| id == u))
            .map_or("this presentation".to_owned(), |(_, n)| format!("theme {n}"));
        egui::ComboBox::from_id_salt("lyrics-theme")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut l.theme, None, "this presentation");
                for (id, n) in &names {
                    ui.selectable_value(&mut l.theme, Some(id.clone()), format!("theme {n}"));
                }
            });
        if (l.target, l.theme.clone()) != before {
            l.preview_for = None;
        }
    });
}

fn name_row(ui: &mut egui::Ui, st: &mut UiState) {
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.add(
            egui::TextEdit::singleline(&mut st.edit.lyrics.new_name)
                .hint_text("Song title")
                .desired_width(260.0),
        );
        match &st.edit.new_dir {
            Some(dir) => ui.label(
                RichText::new(format!(
                    "saved as {}.opres in {} (change it in Settings, Folders)",
                    file_stem(st.edit.lyrics.new_name.trim()),
                    dir.display()
                ))
                .small()
                .weak(),
            ),
            None => ui.label(
                RichText::new("Choose a folder for new presentations in Settings, Folders first.")
                    .color(Color32::YELLOW),
            ),
        };
    });
}

/// A file name for a presentation called `name`.
fn file_stem(name: &str) -> String {
    let n = name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    if n.is_empty() { "…".into() } else { n }
}

/// How to type a song: open by default from the toolbar, a line in the editor.
fn format_help(ui: &mut egui::Ui, st: &mut UiState, standalone: bool) {
    egui::CollapsingHeader::new(RichText::new("How to type or paste a song").strong())
        .id_salt("lyrics-help")
        .default_open(standalone)
        .show(ui, |ui| {
            let rows: [(&str, &str); 5] = [
                ("[Verse 1]", "A name in square brackets, on its own line, starts a group."),
                ("Amazing grace how sweet…", "The lines under it are the slide's lines."),
                ("(empty line)", "Starts the next slide in the same group."),
                ("[Chorus]  (again)", "A name used again repeats that group: no need to type it twice. The order of the names is the arrangement."),
                ("Verse 1:   Chorus   (Bridge)", "Headings as lyric websites write them work too."),
            ];
            egui::Grid::new("lyrics-help-grid")
                .num_columns(2)
                .spacing(Vec2::new(16.0, 4.0))
                .show(ui, |ui| {
                    for (code, what) in rows {
                        ui.label(
                            RichText::new(code)
                                .monospace()
                                .color(crate::theme::brand::ACCENT_START),
                        );
                        ui.label(what);
                        ui.end_row();
                    }
                });
            ui.label(
                RichText::new("Text before the first name goes in a group called \"Lyrics\". Group colours come from Settings, General. Leave the text empty for a blank presentation.")
                    .small()
                    .weak(),
            );
            let l = &mut st.edit.lyrics;
            if l.text.trim().is_empty() && ui.button("Show an example").clicked() {
                l.text = EXAMPLE.to_owned();
                l.changed_at = 0.0;
            }
        });
}

fn body_main(
    ui: &mut egui::Ui,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    now: f64,
) {
    let parsed = parse(&st.edit.lyrics.text);
    let avail = ui.available_size();
    ui.horizontal_top(|ui| {
        // Text, full height.
        ui.vertical(|ui| {
            ui.set_width(avail.x * 0.45);
            egui::ScrollArea::vertical()
                .id_salt("lyrics-text")
                .max_height(avail.y - 40.0)
                .show(ui, |ui| {
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut st.edit.lyrics.text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(((avail.y - 60.0) / 16.0).max(10.0) as usize),
                    );
                    if r.changed() {
                        st.edit.lyrics.changed_at = now;
                    }
                });
        });
        ui.separator();
        // Preview.
        ui.vertical(|ui| {
            ui.set_width(avail.x * 0.52);
            let order: Vec<&str> = parsed
                .order
                .iter()
                .map(|&g| parsed.groups[g].name.as_str())
                .collect();
            ui.label(format!(
                "{} group(s), {} slide(s). Order: {}",
                parsed.groups.len(),
                parsed.slide_count(),
                if order.is_empty() { "—".to_owned() } else { order.join(", ") }
            ));
            if parsed.ignored_lines > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} line(s) under a repeated name are ignored (the group already has its slides).",
                        parsed.ignored_lines
                    ))
                    .small()
                    .color(Color32::from_rgb(255, 190, 120)),
                );
            }
            let blank = d
                .slides()
                .filter(|s| s.text_boxes().all(|t| t.plain_text().trim().is_empty()))
                .count();
            if st.edit.lyrics.target == Target::Replace && blank > 0 {
                ui.label(
                    RichText::new(format!(
                        "{blank} slide(s) without text aren't shown as lyrics and will be removed by Replace."
                    ))
                    .small()
                    .color(Color32::from_rgb(255, 190, 120)),
                );
            }
            let preview = st.edit.lyrics.preview.clone();
            match preview {
                None => {
                    ui.label(RichText::new("Preparing preview…").weak());
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(Color32::from_rgb(255, 130, 130)));
                }
                Some(Ok(p)) => {
                    egui::ScrollArea::vertical()
                        .id_salt("lyrics-preview")
                        .max_height(avail.y - 90.0)
                        .show(ui, |ui| preview_grid(ui, st, thumbs, &p));
                }
            }
        });
    });
    ui.separator();
    ui.horizontal(|ui| {
        let l = &st.edit.lyrics;
        let new = l.target == Target::New;
        let ready = if new {
            !l.new_name.trim().is_empty() && st.edit.new_dir.is_some()
        } else {
            !parsed.groups.is_empty()
        };
        let label = match (new, l.text.trim().is_empty()) {
            (false, _) => "Apply",
            (true, true) => "Make blank presentation",
            (true, false) => "Make presentation",
        };
        if ui
            .add_enabled(ready, egui::Button::new(RichText::new(label).strong()))
            .clicked()
        {
            apply(st, ui.ctx(), actions, path, d);
        }
        if ui.button("Cancel").clicked() {
            st.edit.lyrics.open = false;
        }
    });
}

fn preview_grid(
    ui: &mut egui::Ui,
    st: &mut UiState,
    thumbs: &Sender<ThumbRequest>,
    p: &Arc<doc::Document>,
) {
    let tile_w = 150.0;
    let tile_h = tile_w * p.canvas.height as f32 / p.canvas.width.max(1) as f32;
    let mut flat = 0;
    for g in &p.groups {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, color32(g.color));
            ui.label(RichText::new(&g.name).strong());
        });
        ui.horizontal_wrapped(|ui| {
            for _ in &g.slides {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(tile_w, tile_h), egui::Sense::hover());
                let (_, tex) = st.thumb_public(thumbs, p, flat);
                match tex {
                    Some(Ok(t)) => {
                        egui::Image::new(t).paint_at(ui, rect);
                    }
                    Some(Err(e)) => paint_error(ui, rect, e),
                    None => {
                        ui.painter().rect_filled(rect, 0.0, Color32::from_gray(18));
                    }
                }
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, color32(g.color)),
                    egui::StrokeKind::Outside,
                );
                flat += 1;
            }
        });
        ui.add_space(4.0);
    }
}

fn apply(
    st: &mut UiState,
    ctx: &egui::Context,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
) {
    let theme = chosen_theme(st, path);
    let colors = colors(st);
    let text = st.edit.lyrics.text.clone();
    match st.edit.lyrics.target {
        Target::Replace | Target::Append => {
            let op = Op::SetLyrics {
                text,
                mode: if st.edit.lyrics.target == Target::Append {
                    LyricsMode::Append
                } else {
                    LyricsMode::Replace
                },
                colors,
                theme,
            };
            send_checked(st, actions, path, d, op, None);
            st.edit.sel = None;
            st.edit.element = None;
            st.edit.lyrics.open = false;
        }
        Target::New => {
            let Some(dir) = st.edit.new_dir.clone() else {
                return;
            };
            let name = st.edit.lyrics.new_name.trim().to_owned();
            let file: PathBuf = dir.join(format!("{}.{}", file_stem(&name), doc::NATIVE_EXT));
            // A blank presentation opens in the editor; a song is ready to use.
            let edit = text.trim().is_empty();
            if file.exists() || file.with_extension("pro6").exists() {
                st.toast(format!(
                    "A presentation named '{name}' already exists in {}. Choose another name.",
                    dir.display()
                ));
                return;
            }
            let theme = theme.map(|t| Box::new(t.for_document(&file)));
            match new_presentation(d, &file, &text, colors, theme) {
                Ok(n) => {
                    // Written on a worker thread, then opened in the editor.
                    let tx_ctx = ctx.clone();
                    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
                    let spawned = std::thread::Builder::new()
                        .name("new-presentation".into())
                        .spawn(move || {
                            let r = match std::fs::create_dir_all(&dir) {
                                Ok(()) => doc::save(&n).map(|s| s.path).map_err(|e| e.user_message()),
                                Err(e) => Err(format!(
                                    "The folder for new presentations, {}, couldn't be made ({e}). Choose another in Settings, Folders.",
                                    dir.display()
                                )),
                            };
                            let _ = done_tx.send(r);
                            tx_ctx.request_repaint();
                        });
                    match spawned {
                        Ok(_) => st.edit.lyrics_new = Some(Saving { rx: done_rx, edit }),
                        Err(e) => st.toast(format!("Couldn't save the new presentation: {e}")),
                    }
                    st.edit.lyrics.open = false;
                }
                Err(e) => st.toast(e),
            }
        }
    }
}

/// A new presentation finished saving: open it in the editor.
pub fn poll(st: &mut UiState, actions: &mut Vec<UiAction>) {
    let Some(saving) = &st.edit.lyrics_new else {
        return;
    };
    let Ok(r) = saving.rx.try_recv() else { return };
    let edit = saving.edit;
    st.edit.lyrics_new = None;
    match r {
        Ok(p) => {
            st.edit.sel = None;
            st.edit.element = None;
            if edit {
                // Turned on once it's selected, so the editor never shows the previous one.
                st.edit.open_when_selected = Some(p.clone());
            }
            actions.push(UiAction::RefreshLibrary);
            actions.push(UiAction::Core(opcore::Command::Open(p)));
        }
        Err(e) => st.toast(e),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn like_another_presentation_takes_its_look_not_its_slides() {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/song_1024x768.pro6");
        let d = doc::load(&src).unwrap();
        let mut st = UiState::default();
        st.edit.lyrics.like_selected = true;
        let (_, tpl) = template(&mut st, Some((&src, &d)));
        assert_eq!(tpl.groups.len(), 1);
        assert_eq!(tpl.groups[0].slides.len(), 1);
        let slide = &tpl.groups[0].slides[0];
        assert_eq!(slide.elements.len(), 1, "only the main text box");
        assert!(slide.notes.is_empty() && slide.label.is_empty());
        assert_eq!(tpl.canvas, d.canvas);
        let n = new_presentation(&tpl, Path::new("/x/T.opres"), "", Vec::new(), None).unwrap();
        assert_eq!(n.slides().count(), 1, "a blank presentation has one slide");
        let theirs = d.text_boxes().find(|t| t.runs().next().is_some()).unwrap();
        let ours = n.text_boxes().next().unwrap();
        assert_eq!(ours.frame, theirs.frame);
        assert_eq!(
            ours.runs().next().unwrap().font,
            theirs.runs().next().unwrap().font
        );
    }

    #[test]
    fn plain_look_and_blank_presentations() {
        let tpl = Theme::plain(PLAIN_CANVAS).to_document();
        let file = PathBuf::from("/x/New.opres");
        let n = new_presentation(&tpl, &file, "", Vec::new(), None).unwrap();
        assert_eq!(n.groups.len(), 1);
        assert_eq!(n.groups[0].name, "Slides");
        assert_eq!(n.groups[0].slides.len(), 1);
        assert!(n.text_boxes().all(|t| t.plain_text().is_empty()));
        assert_eq!(n.canvas, PLAIN_CANVAS);
        let song = new_presentation(&tpl, &file, EXAMPLE, Vec::new(), None).unwrap();
        let names: Vec<&str> = song.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Verse 1", "Chorus", "Verse 2"]);
        let run = song
            .text_boxes()
            .next()
            .unwrap()
            .runs()
            .next()
            .unwrap()
            .clone();
        assert_eq!(run.font, "Arial");
        assert_eq!(run.color, Some([255, 255, 255]));
    }

    #[test]
    fn a_new_presentation_from_lyrics_saves_as_opres_in_the_presentations_style() {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/song_1024x768.pro6");
        let d = doc::load(&src).unwrap();
        let dir =
            std::env::temp_dir().join(format!("openpresenter-lyrics-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("New Song.opres");
        let text = "[Verse]\nLine one\nLine two\n\n[Chorus]\nSing it\n\n[Verse]\n";
        let n = new_presentation(&d, &file, text, Vec::new(), None).unwrap();
        assert_eq!(n.mode, doc::Mode::Native);
        let names: Vec<&str> = n.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Verse", "Chorus"], "the template group is replaced");
        assert_eq!(to_text(&n), text);
        let style = d
            .text_boxes()
            .next()
            .unwrap()
            .runs()
            .next()
            .unwrap()
            .font
            .clone();
        assert!(n.text_boxes().all(|t| t.runs().all(|r| r.font == style)));
        let saved = doc::save(&n).unwrap();
        assert_eq!(saved.path, file);
        let back = doc::load(&file).unwrap();
        assert_eq!(to_text(&back), text);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
