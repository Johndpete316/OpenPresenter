//! Hand-rolled RTF subset reader (`rtf-parser` 0.5 evaluated and rejected; see
//! docs/format-notes.md).
//!
//! Extracts what slide rendering needs: font table, colour table, and per-run
//! font / size (`\fs`, half-points) / colour / bold / italic / underline, grouped
//! into paragraphs with alignment. Control words it doesn't act on are counted in
//! [`RtfDoc::unhandled_words`] so corpus reports show what PP6 actually emits.

use std::collections::BTreeMap;

/// RTF default font size: 12 pt.
pub const DEFAULT_SIZE_HALF_PT: u32 = 24;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RtfDoc {
    /// Font table keyed by `\fN` index, in table order.
    pub fonts: Vec<FontEntry>,
    /// Colour table; index 0 is usually the "auto" (empty) entry → `None`.
    pub colors: Vec<Option<Rgb>>,
    pub paragraphs: Vec<Paragraph>,
    /// `\ansicpg` value, if present.
    pub code_page: Option<u16>,
    /// Control words seen but not acted on, with counts.
    pub unhandled_words: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontEntry {
    pub index: i32,
    pub name: String,
    pub charset: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justified,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    pub align: Align,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Text; `\line` appears as `'\n'`, `\tab` as `'\t'`.
    pub text: String,
    /// Font name resolved from the font table; `None` if `\fN` isn't in the table.
    pub font: Option<String>,
    pub font_index: i32,
    pub size_half_pt: u32,
    /// `None` = auto / default colour (`\cf0` or no `\cf`).
    pub color: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl RtfDoc {
    /// Plain text: paragraphs joined with `'\n'`.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            for r in &p.runs {
                out.push_str(&r.text);
            }
        }
        out
    }

    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.paragraphs.iter().flat_map(|p| &p.runs)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message} (byte {offset})")]
pub struct RtfError {
    pub message: String,
    pub offset: usize,
}

// ---- tokenizer -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok<'a> {
    Open,
    Close,
    Word(&'a str, Option<i32>),
    /// Control symbol `\x` other than escapes handled below.
    Symbol(u8),
    Hex(u8),
    /// Literal text bytes (may include `\\`, `\{`, `\}` unescaped).
    Text(u8),
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn next(&mut self) -> Result<Option<(usize, Tok<'a>)>, RtfError> {
        loop {
            let start = self.pos;
            let Some(&c) = self.src.get(self.pos) else {
                return Ok(None);
            };
            self.pos += 1;
            let tok = match c {
                b'{' => Tok::Open,
                b'}' => Tok::Close,
                b'\r' | b'\n' => continue, // raw newlines are not content in RTF
                b'\\' => self.control(start)?,
                _ => Tok::Text(c),
            };
            return Ok(Some((start, tok)));
        }
    }

    fn control(&mut self, start: usize) -> Result<Tok<'a>, RtfError> {
        let Some(&c) = self.src.get(self.pos) else {
            return Err(RtfError {
                message: "backslash at end of text".into(),
                offset: start,
            });
        };
        if c.is_ascii_alphabetic() {
            let name_start = self.pos;
            while self
                .src
                .get(self.pos)
                .is_some_and(|b| b.is_ascii_alphabetic())
            {
                self.pos += 1;
            }
            let name = std::str::from_utf8(&self.src[name_start..self.pos]).unwrap_or("");
            let num_start = self.pos;
            if self.src.get(self.pos) == Some(&b'-') {
                self.pos += 1;
            }
            while self.src.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            let param = std::str::from_utf8(&self.src[num_start..self.pos])
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .map(|v| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
            if param.is_none() {
                self.pos = num_start; // a lone '-' is not a parameter
            }
            if self.src.get(self.pos) == Some(&b' ') {
                self.pos += 1; // delimiter space belongs to the control word
            }
            return Ok(Tok::Word(name, param));
        }
        self.pos += 1;
        Ok(match c {
            b'\\' | b'{' | b'}' => Tok::Text(c),
            b'\'' => {
                let hex = self
                    .src
                    .get(self.pos..self.pos + 2)
                    .ok_or_else(|| RtfError {
                        message: "truncated \\' hex escape".into(),
                        offset: start,
                    })?;
                let v = std::str::from_utf8(hex)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| RtfError {
                        message: "invalid \\' hex escape".into(),
                        offset: start,
                    })?;
                self.pos += 2;
                Tok::Hex(v)
            }
            // `\` + newline is a paragraph break (Cocoa RTF writes this).
            b'\n' | b'\r' => Tok::Word("par", None),
            other => Tok::Symbol(other),
        })
    }
}

