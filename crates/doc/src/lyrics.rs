//! Paste lyrics to slides (docs/EDITOR-PLAN.md E8).
//!
//! Text format:
//! - A line `[Name]` (e.g. `[Verse 1]`, `[Chorus]`) starts a group with that name.
//! - A blank line starts a new slide.
//! - A group name used again points at the same group (a repeat): its lines, if
//!   any, are ignored when the group already has slides.
//! - The order of the `[Name]` lines is the arrangement.
//! - Headings as lyric websites write them also start a group, when they're alone
//!   on a line: `Verse 1:`, `Chorus`, `(Bridge)`, `Pre-Chorus 2:` (see [`SECTION_WORDS`]).
//!
//! [`to_text`] goes the other way, so a presentation can be edited as text and
//! re-flowed; [`Edit::SetLyrics`] applies text, keeping the slides (and their
//! style and `.pro6` XML) of groups that keep their name.

use crate::edit::{Edit, EditError, new_uuid, set_text_pub};
use crate::model::*;

/// Name for lines before the first `[Name]`.
pub const UNNAMED: &str = "Lyrics";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGroup {
    pub name: String,
    /// Each slide's text (`\n` between lines).
    pub slides: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub groups: Vec<ParsedGroup>,
    /// Group indices in the order the text uses them (repeats included).
    pub order: Vec<usize>,
    /// Lines under a repeated name that were ignored (the group had its slides).
    pub ignored_lines: usize,
}

impl Parsed {
    pub fn slide_count(&self) -> usize {
        self.groups.iter().map(|g| g.slides.len()).sum()
    }

    /// Every group once, in order: no arrangement needed.
    pub fn natural_order(&self) -> bool {
        self.order.iter().copied().eq(0..self.groups.len())
    }
}

/// Words that, alone on a line (with an optional number, `:` or brackets), are a
/// section heading as lyric websites write them.
pub const SECTION_WORDS: &[&str] = &[
    "verse",
    "chorus",
    "pre-chorus",
    "pre chorus",
    "prechorus",
    "bridge",
    "tag",
    "intro",
    "outro",
    "ending",
    "refrain",
    "interlude",
    "instrumental",
    "vamp",
    "hook",
    "coda",
];

fn header(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let name = name.trim();
        return (!name.is_empty() && !name.contains(['[', ']'])).then(|| name.to_owned());
    }
    site_header(t)
}

/// `Verse 1:`, `CHORUS`, `(Bridge)`, `Pre-Chorus 2` → the group name.
fn site_header(t: &str) -> Option<String> {
    let mut s = t.trim();
    if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        s = inner.trim();
    }
    let s = s.strip_suffix(':').unwrap_or(s).trim();
    let lower = s.to_lowercase();
    let word = SECTION_WORDS
        .iter()
        .filter(|w| lower.starts_with(*w))
        .max_by_key(|w| w.len())?;
    let rest = s[word.len()..].trim();
    // Only a number (and maybe a letter): "Verse 2", "Verse 2a". Not "Chorus of angels".
    let ok = rest.is_empty()
        || (rest.len() <= 4
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            && rest.chars().all(|c| c.is_ascii_alphanumeric()));
    if !ok {
        return None;
    }
    // Name as the editor writes it: "Verse 1", "Pre-Chorus".
    let base = match *word {
        "pre chorus" | "prechorus" | "pre-chorus" => "Pre-Chorus".to_owned(),
        w => {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    };
    Some(if rest.is_empty() {
        base
    } else {
        format!("{base} {rest}")
    })
}

