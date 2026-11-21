//! Editing styled text: paragraphs of runs (see `model::Paragraph`). Positions are
//! `(paragraph, byte offset into the paragraph's text)`, always on a character
//! boundary. Every operation leaves the runs normalised: adjacent runs with the same
//! style are merged, and empty runs are dropped, except that an empty paragraph keeps
//! one empty run so it remembers its style.

use crate::model::{Align, Paragraph, Run};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub para: usize,
    pub offset: usize,
}

impl Pos {
    pub fn new(para: usize, offset: usize) -> Pos {
        Pos { para, offset }
    }
}

/// The paragraph's text (its runs joined).
pub fn text(p: &Paragraph) -> String {
    p.runs.iter().map(|r| r.text.as_str()).collect()
}

pub fn len(p: &Paragraph) -> usize {
    p.runs.iter().map(|r| r.text.len()).sum()
}

/// End of the text.
pub fn end(paras: &[Paragraph]) -> Pos {
    match paras.last() {
        Some(p) => Pos::new(paras.len() - 1, len(p)),
        None => Pos::default(),
    }
}

/// Clamp to an existing position on a character boundary.
pub fn clamp(paras: &[Paragraph], pos: Pos) -> Pos {
    if paras.is_empty() {
        return Pos::default();
    }
    let para = pos.para.min(paras.len() - 1);
    let t = text(&paras[para]);
    let mut offset = pos.offset.min(t.len());
    while !t.is_char_boundary(offset) {
        offset -= 1;
    }
    Pos::new(para, offset)
}

/// Previous / next character position (crossing paragraphs).
pub fn prev(paras: &[Paragraph], pos: Pos) -> Pos {
    let pos = clamp(paras, pos);
    if pos.offset == 0 {
        if pos.para == 0 {
            return pos;
        }
        return Pos::new(pos.para - 1, len(&paras[pos.para - 1]));
    }
    let t = text(&paras[pos.para]);
    let o = t[..pos.offset]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i);
    Pos::new(pos.para, o)
}

pub fn next(paras: &[Paragraph], pos: Pos) -> Pos {
    let pos = clamp(paras, pos);
    let t = text(&paras[pos.para]);
    if pos.offset >= t.len() {
        if pos.para + 1 >= paras.len() {
            return pos;
        }
        return Pos::new(pos.para + 1, 0);
    }
    let c = t[pos.offset..].chars().next().map_or(0, char::len_utf8);
    Pos::new(pos.para, pos.offset + c)
}

/// Start of the word before `pos` (Ctrl+Left).
pub fn word_prev(paras: &[Paragraph], pos: Pos) -> Pos {
    let pos = clamp(paras, pos);
    if pos.offset == 0 {
        return prev(paras, pos);
    }
    let t = text(&paras[pos.para]);
    let before = &t[..pos.offset];
    let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
    let start = trimmed
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric())
        .map_or(0, |(i, c)| i + c.len_utf8());
    Pos::new(pos.para, start)
}

/// End of the word after `pos` (Ctrl+Right).
pub fn word_next(paras: &[Paragraph], pos: Pos) -> Pos {
    let pos = clamp(paras, pos);
    let t = text(&paras[pos.para]);
    if pos.offset >= t.len() {
        return next(paras, pos);
    }
    let after = &t[pos.offset..];
    let skip = after.len()
        - after
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .len();
    let rest = &after[skip..];
    let word = rest
        .char_indices()
        .find(|(_, c)| !c.is_alphanumeric())
        .map_or(rest.len(), |(i, _)| i);
    Pos::new(pos.para, pos.offset + skip + word)
}

/// Style for text typed at `pos`: the run the character before it belongs to (or
/// the first run of the paragraph at its start), else a neighbouring paragraph's.
pub fn style_at(paras: &[Paragraph], pos: Pos) -> Option<Run> {
    let pos = clamp(paras, pos);
    let p = paras.get(pos.para)?;
    let mut at = 0;
    for r in &p.runs {
        let end = at + r.text.len();
        if pos.offset > at && pos.offset <= end {
            return Some(styled(r));
        }
        at = end;
    }
    if let Some(r) = p.runs.first() {
        return Some(styled(r));
    }
    // Empty paragraph without runs: borrow from the nearest paragraph.
    (0..paras.len())
        .flat_map(|d| [pos.para.checked_sub(d), pos.para.checked_add(d)])
        .flatten()
        .filter_map(|i| paras.get(i))
        .find_map(|p| p.runs.first().map(styled))
}