// ---- parser ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Dest {
    Body,
    FontTable,
    ColorTable,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CharProps {
    font: i32,
    size: u32,
    color: i32,
    bold: bool,
    italic: bool,
    underline: bool,
}

#[derive(Debug, Clone, Copy)]
struct State {
    dest: Dest,
    chr: CharProps,
    uc: u32,
}

/// Destinations whose text is not slide content.
const SKIP_DESTINATIONS: &[&str] = &[
    "info",
    "stylesheet",
    "pict",
    "header",
    "headerl",
    "headerr",
    "footer",
    "footerl",
    "footerr",
    "fldinst",
    "object",
    "listtable",
    "listoverridetable",
    "revtbl",
    "rsidtbl",
    "xmlnstbl",
    "filetbl",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "generator",
    "panose",
    "falt",
];

/// Control words this reader acts on (not counted as unhandled).
const HANDLED: &[&str] = &[
    "rtf", "ansi", "ansicpg", "deff", "fonttbl", "colortbl", "f", "fcharset", "fs", "cf", "b", "i",
    "ul", "ulnone", "plain", "pard", "par", "line", "tab", "ql", "qc", "qr", "qj", "u", "uc",
    "red", "green", "blue", "fnil", "froman", "fswiss", "fmodern", "fscript", "fdecor", "ftech",
    "fbidi",
];