pub fn parse(text: &str) -> Parsed {
    let mut p = Parsed::default();
    // Current group, whether its lines count, and the slide being built.
    let mut cur: Option<(usize, bool)> = None;
    let mut slide: Vec<String> = Vec::new();
    let flush = |p: &mut Parsed, cur: Option<(usize, bool)>, slide: &mut Vec<String>| {
        if slide.is_empty() {
            return;
        }
        match cur {
            Some((g, true)) => p.groups[g].slides.push(slide.join("\n")),
            Some((_, false)) => p.ignored_lines += slide.len(),
            None => {}
        }
        slide.clear();
    };
    for raw in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let line = raw.trim_end();
        if let Some(name) = header(line) {
            let name = name.as_str();
            flush(&mut p, cur, &mut slide);
            let g = match p
                .groups
                .iter()
                .position(|g| g.name.eq_ignore_ascii_case(name))
            {
                Some(g) => g,
                None => {
                    p.groups.push(ParsedGroup {
                        name: name.to_owned(),
                        slides: Vec::new(),
                    });
                    p.groups.len() - 1
                }
            };
            p.order.push(g);
            cur = Some((g, p.groups[g].slides.is_empty()));
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut p, cur, &mut slide);
            continue;
        }
        if cur.is_none() {
            p.groups.push(ParsedGroup {
                name: UNNAMED.to_owned(),
                slides: Vec::new(),
            });
            p.order.push(p.groups.len() - 1);
            cur = Some((p.groups.len() - 1, true));
        }
        slide.push(line.to_owned());
    }
    flush(&mut p, cur, &mut slide);
    // A name with no slides anywhere is dropped (and from the order).
    let keep: Vec<bool> = p.groups.iter().map(|g| !g.slides.is_empty()).collect();
    if keep.iter().any(|k| !k) {
        let mut map = Vec::with_capacity(keep.len());
        let mut n = 0;
        for k in &keep {
            map.push(n);
            if *k {
                n += 1;
            }
        }
        p.order = p
            .order
            .iter()
            .filter(|&&g| keep[g])
            .map(|&g| map[g])
            .collect();
        let mut i = 0;
        p.groups.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
    }
    p
}

/// A slide's lyrics: its first text box with text (`\n` between lines).
fn slide_text(s: &Slide) -> String {
    s.text_boxes()
        .map(TextBox::plain_text)
        .find(|t| !t.trim().is_empty())
        .unwrap_or_default()
}