fn styled(r: &Run) -> Run {
    Run {
        text: String::new(),
        ..r.clone()
    }
}

/// Merge adjacent same-style runs and drop empty ones (an empty paragraph keeps one
/// empty run, to hold its style).
pub fn normalize(paras: &mut [Paragraph]) {
    for p in paras {
        let keep = p.runs.first().map(styled);
        let mut out: Vec<Run> = Vec::with_capacity(p.runs.len());
        for r in p.runs.drain(..) {
            if r.text.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.same_style(&r) => last.text.push_str(&r.text),
                _ => out.push(r),
            }
        }
        if out.is_empty()
            && let Some(k) = keep
        {
            out.push(k);
        }
        p.runs = out;
    }
}

/// Split the run containing `offset` so a run boundary falls exactly there; returns
/// the index of the first run at or after `offset`.
fn split_at(p: &mut Paragraph, offset: usize) -> usize {
    let mut at = 0;
    for i in 0..p.runs.len() {
        let len = p.runs[i].text.len();
        if offset == at {
            return i;
        }
        if offset < at + len {
            let tail = p.runs[i].text.split_off(offset - at);
            let mut r = p.runs[i].clone();
            r.text = tail;
            p.runs.insert(i + 1, r);
            return i + 1;
        }
        at += len;
    }
    p.runs.len()
}

/// Insert `s` at `pos` in `style`. `'\n'` in `s` starts a new paragraph; use
/// [`LINE_BREAK`] for a line break inside the paragraph. Returns the position after
/// the inserted text.
pub fn insert(paras: &mut Vec<Paragraph>, pos: Pos, s: &str, style: &Run) -> Pos {
    if paras.is_empty() {
        paras.push(Paragraph {
            runs: vec![styled(style)],
            ..Paragraph::default()
        });
    }
    let mut pos = clamp(paras, pos);
    for (i, piece) in s.split('\n').enumerate() {
        if i > 0 {
            pos = split(paras, pos);
        }
        if piece.is_empty() {
            continue;
        }
        let p = &mut paras[pos.para];
        let at = split_at(p, pos.offset);
        p.runs.insert(
            at,
            Run {
                text: piece.replace(LINE_BREAK, "\n"),
                ..style.clone()
            },
        );
        pos.offset += piece.len();
    }
    normalize(paras);
    pos
}

/// Insert it to get a line break inside a paragraph (`\line` in RTF, Shift+Enter);
/// it's stored as `'\n'` in the run's text.
pub const LINE_BREAK: char = '\u{2028}';

/// Split the paragraph at `pos` (Enter); the new paragraph keeps the alignment and
/// spacing. Returns the start of the new paragraph.
pub fn split(paras: &mut Vec<Paragraph>, pos: Pos) -> Pos {
    let pos = clamp(paras, pos);
    let p = &mut paras[pos.para];
    let style = style_at(std::slice::from_ref(p), Pos::new(0, pos.offset));
    let at = split_at(p, pos.offset);
    let tail: Vec<Run> = p.runs.drain(at..).collect();
    let mut new = Paragraph {
        align: p.align,
        line_spacing: p.line_spacing,
        runs: tail,
    };
    if new.runs.is_empty()
        && let Some(s) = style.clone()
    {
        new.runs.push(s);
    }
    if p.runs.is_empty()
        && let Some(s) = style
    {
        p.runs.push(s);
    }
    paras.insert(pos.para + 1, new);
    normalize(paras);
    Pos::new(pos.para + 1, 0)
}

/// Delete between `a` and `b` (either order). Returns the position where they were.
pub fn delete(paras: &mut Vec<Paragraph>, a: Pos, b: Pos) -> Pos {
    let (a, b) = (clamp(paras, a.min(b)), clamp(paras, a.max(b)));
    if a == b {
        return a;
    }
    // The style the emptied paragraph keeps, if everything in it goes.
    let keep = style_at(paras, next(paras, a));
    if a.para == b.para {
        let p = &mut paras[a.para];
        let i = split_at(p, a.offset);
        let j = split_at(p, b.offset);
        p.runs.drain(i..j);
    } else {
        // Keep the start of a's paragraph and the end of b's, joined.
        let tail = {
            let pb = &mut paras[b.para];
            let j = split_at(pb, b.offset);
            pb.runs.split_off(j)
        };
        let pa = &mut paras[a.para];
        let i = split_at(pa, a.offset);
        pa.runs.truncate(i);
        pa.runs.extend(tail);
        paras.drain(a.para + 1..=b.para);
    }
    normalize(paras);
    if paras[a.para].runs.is_empty()
        && let Some(k) = keep
    {
        paras[a.para].runs.push(k);
    }
    a
}