pub fn parse(src: &[u8]) -> Result<RtfDoc, RtfError> {
    let mut lx = Lexer { src, pos: 0 };
    let default_chr = CharProps {
        font: 0,
        size: DEFAULT_SIZE_HALF_PT,
        color: 0,
        bold: false,
        italic: false,
        underline: false,
    };
    let mut doc = RtfDoc::default();
    let mut stack: Vec<State> = Vec::new();
    let mut st = State {
        dest: Dest::Body,
        chr: default_chr,
        uc: 1,
    };
    let mut deff = 0;
    let mut para = Paragraph::default();
    let mut started = false;
    let mut ended = false;
    // Pending skip count after \uN.
    let mut skip = 0u32;
    // Bytes awaiting code-page decoding (hex escapes and raw 8-bit text).
    let mut bytes: Vec<u8> = Vec::new();
    // Font table entry being built.
    let mut font_entry: Option<FontEntry> = None;
    // Colour table entry being built.
    let mut color = (None::<u8>, None::<u8>, None::<u8>);
    // High surrogate from a preceding \uN.
    let mut high_surrogate: Option<u16> = None;
    // `\*` seen: the next destination word makes the group ignorable.
    let mut star = false;

    macro_rules! flush_bytes {
        () => {
            if !bytes.is_empty() {
                let text = decode_cp(&bytes, doc.code_page);
                bytes.clear();
                emit(&mut doc, &mut para, &st, &mut font_entry, &text);
            }
        };
    }

    while let Some((offset, tok)) = lx.next()? {
        if ended {
            // Anything after the outer group closes is ignored, but keep scanning for braces.
            if !matches!(tok, Tok::Text(b' ') | Tok::Text(0)) {
                doc.unhandled_words
                    .entry("<text after end>".into())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            }
            continue;
        }
        if !started {
            if tok != Tok::Open {
                return Err(RtfError {
                    message: "RTF must start with '{\\rtf'".into(),
                    offset,
                });
            }
            started = true;
            stack.push(st);
            continue;
        }
        // Pending 8-bit bytes must be decoded before anything else is emitted.
        let is_byte = matches!(tok, Tok::Hex(_)) || matches!(tok, Tok::Text(b) if b >= 0x80);
        if !is_byte {
            flush_bytes!();
        }
        // Characters skipped after \uN (RTF spec: each hex escape or char counts as one).
        if skip > 0 && matches!(tok, Tok::Hex(_) | Tok::Text(_) | Tok::Symbol(_)) {
            skip -= 1;
            continue;
        }
        skip = 0;
        match tok {
            Tok::Open => {
                stack.push(st);
                star = false;
            }
            Tok::Close => {
                star = false;
                if st.dest == Dest::FontTable {
                    finish_font(&mut doc, &mut font_entry);
                }
                match stack.pop() {
                    Some(prev) => st = prev,
                    None => {
                        return Err(RtfError {
                            message: "unbalanced '}'".into(),
                            offset,
                        });
                    }
                }
                if stack.is_empty() {
                    ended = true;
                }
            }
            Tok::Symbol(b'*') => star = true,
            Tok::Symbol(b'~') => emit(&mut doc, &mut para, &st, &mut font_entry, "\u{a0}"),
            Tok::Symbol(b'_') => emit(&mut doc, &mut para, &st, &mut font_entry, "\u{2011}"),
            Tok::Symbol(b'-') => {} // optional hyphen: invisible
            Tok::Symbol(s) => {
                *doc.unhandled_words
                    .entry(format!("\\{}", s as char))
                    .or_insert(0) += 1;
            }
            Tok::Hex(b) => {
                if st.dest == Dest::Body || st.dest == Dest::FontTable {
                    bytes.push(b);
                }
            }
            Tok::Text(b) => match st.dest {
                Dest::Skip => {}
                Dest::ColorTable => {
                    if b == b';' {
                        doc.colors.push(match color {
                            (Some(r), Some(g), Some(b)) => Some(Rgb { r, g, b }),
                            _ => None,
                        });
                        color = (None, None, None);
                    }
                }
                _ if b >= 0x80 => bytes.push(b),
                _ => emit(
                    &mut doc,
                    &mut para,
                    &st,
                    &mut font_entry,
                    &(b as char).to_string(),
                ),
            },
            Tok::Word(w, param) => {
                if star {
                    star = false;
                    if !HANDLED.contains(&w) {
                        st.dest = Dest::Skip;
                        continue;
                    }
                }
                if st.dest == Dest::Skip {
                    continue;
                }
                let on = param.is_none_or(|p| p != 0);
                match (w, st.dest) {
                    ("fonttbl", _) => st.dest = Dest::FontTable,
                    ("colortbl", _) => st.dest = Dest::ColorTable,
                    (w, _) if SKIP_DESTINATIONS.contains(&w) => st.dest = Dest::Skip,
                    ("rtf" | "ansi", _) => {}
                    ("ansicpg", _) => doc.code_page = param.and_then(|p| u16::try_from(p).ok()),
                    ("deff", _) => {
                        deff = param.unwrap_or(0);
                        st.chr.font = deff;
                    }
                    ("f", Dest::FontTable) => {
                        finish_font(&mut doc, &mut font_entry);
                        font_entry = Some(FontEntry {
                            index: param.unwrap_or(0),
                            name: String::new(),
                            charset: None,
                        });
                    }
                    ("fcharset", Dest::FontTable) => {
                        if let Some(f) = font_entry.as_mut() {
                            f.charset = param;
                        }
                    }
                    ("red", Dest::ColorTable) => color.0 = param.map(clamp_u8),
                    ("green", Dest::ColorTable) => color.1 = param.map(clamp_u8),
                    ("blue", Dest::ColorTable) => color.2 = param.map(clamp_u8),
                    ("uc", _) => st.uc = param.and_then(|p| u32::try_from(p).ok()).unwrap_or(1),
                    ("u", _) => {
                        let v = param.unwrap_or(0);
                        let unit = if v < 0 { (v + 65536) as u16 } else { v as u16 };
                        let ch = match (high_surrogate.take(), unit) {
                            (None, 0xD800..=0xDBFF) => {
                                high_surrogate = Some(unit);
                                None
                            }
                            (Some(hi), 0xDC00..=0xDFFF) => {
                                char::decode_utf16([hi, unit]).next().and_then(Result::ok)
                            }
                            (_, u) => char::from_u32(u32::from(u)),
                        };
                        if let Some(ch) = ch {
                            emit(&mut doc, &mut para, &st, &mut font_entry, &ch.to_string());
                        }
                        skip = st.uc;
                    }
                    (_, Dest::FontTable | Dest::ColorTable) => count(&mut doc, w),
                    ("f", _) => st.chr.font = param.unwrap_or(deff),
                    ("fs", _) => {
                        st.chr.size = param
                            .and_then(|p| u32::try_from(p).ok())
                            .unwrap_or(DEFAULT_SIZE_HALF_PT)
                    }
                    ("cf", _) => st.chr.color = param.unwrap_or(0),
                    ("b", _) => st.chr.bold = on,
                    ("i", _) => st.chr.italic = on,
                    ("ul", _) => st.chr.underline = on,
                    ("ulnone", _) => st.chr.underline = false,
                    ("plain", _) => {
                        st.chr = CharProps {
                            font: deff,
                            ..default_chr
                        }
                    }
                    ("pard", _) => para.align = Align::Left,
                    ("ql", _) => para.align = Align::Left,
                    ("qc", _) => para.align = Align::Center,
                    ("qr", _) => para.align = Align::Right,
                    ("qj", _) => para.align = Align::Justified,
                    ("par", _) => {
                        let align = para.align;
                        doc.paragraphs.push(std::mem::take(&mut para));
                        para.align = align; // paragraph props persist until \pard
                    }
                    ("line", _) => emit(&mut doc, &mut para, &st, &mut font_entry, "\n"),
                    ("tab", _) => emit(&mut doc, &mut para, &st, &mut font_entry, "\t"),
                    (w, _) => count(&mut doc, w),
                }
            }
        }
    }
    flush_bytes!();
    if !ended {
        return Err(RtfError {
            message: format!("text ends with {} unclosed group(s)", stack.len()),
            offset: src.len(),
        });
    }
    if !para.runs.is_empty() {
        doc.paragraphs.push(para);
    }
    Ok(doc)
}