/// The presentation as lyrics text: groups in the selected arrangement's order
/// (or their own), `[Name]` before each, slides separated by blank lines; a repeat
/// is just its `[Name]`.
pub fn to_text(d: &Document) -> String {
    let order: Vec<usize> = match d.selected_arrangement() {
        Some(a) if !a.groups.is_empty() => {
            let mut o = a.groups.clone();
            // Groups the arrangement leaves out still need their text.
            for g in 0..d.groups.len() {
                if !o.contains(&g) {
                    o.push(g);
                }
            }
            o
        }
        _ => (0..d.groups.len()).collect(),
    };
    let mut out = String::new();
    let mut done = vec![false; d.groups.len()];
    for g in order {
        let gr = &d.groups[g];
        if !out.is_empty() {
            out.push('\n');
        }
        out.push('[');
        out.push_str(if gr.name.trim().is_empty() {
            UNNAMED
        } else {
            gr.name.trim()
        });
        out.push_str("]\n");
        if done[g] {
            continue;
        }
        done[g] = true;
        let texts: Vec<String> = gr
            .slides
            .iter()
            .map(slide_text)
            .map(|t| {
                // Blank lines inside a slide would split it: drop them.
                t.lines()
                    .map(str::trim_end)
                    .filter(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|t| !t.is_empty())
            .collect();
        out.push_str(&texts.join("\n\n"));
        if !texts.is_empty() {
            out.push('\n');
        }
    }
    out
}

/// How [`Edit::SetLyrics`] uses the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsMode {
    /// The text becomes the presentation (groups that keep their name keep their
    /// slides and look).
    Replace,
    /// The text's groups are added after the existing ones.
    Append,
}

/// Default colour for a group called `name`: the first entry whose name starts
/// `name` (case-insensitive), e.g. "Verse" for "Verse 2".
pub fn default_color(name: &str, defaults: &[(String, Rgba)]) -> Option<Rgba> {
    let n = name.trim().to_lowercase();
    defaults
        .iter()
        .find(|(k, _)| {
            let k = k.trim().to_lowercase();
            !k.is_empty() && n.starts_with(&k)
        })
        .map(|(_, c)| *c)
}

fn styled(t: &TextBox) -> bool {
    t.problem.is_none() && t.runs().next().is_some()
}

/// A slide to copy the look from: the first slide with styled text.
fn template(d: &Document) -> Option<Slide> {
    d.slides().find(|s| s.text_boxes().any(styled)).cloned()
}

fn new_slide(template: &Slide, text: &str) -> Result<Slide, EditError> {
    let mut s = crate::convert::fresh_slide(template);
    s.enabled = true;
    s.label.clear();
    s.notes.clear();
    s.hot_key.clear();
    s.timer = None;
    // Only the first text box carries the lyrics; other text boxes are emptied.
    let mut first = true;
    for e in &mut s.elements {
        if let Element::Text(t) = e
            && styled(t)
        {
            set_text_pub(t, if first { text } else { "" })?;
            first = false;
        }
    }
    Ok(s)
}

/// Apply lyrics `text` to `doc`.
pub(crate) fn apply(
    doc: &mut Document,
    text: &str,
    mode: LyricsMode,
    colors: &[(String, Rgba)],
    theme: Option<&crate::Theme>,
) -> Result<(), EditError> {
    let p = parse(text);
    let template = template(doc).ok_or(EditError::NoTemplate)?;
    let mut groups: Vec<Group> = Vec::new();
    let mut fresh: Vec<(usize, usize)> = Vec::new();
    let mut used = vec![false; doc.groups.len()];
    // Group index (in the result) of each parsed group.
    let mut index: Vec<usize> = Vec::with_capacity(p.groups.len());
    if mode == LyricsMode::Append {
        groups = doc.groups.clone();
    }
    for pg in &p.groups {
        let existing = doc
            .groups
            .iter()
            .enumerate()
            .find(|(i, g)| !used[*i] && g.name.trim().eq_ignore_ascii_case(&pg.name));
        match (mode, existing) {
            (LyricsMode::Append, Some((i, _))) => {
                // Already there: points at it (its slides stay as they are).
                used[i] = true;
                index.push(i);
            }
            (LyricsMode::Replace, Some((i, g))) => {
                used[i] = true;
                let mut ng = g.clone();
                ng.slides.clear();
                for (si, t) in pg.slides.iter().enumerate() {
                    match g.slides.get(si) {
                        Some(old) if slide_text(old) == *t => ng.slides.push(old.clone()),
                        Some(old) if old.text_boxes().any(styled) => {
                            // Same slide, new words: its first styled box takes them.
                            let mut s = old.clone();
                            if let Some(Element::Text(tb)) = s
                                .elements
                                .iter_mut()
                                .find(|e| matches!(e, Element::Text(tb) if styled(tb)))
                            {
                                set_text_pub(tb, t)?;
                            }
                            ng.slides.push(s);
                        }
                        _ => {
                            fresh.push((groups.len(), ng.slides.len()));
                            ng.slides.push(new_slide(&template, t)?);
                        }
                    }
                }
                index.push(groups.len());
                groups.push(ng);
            }
            (_, None) => {
                let mut ng = Group {
                    uuid: new_uuid(),
                    name: pg.name.clone(),
                    color: default_color(&pg.name, colors),
                    slides: Vec::new(),
                    src: None,
                };
                for t in &pg.slides {
                    fresh.push((groups.len(), ng.slides.len()));
                    ng.slides.push(new_slide(&template, t)?);
                }
                index.push(groups.len());
                groups.push(ng);
            }
        }
    }
    // Arrangement: the text's order.
    let seq: Vec<String> = p
        .order
        .iter()
        .map(|&g| groups[index[g]].uuid.clone())
        .collect();
    let removed: Vec<String> = if mode == LyricsMode::Replace {
        doc.groups
            .iter()
            .filter(|g| !groups.iter().any(|n| n.uuid == g.uuid))
            .map(|g| g.uuid.clone())
            .collect()
    } else {
        Vec::new()
    };
    doc.groups = groups;
    for a in &mut doc.arrangements {
        a.groups
            .retain(|id| !removed.iter().any(|r| r.eq_ignore_ascii_case(id)));
    }
    let natural =
        mode == LyricsMode::Replace && p.natural_order() && doc.groups.len() == p.groups.len();
    match doc.selected_arrangement.clone().and_then(|id| {
        doc.arrangements
            .iter()
            .position(|a| a.uuid.eq_ignore_ascii_case(&id))
    }) {
        Some(ai) => match mode {
            LyricsMode::Replace => doc.arrangements[ai].groups = seq,
            LyricsMode::Append => doc.arrangements[ai].groups.extend(seq),
        },
        None if !natural && mode == LyricsMode::Replace => {
            let uuid = new_uuid();
            doc.arrangements.push(Arrangement {
                uuid: uuid.clone(),
                name: "Lyrics".into(),
                color: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }),
                groups: seq,
            });
            doc.selected_arrangement = Some(uuid);
        }
        None => {}
    }
    if let Some(t) = theme
        && !fresh.is_empty()
    {
        let link = doc.mode == crate::Mode::Native;
        t.apply(doc, &fresh, false, link);
    }
    Ok(())
}