/// Change the style of the runs between `a` and `b`.
#[allow(clippy::needless_range_loop)] // positions index paragraphs
pub fn restyle(paras: &mut [Paragraph], a: Pos, b: Pos, mut f: impl FnMut(&mut Run)) {
    let (a, b) = (clamp(paras, a.min(b)), clamp(paras, a.max(b)));
    for pi in a.para..=b.para {
        let p = &mut paras[pi];
        let from = if pi == a.para { a.offset } else { 0 };
        let to = if pi == b.para { b.offset } else { len(p) };
        if from == to && len(p) > 0 {
            continue;
        }
        if len(p) == 0 {
            // Empty paragraph: its style-holding run.
            p.runs.iter_mut().for_each(&mut f);
            continue;
        }
        let i = split_at(p, from);
        let j = split_at(p, to);
        p.runs[i..j].iter_mut().for_each(&mut f);
    }
    normalize(paras);
}

/// Change every run.
pub fn restyle_all(paras: &mut [Paragraph], f: impl FnMut(&mut Run)) {
    let e = end(paras);
    restyle(paras, Pos::default(), e, f);
}

/// Set the alignment of the paragraphs `a` to `b` touch.
pub fn set_align(paras: &mut [Paragraph], a: Pos, b: Pos, align: Align) {
    let (a, b) = (a.min(b), a.max(b));
    for p in paras.iter_mut().take(b.para + 1).skip(a.para) {
        p.align = align;
    }
}

pub fn set_line_spacing(paras: &mut [Paragraph], a: Pos, b: Pos, spacing: f64) {
    let (a, b) = (a.min(b), a.max(b));
    for p in paras.iter_mut().take(b.para + 1).skip(a.para) {
        p.line_spacing = spacing;
    }
}

/// The runs (with their text) between `a` and `b`, for reading a selection's style.
#[allow(clippy::needless_range_loop)] // positions index paragraphs
pub fn runs_between(paras: &[Paragraph], a: Pos, b: Pos) -> Vec<Run> {
    let mut copy = paras.to_vec();
    let (a, b) = (clamp(paras, a.min(b)), clamp(paras, a.max(b)));
    let mut out = Vec::new();
    for pi in a.para..=b.para {
        let p = &mut copy[pi];
        let from = if pi == a.para { a.offset } else { 0 };
        let to = if pi == b.para { b.offset } else { len(p) };
        let i = split_at(p, from);
        let j = split_at(p, to);
        out.extend(p.runs[i..j].iter().cloned());
    }
    out
}

/// Plain text between `a` and `b` (paragraphs joined with `'\n'`).
#[allow(clippy::needless_range_loop)] // positions index paragraphs
pub fn text_between(paras: &[Paragraph], a: Pos, b: Pos) -> String {
    let (a, b) = (clamp(paras, a.min(b)), clamp(paras, a.max(b)));
    let mut s = String::new();
    for pi in a.para..=b.para {
        let t = text(&paras[pi]);
        let from = if pi == a.para { a.offset } else { 0 };
        let to = if pi == b.para { b.offset } else { t.len() };
        if pi > a.para {
            s.push('\n');
        }
        s.push_str(&t[from..to]);
    }
    s
}

/// Position of byte `o` in the plain text of `paras` (paragraphs joined with `'\n'`,
/// as [`crate::TextBox::plain_text`] makes it).
fn plain_pos(paras: &[Paragraph], mut o: usize) -> Pos {
    for (i, p) in paras.iter().enumerate() {
        let l = len(p);
        if o <= l {
            return Pos::new(i, o);
        }
        o -= l + 1;
    }
    end(paras)
}