fn count(doc: &mut RtfDoc, w: &str) {
    if HANDLED.contains(&w) {
        return;
    }
    *doc.unhandled_words.entry(w.to_owned()).or_insert(0) += 1;
}

fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn finish_font(doc: &mut RtfDoc, entry: &mut Option<FontEntry>) {
    if let Some(mut f) = entry.take() {
        f.name = f.name.trim().trim_end_matches(';').trim().to_owned();
        doc.fonts.push(f);
    }
}

/// Append text to the current destination.
fn emit(
    doc: &mut RtfDoc,
    para: &mut Paragraph,
    st: &State,
    font_entry: &mut Option<FontEntry>,
    text: &str,
) {
    match st.dest {
        Dest::FontTable => {
            if let Some(f) = font_entry.as_mut() {
                for ch in text.chars() {
                    if ch == ';' {
                        // Entry complete; next text belongs to the next entry.
                        f.name.push(';');
                    } else if !f.name.ends_with(';') {
                        f.name.push(ch);
                    }
                }
            }
        }
        Dest::Body => {
            let chr = st.chr;
            let font = doc
                .fonts
                .iter()
                .find(|f| f.index == chr.font)
                .map(|f| f.name.clone());
            let color = usize::try_from(chr.color)
                .ok()
                .filter(|&c| c > 0)
                .and_then(|c| doc.colors.get(c).copied().flatten());
            match para.runs.last_mut() {
                Some(r)
                    if r.font_index == chr.font
                        && r.size_half_pt == chr.size
                        && r.color == color
                        && r.bold == chr.bold
                        && r.italic == chr.italic
                        && r.underline == chr.underline =>
                {
                    r.text.push_str(text);
                }
                _ => para.runs.push(Run {
                    text: text.to_owned(),
                    font,
                    font_index: chr.font,
                    size_half_pt: chr.size,
                    color,
                    bold: chr.bold,
                    italic: chr.italic,
                    underline: chr.underline,
                }),
            }
        }
        Dest::ColorTable | Dest::Skip => {}
    }
}