impl Edit {
    /// Convenience for [`Edit::SetLyrics`].
    pub fn lyrics(text: &str, mode: LyricsMode) -> Edit {
        Edit::SetLyrics {
            text: text.to_owned(),
            mode,
            colors: Vec::new(),
            theme: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONG: &str = "[Verse 1]
Amazing grace how sweet the sound
That saved a wretch like me

I once was lost but now am found
Was blind but now I see

[Chorus]
My chains are gone
I've been set free

[Verse 2]
'Twas grace that taught my heart to fear

[Chorus]

[Chorus]
";

    #[test]
    fn parses_groups_slides_and_order() {
        let p = parse(SONG);
        let names: Vec<&str> = p.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Verse 1", "Chorus", "Verse 2"]);
        assert_eq!(p.groups[0].slides.len(), 2);
        assert_eq!(
            p.groups[0].slides[1],
            "I once was lost but now am found\nWas blind but now I see"
        );
        assert_eq!(p.order, vec![0, 1, 2, 1, 1]);
        assert!(!p.natural_order());
        assert_eq!(p.ignored_lines, 0);
    }

    #[test]
    fn repeats_with_text_keep_the_first_and_count_the_rest() {
        let p = parse("[Chorus]\nA\n\n[Verse]\nB\n\n[chorus]\nA again\n");
        assert_eq!(p.groups.len(), 2);
        assert_eq!(p.groups[0].slides, vec!["A"]);
        assert_eq!(p.order, vec![0, 1, 0]);
        assert_eq!(p.ignored_lines, 1);
    }

    #[test]
    fn text_before_any_name_and_blank_runs() {
        let p = parse("one\n\n\n\ntwo\n[Empty]\n\n");
        assert_eq!(p.groups.len(), 1, "a name with nothing under it is dropped");
        assert_eq!(p.groups[0].name, UNNAMED);
        assert_eq!(p.groups[0].slides, vec!["one", "two"]);
        assert_eq!(p.order, vec![0]);
    }

    #[test]
    fn headings_from_lyric_websites_start_groups() {
        let p = parse(
            "Verse 1:\nAmazing grace\n\nCHORUS\nMy chains are gone\n\n(Bridge)\nB\n\npre-chorus 2\nP\n\nchorus:\n\nChorus of angels sing\n",
        );
        let names: Vec<&str> = p.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Verse 1", "Chorus", "Bridge", "Pre-Chorus 2"]);
        assert_eq!(p.order, vec![0, 1, 2, 3, 1]);
        assert_eq!(
            p.ignored_lines, 1,
            "'Chorus of angels sing' is a lyric line (under the repeated Chorus), not a heading"
        );
        assert_eq!(site_header("Verse 2a"), Some("Verse 2a".into()));
        assert_eq!(site_header("Versed in grace"), None);
        assert_eq!(site_header("Tag x2"), None);
    }

    #[test]
    fn default_colours_match_by_name_start() {
        let red = Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let d = vec![("Chorus".to_owned(), red)];
        assert_eq!(default_color("chorus 2", &d), Some(red));
        assert_eq!(default_color("Verse", &d), None);
    }
}