/// `paras` with its plain text changed from `old` to `new`, keeping the styles: only
/// the part that differs is replaced, in the style of the text before it (quick edit,
/// spelling fixes). `'\n'` in `new` is a new line the way this box already makes them:
/// a line break inside the paragraph if it uses those, else a new paragraph.
pub fn replace_plain(paras: &[Paragraph], old: &str, new: &str) -> Vec<Paragraph> {
    let mut out = paras.to_vec();
    let line_breaks = out
        .iter()
        .any(|p| p.runs.iter().any(|r| r.text.contains('\n')));
    // Changed regions as (start, end in `old`, replacement), applied last first so the
    // earlier offsets stay valid. Same number of lines: each changed line on its own
    // (a fix in the first line and another in the last keep the styles between).
    let mut regions: Vec<(usize, usize, &str)> = Vec::new();
    let (ol, nl): (Vec<&str>, Vec<&str>) = (old.split('\n').collect(), new.split('\n').collect());
    if ol.len() == nl.len() {
        let mut at = 0;
        for (o, n) in ol.iter().zip(&nl) {
            if o != n {
                let (a, b, m) = diff(o, n);
                regions.push((at + a, at + b, m));
            }
            at += o.len() + 1;
        }
    } else {
        let (a, b, m) = diff(old, new);
        regions.push((a, b, m));
    }
    for (a, b, mid) in regions.into_iter().rev() {
        let (pa, pb) = (plain_pos(&out, a), plain_pos(&out, b));
        let style =
            style_at(&out, pa).or_else(|| out.iter().flat_map(|p| &p.runs).next().map(styled));
        let mid = if line_breaks {
            mid.replace('\n', &LINE_BREAK.to_string())
        } else {
            mid.to_owned()
        };
        let at = delete(&mut out, pa, pb);
        if let Some(style) = style
            && !mid.is_empty()
        {
            insert(&mut out, at, &mid, &style);
        }
    }
    out
}