/// Decode 8-bit text with the document code page (default Windows-1252).
fn decode_cp(bytes: &[u8], cp: Option<u16>) -> String {
    let enc = match cp.unwrap_or(1252) {
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        10000 => encoding_rs::MACINTOSH,
        65001 => encoding_rs::UTF_8,
        _ => encoding_rs::WINDOWS_1252,
    };
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

// ---- writer ------------------------------------------------------------------------

/// Serialize to Windows-style RTF (`{\fonttbl{\f0\fnil\fcharset0 Name;}}`).
///
/// Writes the font table, colour table (adding any run colour that's missing), and
/// each paragraph as `\pard\qX` + runs, emitting only the character properties that
/// change between runs. Text outside printable ASCII is written as `\uN?`.
/// What PP6 itself writes is unverified (see docs/PLAN.md Unknowns).
pub fn write(doc: &RtfDoc) -> String {
    use std::fmt::Write as _;

    let mut colors = doc.colors.clone();
    if colors.is_empty() {
        colors.push(None); // index 0 = auto
    }
    for run in doc.runs() {
        if let Some(c) = run.color
            && !colors.iter().skip(1).any(|x| *x == Some(c))
        {
            colors.push(Some(c));
        }
    }
    let color_index = |c: Option<Rgb>| -> usize {
        c.and_then(|c| {
            colors
                .iter()
                .skip(1)
                .position(|x| *x == Some(c))
                .map(|i| i + 1)
        })
        .unwrap_or(0)
    };

    let mut o = String::new();
    let deff = doc.fonts.first().map_or(0, |f| f.index);
    let _ = write!(
        o,
        "{{\\rtf1\\ansi\\ansicpg{}\\deff{deff}",
        doc.code_page.unwrap_or(1252)
    );
    o.push_str("{\\fonttbl");
    for f in &doc.fonts {
        let _ = write!(o, "{{\\f{}\\fnil", f.index);
        if let Some(cs) = f.charset {
            let _ = write!(o, "\\fcharset{cs}");
        }
        o.push(' ');
        escape_text(&mut o, &f.name);
        o.push_str(";}");
    }
    o.push('}');
    o.push_str("{\\colortbl;");
    for c in colors.iter().skip(1) {
        if let Some(c) = c {
            let _ = write!(o, "\\red{}\\green{}\\blue{}", c.r, c.g, c.b);
        }
        o.push(';');
    }
    o.push('}');

    let mut cur: Option<(i32, u32, usize, bool, bool, bool)> = None;
    for (i, p) in doc.paragraphs.iter().enumerate() {
        if i > 0 {
            o.push_str("\\par\n");
        }
        o.push_str(match p.align {
            Align::Left => "\\pard\\ql",
            Align::Center => "\\pard\\qc",
            Align::Right => "\\pard\\qr",
            Align::Justified => "\\pard\\qj",
        });
        for run in &p.runs {
            let next = (
                run.font_index,
                run.size_half_pt,
                color_index(run.color),
                run.bold,
                run.italic,
                run.underline,
            );
            let prev = cur.unwrap_or((i32::MIN, u32::MAX, usize::MAX, false, false, false));
            if prev.0 != next.0 {
                let _ = write!(o, "\\f{}", next.0);
            }
            if prev.1 != next.1 {
                let _ = write!(o, "\\fs{}", next.1);
            }
            if prev.2 != next.2 {
                let _ = write!(o, "\\cf{}", next.2);
            }
            if prev.3 != next.3 {
                o.push_str(if next.3 { "\\b" } else { "\\b0" });
            }
            if prev.4 != next.4 {
                o.push_str(if next.4 { "\\i" } else { "\\i0" });
            }
            if prev.5 != next.5 {
                o.push_str(if next.5 { "\\ul" } else { "\\ulnone" });
            }
            o.push(' ');
            cur = Some(next);
            escape_text(&mut o, &run.text);
        }
    }
    o.push('}');
    o
}

fn escape_text(o: &mut String, text: &str) {
    use std::fmt::Write as _;
    for ch in text.chars() {
        match ch {
            '\\' => o.push_str("\\\\"),
            '{' => o.push_str("\\{"),
            '}' => o.push_str("\\}"),
            '\n' => o.push_str("\\line "),
            '\t' => o.push_str("\\tab "),
            '\u{a0}' => o.push_str("\\~"),
            ' '..='~' => o.push(ch),
            _ => {
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    // RTF \u takes a signed 16-bit value; `?` is the fallback for old readers.
                    let _ = write!(o, "\\u{}?", *unit as i16);
                }
            }
        }
    }
}