/// The part of `old` that differs from `new`: `(start, end)` in `old` and what
/// replaces it, after taking off the common start and end.
fn diff<'a>(old: &str, new: &'a str) -> (usize, usize, &'a str) {
    let pre = old
        .char_indices()
        .zip(new.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, a), _)| i + a.len_utf8());
    let suf: usize = old[pre..]
        .chars()
        .rev()
        .zip(new[pre..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (pre, old.len() - suf, &new[pre..new.len() - suf])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(paras: &[Paragraph]) -> String {
        paras.iter().map(text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn replace_plain_keeps_the_styles_around_the_change() {
        let paras = vec![
            para(vec![run("Amazing ", false), run("grase", true)]),
            para(vec![run("how sweet", false)]),
        ];
        let old = plain(&paras);
        let out = replace_plain(&paras, &old, "Amazing grace\nhow sweet the sound");
        assert_eq!(
            texts(&out),
            vec![
                vec![("Amazing ".into(), false), ("grace".into(), true)],
                vec![("how sweet the sound".into(), false)],
            ]
        );
        // A new line where the box uses paragraphs: a new paragraph.
        let out = replace_plain(&paras, &old, "Amazing grase\nhow\nsweet");
        assert_eq!(out.len(), 3);
        assert_eq!(plain(&out), "Amazing grase\nhow\nsweet");
        // A box with line breaks inside one paragraph keeps using them.
        let lb = vec![para(vec![run("one\ntwo", false)])];
        let out = replace_plain(&lb, "one\ntwo", "one\ntwo\nthree");
        assert_eq!(out.len(), 1);
        assert_eq!(text(&out[0]), "one\ntwo\nthree");
        // Unchanged, emptied, and multi-byte text.
        assert_eq!(replace_plain(&paras, &old, &old), paras);
        let out = replace_plain(&paras, &old, "");
        assert_eq!(plain(&out), "");
        let out = replace_plain(&lb, "one\ntwo", "ône\ntwo");
        assert_eq!(text(&out[0]), "ône\ntwo");
    }

    fn run(text: &str, bold: bool) -> Run {
        Run {
            text: text.into(),
            font: "Arial".into(),
            size: 40.0,
            color: None,
            bold,
            italic: false,
            underline: false,
            letter_spacing: 0.0,
        }
    }

    fn para(runs: Vec<Run>) -> Paragraph {
        Paragraph {
            runs,
            ..Paragraph::default()
        }
    }

    fn texts(paras: &[Paragraph]) -> Vec<Vec<(String, bool)>> {
        paras
            .iter()
            .map(|p| p.runs.iter().map(|r| (r.text.clone(), r.bold)).collect())
            .collect()
    }

    fn sample() -> Vec<Paragraph> {
        vec![
            para(vec![run("Amazing ", false), run("grace", true)]),
            para(vec![run("how sweet", false)]),
        ]
    }

    #[test]
    fn typing_takes_the_style_before_the_caret() {
        let mut p = sample();
        let plain = style_at(&p, Pos::new(0, 3)).unwrap();
        assert!(!plain.bold);
        let bold = style_at(&p, Pos::new(0, 13)).unwrap();
        assert!(bold.bold, "end of the bold run");
        let at = insert(&mut p, Pos::new(0, 13), "!", &bold);
        assert_eq!(at, Pos::new(0, 14));
        assert_eq!(
            texts(&p)[0],
            vec![("Amazing ".into(), false), ("grace!".into(), true)]
        );
    }

    #[test]
    fn enter_splits_and_backspace_joins() {
        let mut p = sample();
        let at = split(&mut p, Pos::new(0, 8));
        assert_eq!(at, Pos::new(1, 0));
        assert_eq!(text(&p[0]), "Amazing ");
        assert_eq!(text(&p[1]), "grace");
        assert_eq!(p.len(), 3);
        // Backspace at the start of a paragraph joins it to the previous one.
        let b = prev(&p, at);
        let at = delete(&mut p, b, at);
        assert_eq!(at, Pos::new(0, 8));
        assert_eq!(texts(&p), texts(&sample()));
    }

    #[test]
    fn deleting_across_paragraphs() {
        let mut p = sample();
        delete(&mut p, Pos::new(0, 3), Pos::new(1, 4));
        assert_eq!(p.len(), 1);
        assert_eq!(text(&p[0]), "Amasweet");
        assert_eq!(
            texts(&p),
            vec![vec![("Amasweet".into(), false)]],
            "runs merged"
        );
    }

    #[test]
    fn restyle_splits_runs_and_merges_back() {
        let mut p = sample();
        restyle(&mut p, Pos::new(1, 4), Pos::new(1, 9), |r| r.bold = true);
        assert_eq!(
            texts(&p)[1],
            vec![("how ".into(), false), ("sweet".into(), true)]
        );
        restyle(&mut p, Pos::new(1, 0), Pos::new(1, 9), |r| r.bold = false);
        assert_eq!(texts(&p)[1], vec![("how sweet".into(), false)]);
        // Across paragraphs.
        restyle(&mut p, Pos::new(0, 4), Pos::new(1, 3), |r| r.italic = true);
        assert!(p[0].runs.iter().any(|r| r.italic && r.text == "ing "));
        assert!(p[1].runs[0].italic && p[1].runs[0].text == "how");
    }

    #[test]
    fn empty_paragraphs_keep_their_style() {
        let mut p = vec![para(vec![run("x", true)])];
        let at = delete(&mut p, Pos::new(0, 0), Pos::new(0, 1));
        assert_eq!(p[0].runs.len(), 1, "style kept in an empty run");
        assert_eq!(p[0].runs[0].text, "");
        let s = style_at(&p, at).unwrap();
        assert!(s.bold);
        insert(&mut p, at, "y", &s);
        assert_eq!(texts(&p), vec![vec![("y".into(), true)]]);
    }

    #[test]
    fn multiline_paste_makes_paragraphs() {
        let mut p = sample();
        let s = style_at(&p, Pos::new(1, 0)).unwrap();
        let at = insert(&mut p, Pos::new(1, 0), "one\ntwo\n", &s);
        assert_eq!(at, Pos::new(3, 0));
        let t: Vec<String> = p.iter().map(text).collect();
        assert_eq!(t, ["Amazing grace", "one", "two", "how sweet"]);
        assert_eq!(
            text_between(&p, Pos::new(0, 8), Pos::new(2, 3)),
            "grace\none\ntwo"
        );
    }

    #[test]
    fn caret_moves_by_characters_and_words() {
        let p = vec![para(vec![run("héllo wörld", false)])];
        assert_eq!(next(&p, Pos::new(0, 1)), Pos::new(0, 3), "é is two bytes");
        assert_eq!(prev(&p, Pos::new(0, 3)), Pos::new(0, 1));
        assert_eq!(word_next(&p, Pos::new(0, 0)), Pos::new(0, 6));
        assert_eq!(word_next(&p, Pos::new(0, 6)), Pos::new(0, 13));
        assert_eq!(word_prev(&p, Pos::new(0, 13)), Pos::new(0, 7));
        assert_eq!(clamp(&p, Pos::new(0, 2)), Pos::new(0, 1), "inside é");
    }
}