// ---- in-place size scaling -----------------------------------------------------------

/// Control words whose numeric parameter is an absolute size (half-points for `\fs`,
/// twips or points for the rest) and must scale with the canvas.
pub const SIZE_WORDS: &[&str] = &[
    "fs", "sl", "sa", "sb", "li", "ri", "fi", "tx", "expnd", "expndtw", "up", "dn",
];

/// Change made by [`scale_sizes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeChange {
    pub word: String,
    pub old: i32,
    pub new: i32,
}

/// Multiply the parameter of every size control word by `factor`, editing the RTF
/// text in place so everything else (including control words this reader doesn't
/// model) is kept byte-for-byte. `\fs` never goes below 1. `\slmult` and other
/// unit-less words are untouched.
pub fn scale_sizes(src: &[u8], factor: f64) -> Result<(Vec<u8>, Vec<SizeChange>), RtfError> {
    let mut lx = Lexer { src, pos: 0 };
    let mut out = Vec::with_capacity(src.len() + 16);
    let mut copied = 0usize;
    let mut changes = Vec::new();
    while let Some((start, tok)) = lx.next()? {
        let Tok::Word(name, Some(old)) = tok else {
            continue;
        };
        if !SIZE_WORDS.contains(&name) {
            continue;
        }
        let mut new = (f64::from(old) * factor).round() as i32;
        if name == "fs" {
            new = new.max(1);
        }
        // Parameter text starts right after `\` + name.
        let p0 = start + 1 + name.len();
        let mut p1 = p0;
        if src.get(p1) == Some(&b'-') {
            p1 += 1;
        }
        while src.get(p1).is_some_and(u8::is_ascii_digit) {
            p1 += 1;
        }
        out.extend_from_slice(&src[copied..p0]);
        out.extend_from_slice(new.to_string().as_bytes());
        copied = p1;
        if new != old {
            changes.push(SizeChange {
                word: name.to_owned(),
                old,
                new,
            });
        }
    }
    out.extend_from_slice(&src[copied..]);
    Ok((out, changes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RtfDoc {
        parse(s.as_bytes()).unwrap()
    }

    const WIN: &str = r"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fnil\fcharset0 Arial;}{\f1\fnil\fcharset0 Futura PT;}}{\colortbl;\red255\green255\blue255;}\pard\qc\f0\fs120\cf1 Amazing grace\par how sweet \f1\fs96 the sound\par}";

    #[test]
    fn font_table_and_runs() {
        let d = p(WIN);
        assert_eq!(
            d.fonts.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["Arial", "Futura PT"]
        );
        assert_eq!(d.fonts[1].charset, Some(0));
        assert_eq!(
            d.colors,
            vec![
                None,
                Some(Rgb {
                    r: 255,
                    g: 255,
                    b: 255
                })
            ]
        );
        assert_eq!(d.code_page, Some(1252));
        assert_eq!(d.paragraphs.len(), 2);
        assert_eq!(d.paragraphs[0].align, Align::Center);
        assert_eq!(
            d.paragraphs[1].align,
            Align::Center,
            "alignment persists across \\par"
        );
        let runs: Vec<_> = d
            .runs()
            .map(|r| (r.text.as_str(), r.font.as_deref(), r.size_half_pt))
            .collect();
        assert_eq!(
            runs,
            [
                ("Amazing grace", Some("Arial"), 120),
                ("how sweet ", Some("Arial"), 120),
                ("the sound", Some("Futura PT"), 96)
            ]
        );
        assert_eq!(
            d.runs().next().unwrap().color,
            Some(Rgb {
                r: 255,
                g: 255,
                b: 255
            })
        );
        assert_eq!(d.plain_text(), "Amazing grace\nhow sweet the sound");
        assert!(d.unhandled_words.is_empty(), "{:?}", d.unhandled_words);
    }

    #[test]
    fn cocoa_style_fonttbl_and_backslash_newline() {
        let d = p(
            "{\\rtf1\\ansi\\ansicpg1252\\cocoartf1038\\cocoasubrtf360\n{\\fonttbl\\f0\\fswiss\\fcharset0 Helvetica;\\f1\\fnil Times;}\n{\\colortbl;\\red255\\green255\\blue255;}\n\\pard\\tx560\\pardirnatural\\qc\n\\f0\\fs120 \\cf1 Line one\\\nLine two}",
        );
        assert_eq!(
            d.fonts.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["Helvetica", "Times"]
        );
        assert_eq!(d.plain_text(), "Line one\nLine two");
        assert_eq!(d.paragraphs.len(), 2);
        assert_eq!(d.unhandled_words.get("cocoartf"), Some(&1));
        assert_eq!(d.unhandled_words.get("pardirnatural"), Some(&1));
    }

    #[test]
    fn unicode_hex_and_skip_counts() {
        let d = p(
            // `\x5c` is a backslash: keeps RTF unicode escapes literal in this source.
            "{\\rtf1\\ansi\\ansicpg1252{\\fonttbl{\\f0 Arial;}}\\f0\\fs100 Caf\\'e9 \x5cu8217?s {\\uc2\x5cu8220\\'93\\'94}x \\u-10179?\\u-8704? end\\line next}",
        );
        assert_eq!(
            d.plain_text(),
            "Café \u{2019}s \u{201c}x \u{1f600} end\nnext"
        );
    }

    #[test]
    fn ignorable_and_skipped_destinations() {
        let d = p(
            r"{\rtf1\ansi{\fonttbl{\f0 Arial{\*\panose 0000};}}{\*\expandedcolortbl;;}{\info{\title T}}{\*\generator Foo;}\f0\fs80 Hi\~there\{x\}}",
        );
        assert_eq!(d.plain_text(), "Hi\u{a0}there{x}");
        assert_eq!(d.fonts[0].name, "Arial");
    }

    #[test]
    fn bold_italic_toggle_and_plain() {
        let d = p(r"{\rtf1{\fonttbl{\f0 A;}{\f1 B;}}\f1\fs50\b bold\b0 {\i it}\plain plain}");
        let r: Vec<_> = d
            .runs()
            .map(|r| {
                (
                    r.text.as_str(),
                    r.bold,
                    r.italic,
                    r.font_index,
                    r.size_half_pt,
                )
            })
            .collect();
        assert_eq!(
            r,
            [
                ("bold", true, false, 1, 50),
                ("it", false, true, 1, 50),
                ("plain", false, false, 0, 24)
            ]
        );
    }

    #[test]
    fn empty_and_malformed() {
        assert_eq!(p(r"{\rtf1}").paragraphs.len(), 0);
        assert!(parse(br"{\rtf1 Hi").is_err(), "unclosed group");
        assert!(
            parse(br"{\rtf1 Hi}}").is_ok(),
            "trailing junk after end is tolerated"
        );
        assert!(parse(br"Hi").is_err(), "must start with a group");
        assert!(parse(br"{\rtf1 \'zz}").is_err(), "bad hex");
        assert!(parse(br"{\rtf1 \'4").is_err(), "truncated hex");
    }

    #[test]
    fn writer_round_trips_structure() {
        for src in [
            WIN,
            r"{\rtf1\ansi{\fonttbl{\f0 A;}{\f1 B;}}{\colortbl;\red1\green2\blue3;}\pard\qr\f1\fs50\b\cf1 bold\b0\i  it\par\pard\qj\ul under}",
        ] {
            let a = p(src);
            let written = write(&a);
            let b = parse(written.as_bytes()).unwrap();
            assert_eq!(b.fonts, a.fonts, "{written}");
            assert_eq!(b.colors, a.colors, "{written}");
            assert_eq!(b.paragraphs, a.paragraphs, "{written}");
            assert!(b.unhandled_words.is_empty(), "{:?}", b.unhandled_words);
        }
    }

    #[test]
    fn writer_escapes_text() {
        let text =
            "back\\slash {braces} caf\u{e9} \u{2019}quote\u{2019} \u{1f600}\ttab\u{a0}nbsp\nline";
        let doc = RtfDoc {
            fonts: vec![FontEntry {
                index: 0,
                name: "Caf\u{e9} Sans".into(),
                charset: Some(0),
            }],
            colors: vec![],
            paragraphs: vec![Paragraph {
                align: Align::Center,
                runs: vec![Run {
                    text: text.into(),
                    font: Some("Caf\u{e9} Sans".into()),
                    font_index: 0,
                    size_half_pt: 110,
                    color: Some(Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    }),
                    bold: false,
                    italic: false,
                    underline: false,
                }],
            }],
            code_page: None,
            unhandled_words: BTreeMap::new(),
        };
        let written = write(&doc);
        assert!(written.is_ascii(), "{written}");
        let back = parse(written.as_bytes()).unwrap();
        assert_eq!(back.plain_text(), text);
        assert_eq!(back.fonts[0].name, "Caf\u{e9} Sans");
        assert_eq!(
            back.colors,
            vec![
                None,
                Some(Rgb {
                    r: 255,
                    g: 255,
                    b: 255
                })
            ]
        );
        assert_eq!(
            back.runs().next().unwrap().color,
            Some(Rgb {
                r: 255,
                g: 255,
                b: 255
            })
        );
    }

    #[test]
    fn scale_sizes_in_place() {
        let src = "{\\rtf1\\ansi\\cocoartf1038{\\fonttbl\\f0 Arial;}\\pard\\sl-480\\slmult0\\li720\\f0\\fs96 Big\\fs1 tiny\\expnd-20 x}";
        let (out, changes) = scale_sizes(src.as_bytes(), 1.5).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\\rtf1\\ansi\\cocoartf1038{\\fonttbl\\f0 Arial;}\\pard\\sl-720\\slmult0\\li1080\\f0\\fs144 Big\\fs2 tiny\\expnd-30 x}"
        );
        let words: Vec<_> = changes
            .iter()
            .map(|c| (c.word.as_str(), c.old, c.new))
            .collect();
        assert_eq!(
            words,
            [
                ("sl", -480, -720),
                ("li", 720, 1080),
                ("fs", 96, 144),
                ("fs", 1, 2),
                ("expnd", -20, -30)
            ]
        );
        let (out, _) = scale_sizes(b"{\\rtf1\\fs3 a}", 0.1).unwrap();
        assert_eq!(out, b"{\\rtf1\\fs1 a}", "font size never reaches 0");
        let (same, none) = scale_sizes(WIN.as_bytes(), 1.0).unwrap();
        assert_eq!((same.as_slice(), none.len()), (WIN.as_bytes(), 0));
    }
}
