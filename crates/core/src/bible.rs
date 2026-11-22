//! Bible references for the scripture import: the 66 books with the verse count of
//! every chapter (ESV numbering), passages and their labels, the text the ESV API
//! returns, and splitting a passage into slides.
//!
//! The pickers only offer references in this table, so a request for a chapter or
//! verse that doesn't exist is never sent.

use std::cmp::Ordering;

pub struct Book {
    pub name: &'static str,
    /// Verses in each chapter (chapter 1 first).
    pub chapters: &'static [u16],
}

const fn book(name: &'static str, chapters: &'static [u16]) -> Book {
    Book { name, chapters }
}

/// The ESV API returns at most this many verses for one request.
pub const MAX_VERSES: usize = 500;

/// KJV versification with the ESV's differences (3 John has 15 verses, Revelation 12
/// has 18).
pub static BOOKS: [Book; 66] = [
    book(
        "Genesis",
        &[
            31, 25, 24, 26, 32, 22, 24, 22, 29, 32, 32, 20, 18, 24, 21, 16, 27, 33, 38, 18, 34, 24,
            20, 67, 34, 35, 46, 22, 35, 43, 55, 32, 20, 31, 29, 43, 36, 30, 23, 23, 57, 38, 34, 34,
            28, 34, 31, 22, 33, 26,
        ],
    ),
    book(
        "Exodus",
        &[
            22, 25, 22, 31, 23, 30, 25, 32, 35, 29, 10, 51, 22, 31, 27, 36, 16, 27, 25, 26, 36, 31,
            33, 18, 40, 37, 21, 43, 46, 38, 18, 35, 23, 35, 35, 38, 29, 31, 43, 38,
        ],
    ),
    book(
        "Leviticus",
        &[
            17, 16, 17, 35, 19, 30, 38, 36, 24, 20, 47, 8, 59, 57, 33, 34, 16, 30, 37, 27, 24, 33,
            44, 23, 55, 46, 34,
        ],
    ),
    book(
        "Numbers",
        &[
            54, 34, 51, 49, 31, 27, 89, 26, 23, 36, 35, 16, 33, 45, 41, 50, 13, 32, 22, 29, 35, 41,
            30, 25, 18, 65, 23, 31, 40, 16, 54, 42, 56, 29, 34, 13,
        ],
    ),
    book(
        "Deuteronomy",
        &[
            46, 37, 29, 49, 33, 25, 26, 20, 29, 22, 32, 32, 18, 29, 23, 22, 20, 22, 21, 20, 23, 30,
            25, 22, 19, 19, 26, 68, 29, 20, 30, 52, 29, 12,
        ],
    ),
    book(
        "Joshua",
        &[
            18, 24, 17, 24, 15, 27, 26, 35, 27, 43, 23, 24, 33, 15, 63, 10, 18, 28, 51, 9, 45, 34,
            16, 33,
        ],
    ),
    book(
        "Judges",
        &[
            36, 23, 31, 24, 31, 40, 25, 35, 57, 18, 40, 15, 25, 20, 20, 31, 13, 31, 30, 48, 25,
        ],
    ),
    book("Ruth", &[22, 23, 18, 22]),
    book(
        "1 Samuel",
        &[
            28, 36, 21, 22, 12, 21, 17, 22, 27, 27, 15, 25, 23, 52, 35, 23, 58, 30, 24, 42, 15, 23,
            29, 22, 44, 25, 12, 25, 11, 31, 13,
        ],
    ),
    book(
        "2 Samuel",
        &[
            27, 32, 39, 12, 25, 23, 29, 18, 13, 19, 27, 31, 39, 33, 37, 23, 29, 33, 43, 26, 22, 51,
            39, 25,
        ],
    ),
    book(
        "1 Kings",
        &[
            53, 46, 28, 34, 18, 38, 51, 66, 28, 29, 43, 33, 34, 31, 34, 34, 24, 46, 21, 43, 29, 53,
        ],
    ),
    book(
        "2 Kings",
        &[
            18, 25, 27, 44, 27, 33, 20, 29, 37, 36, 21, 21, 25, 29, 38, 20, 41, 37, 37, 21, 26, 20,
            37, 20, 30,
        ],
    ),
    book(
        "1 Chronicles",
        &[
            54, 55, 24, 43, 26, 81, 40, 40, 44, 14, 47, 40, 14, 17, 29, 43, 27, 17, 19, 8, 30, 19,
            32, 31, 31, 32, 34, 21, 30,
        ],
    ),
    book(
        "2 Chronicles",
        &[
            17, 18, 17, 22, 14, 42, 22, 18, 31, 19, 23, 16, 22, 15, 19, 14, 19, 34, 11, 37, 20, 12,
            21, 27, 28, 23, 9, 27, 36, 27, 21, 33, 25, 33, 27, 23,
        ],
    ),
    book("Ezra", &[11, 70, 13, 24, 17, 22, 28, 36, 15, 44]),
    book(
        "Nehemiah",
        &[11, 20, 32, 23, 19, 19, 73, 18, 38, 39, 36, 47, 31],
    ),
    book("Esther", &[22, 23, 15, 17, 14, 14, 10, 17, 32, 3]),
    book(
        "Job",
        &[
            22, 13, 26, 21, 27, 30, 21, 22, 35, 22, 20, 25, 28, 22, 35, 22, 16, 21, 29, 29, 34, 30,
            17, 25, 6, 14, 23, 28, 25, 31, 40, 22, 33, 37, 16, 33, 24, 41, 30, 24, 34, 17,
        ],
    ),
    book(
        "Psalms",
        &[
            6, 12, 8, 8, 12, 10, 17, 9, 20, 18, 7, 8, 6, 7, 5, 11, 15, 50, 14, 9, 13, 31, 6, 10,
            22, 12, 14, 9, 11, 12, 24, 11, 22, 22, 28, 12, 40, 22, 13, 17, 13, 11, 5, 26, 17, 11,
            9, 14, 20, 23, 19, 9, 6, 7, 23, 13, 11, 11, 17, 12, 8, 12, 11, 10, 13, 20, 7, 35, 36,
            5, 24, 20, 28, 23, 10, 12, 20, 72, 13, 19, 16, 8, 18, 12, 13, 17, 7, 18, 52, 17, 16,
            15, 5, 23, 11, 13, 12, 9, 9, 5, 8, 28, 22, 35, 45, 48, 43, 13, 31, 7, 10, 10, 9, 8, 18,
            19, 2, 29, 176, 7, 8, 9, 4, 8, 5, 6, 5, 6, 8, 8, 3, 18, 3, 3, 21, 26, 9, 8, 24, 13, 10,
            7, 12, 15, 21, 10, 20, 14, 9, 6,
        ],
    ),
    book(
        "Proverbs",
        &[
            33, 22, 35, 27, 23, 35, 27, 36, 18, 32, 31, 28, 25, 35, 33, 33, 28, 24, 29, 30, 31, 29,
            35, 34, 28, 28, 27, 28, 27, 33, 31,
        ],
    ),
    book(
        "Ecclesiastes",
        &[18, 26, 22, 16, 20, 12, 29, 17, 18, 20, 10, 14],
    ),
    book("Song of Solomon", &[17, 17, 11, 16, 16, 13, 13, 14]),
    book(
        "Isaiah",
        &[
            31, 22, 26, 6, 30, 13, 25, 22, 21, 34, 16, 6, 22, 32, 9, 14, 14, 7, 25, 6, 17, 25, 18,
            23, 12, 21, 13, 29, 24, 33, 9, 20, 24, 17, 10, 22, 38, 22, 8, 31, 29, 25, 28, 28, 25,
            13, 15, 22, 26, 11, 23, 15, 12, 17, 13, 12, 21, 14, 21, 22, 11, 12, 19, 12, 25, 24,
        ],
    ),
    book(
        "Jeremiah",
        &[
            19, 37, 25, 31, 31, 30, 34, 22, 26, 25, 23, 17, 27, 22, 21, 21, 27, 23, 15, 18, 14, 30,
            40, 10, 38, 24, 22, 17, 32, 24, 40, 44, 26, 22, 19, 32, 21, 28, 18, 16, 18, 22, 13, 30,
            5, 28, 7, 47, 39, 46, 64, 34,
        ],
    ),
    book("Lamentations", &[22, 22, 66, 22, 22]),
    book(
        "Ezekiel",
        &[
            28, 10, 27, 17, 17, 14, 27, 18, 11, 22, 25, 28, 23, 23, 8, 63, 24, 32, 14, 49, 32, 31,
            49, 27, 17, 21, 36, 26, 21, 26, 18, 32, 33, 31, 15, 38, 28, 23, 29, 49, 26, 20, 27, 31,
            25, 24, 23, 35,
        ],
    ),
    book("Daniel", &[21, 49, 30, 37, 31, 28, 28, 27, 27, 21, 45, 13]),
    book(
        "Hosea",
        &[11, 23, 5, 19, 15, 11, 16, 14, 17, 15, 12, 14, 16, 9],
    ),
    book("Joel", &[20, 32, 21]),
    book("Amos", &[15, 16, 15, 13, 27, 14, 17, 14, 15]),
    book("Obadiah", &[21]),
    book("Jonah", &[17, 10, 10, 11]),
    book("Micah", &[16, 13, 12, 13, 15, 16, 20]),
    book("Nahum", &[15, 13, 19]),
    book("Habakkuk", &[17, 20, 19]),
    book("Zephaniah", &[18, 15, 20]),
    book("Haggai", &[15, 23]),
    book(
        "Zechariah",
        &[21, 13, 10, 14, 11, 15, 14, 23, 17, 12, 17, 14, 9, 21],
    ),
    book("Malachi", &[14, 17, 18, 6]),
    book(
        "Matthew",
        &[
            25, 23, 17, 25, 48, 34, 29, 34, 38, 42, 30, 50, 58, 36, 39, 28, 27, 35, 30, 34, 46, 46,
            39, 51, 46, 75, 66, 20,
        ],
    ),
    book(
        "Mark",
        &[
            45, 28, 35, 41, 43, 56, 37, 38, 50, 52, 33, 44, 37, 72, 47, 20,
        ],
    ),
    book(
        "Luke",
        &[
            80, 52, 38, 44, 39, 49, 50, 56, 62, 42, 54, 59, 35, 35, 32, 31, 37, 43, 48, 47, 38, 71,
            56, 53,
        ],
    ),
    book(
        "John",
        &[
            51, 25, 36, 54, 47, 71, 53, 59, 41, 42, 57, 50, 38, 31, 27, 33, 26, 40, 42, 31, 25,
        ],
    ),
    book(
        "Acts",
        &[
            26, 47, 26, 37, 42, 15, 60, 40, 43, 48, 30, 25, 52, 28, 41, 40, 34, 28, 41, 38, 40, 30,
            35, 27, 27, 32, 44, 31,
        ],
    ),
    book(
        "Romans",
        &[
            32, 29, 31, 25, 21, 23, 25, 39, 33, 21, 36, 21, 14, 23, 33, 27,
        ],
    ),
    book(
        "1 Corinthians",
        &[
            31, 16, 23, 21, 13, 20, 40, 13, 27, 33, 34, 31, 13, 40, 58, 24,
        ],
    ),
    book(
        "2 Corinthians",
        &[24, 17, 18, 18, 21, 18, 16, 24, 15, 18, 33, 21, 14],
    ),
    book("Galatians", &[24, 21, 29, 31, 26, 18]),
    book("Ephesians", &[23, 22, 21, 32, 33, 24]),
    book("Philippians", &[30, 30, 21, 23]),
    book("Colossians", &[29, 23, 25, 18]),
    book("1 Thessalonians", &[10, 20, 13, 18, 28]),
    book("2 Thessalonians", &[12, 17, 18]),
    book("1 Timothy", &[20, 15, 16, 16, 25, 21]),
    book("2 Timothy", &[18, 26, 17, 22]),
    book("Titus", &[16, 15, 15]),
    book("Philemon", &[25]),
    book(
        "Hebrews",
        &[14, 18, 19, 16, 14, 20, 28, 13, 28, 39, 40, 29, 25],
    ),
    book("James", &[27, 26, 18, 17, 20]),
    book("1 Peter", &[25, 25, 22, 19, 14]),
    book("2 Peter", &[21, 22, 18]),
    book("1 John", &[10, 29, 24, 21, 21]),
    book("2 John", &[13]),
    book("3 John", &[15]),
    book("Jude", &[25]),
    book(
        "Revelation",
        &[
            20, 29, 22, 11, 14, 17, 17, 13, 21, 11, 19, 18, 18, 20, 8, 21, 18, 24, 21, 15, 27, 21,
        ],
    ),
];

impl Book {
    /// Verses in `chapter` (1-based); 0 if there's no such chapter.
    pub fn verses(&self, chapter: u16) -> u16 {
        usize::from(chapter)
            .checked_sub(1)
            .and_then(|i| self.chapters.get(i))
            .copied()
            .unwrap_or(0)
    }

    pub fn chapter_count(&self) -> u16 {
        u16::try_from(self.chapters.len()).unwrap_or(u16::MAX)
    }

    /// Obadiah, Philemon, 2 and 3 John, Jude: references leave out the chapter.
    pub fn single_chapter(&self) -> bool {
        self.chapters.len() == 1
    }

    pub fn last(&self) -> VerseRef {
        let chapter = self.chapter_count();
        VerseRef {
            chapter,
            verse: self.verses(chapter),
        }
    }

    pub fn contains(&self, v: VerseRef) -> bool {
        v.verse >= 1 && v.verse <= self.verses(v.chapter)
    }

    /// The name in a reference to one chapter ("Psalm 23", but "Psalms 1–2").
    fn name_for(&self, one_chapter: bool) -> &'static str {
        if one_chapter && self.name == "Psalms" {
            "Psalm"
        } else {
            self.name
        }
    }
}

/// A verse within a book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VerseRef {
    pub chapter: u16,
    pub verse: u16,
}

impl VerseRef {
    pub fn new(chapter: u16, verse: u16) -> VerseRef {
        VerseRef { chapter, verse }
    }
}

impl PartialOrd for VerseRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VerseRef {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.chapter, self.verse).cmp(&(other.chapter, other.verse))
    }
}

/// Verses `from` to `to` (inclusive) of a book, possibly across chapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Passage {
    /// Index into [`BOOKS`].
    pub book: usize,
    pub from: VerseRef,
    pub to: VerseRef,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PassageError {
    #[error("There's no such book.")]
    NoBook,
    #[error("{book} has no verse {chapter}:{verse}.")]
    NoVerse {
        book: &'static str,
        chapter: u16,
        verse: u16,
    },
    #[error("The passage ends before it starts.")]
    Backwards,
    #[error(
        "That's {0} verses; the ESV API gives at most 500 at a time. Choose a shorter passage."
    )]
    TooLong(usize),
}

impl Passage {
    /// A passage that exists in the table, in order and not too long for one request.
    pub fn new(book: usize, from: VerseRef, to: VerseRef) -> Result<Passage, PassageError> {
        let b = BOOKS.get(book).ok_or(PassageError::NoBook)?;
        for v in [from, to] {
            if !b.contains(v) {
                return Err(PassageError::NoVerse {
                    book: b.name,
                    chapter: v.chapter,
                    verse: v.verse,
                });
            }
        }
        if to < from {
            return Err(PassageError::Backwards);
        }
        let p = Passage { book, from, to };
        match p.verse_count() {
            n if n > MAX_VERSES => Err(PassageError::TooLong(n)),
            _ => Ok(p),
        }
    }

    fn book_ref(&self) -> &'static Book {
        &BOOKS[self.book.min(BOOKS.len() - 1)]
    }

    pub fn verse_count(&self) -> usize {
        let b = self.book_ref();
        if self.from.chapter == self.to.chapter {
            return usize::from(self.to.verse.saturating_sub(self.from.verse)) + 1;
        }
        let first = usize::from(b.verses(self.from.chapter).saturating_sub(self.from.verse)) + 1;
        let middle: usize = (self.from.chapter + 1..self.to.chapter)
            .map(|c| usize::from(b.verses(c)))
            .sum();
        first + middle + usize::from(self.to.verse)
    }

    /// How the reference is written on slides: "John 3:16", "John 3:16–21",
    /// "John 3:16–4:3", "Psalm 23", "Psalms 1–2", "Jude 3–5".
    pub fn label(&self) -> String {
        let b = self.book_ref();
        let (f, t) = (self.from, self.to);
        if b.single_chapter() {
            return if f.verse == t.verse {
                format!("{} {}", b.name, f.verse)
            } else {
                format!("{} {}\u{2013}{}", b.name, f.verse, t.verse)
            };
        }
        let whole = f.verse == 1 && t.verse == b.verses(t.chapter);
        if f.chapter == t.chapter {
            let name = b.name_for(true);
            if whole {
                format!("{name} {}", f.chapter)
            } else if f.verse == t.verse {
                format!("{name} {}:{}", f.chapter, f.verse)
            } else {
                format!("{name} {}:{}\u{2013}{}", f.chapter, f.verse, t.verse)
            }
        } else if whole {
            format!("{} {}\u{2013}{}", b.name, f.chapter, t.chapter)
        } else {
            format!(
                "{} {}:{}\u{2013}{}:{}",
                b.name, f.chapter, f.verse, t.chapter, t.verse
            )
        }
    }

    /// The reference as the ESV API's `q` parameter ("John 3:16-4:3").
    pub fn query(&self) -> String {
        let (b, f, t) = (self.book_ref(), self.from, self.to);
        if f.chapter == t.chapter {
            format!("{} {}:{}-{}", b.name, f.chapter, f.verse, t.verse)
        } else {
            format!(
                "{} {}:{}-{}:{}",
                b.name, f.chapter, f.verse, t.chapter, t.verse
            )
        }
    }
}

/// The verses in ESV API text fetched with verse numbers on (`[16] For God so…`, and
/// `[4:1]` style markers): each verse's text on one line, in order. Anything before
/// the first number (a heading) is dropped, and so are verses with no text (the ESV
/// leaves out a few, like Matthew 17:21).
pub fn esv_verses(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let marker = after.find(']').filter(|&close| {
            let inner = &after[..close];
            !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == ':')
        });
        match marker {
            Some(close) => {
                if let Some(c) = cur.as_mut() {
                    c.push_str(&rest[..open]);
                }
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                cur = Some(String::new());
                rest = &after[close + 1..];
            }
            None => {
                if let Some(c) = cur.as_mut() {
                    c.push_str(&rest[..=open]);
                }
                rest = after;
            }
        }
    }
    if let Some(mut c) = cur {
        c.push_str(rest);
        out.push(c);
    }
    out.into_iter()
        .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|v| !v.is_empty())
        .collect()
}

/// Split verses into slide texts: whole verses together while `fits` says the text
/// fits on a slide. A verse too long by itself is split at sentence breaks (. ; : ! ?),
/// then commas and dashes, then between words.
pub fn slides(verses: &[String], mut fits: impl FnMut(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for v in verses {
        let joined = join(&cur, v);
        if fits(&joined) {
            cur = joined;
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if fits(v) {
            cur.clone_from(v);
        } else {
            out.extend(split_long(v, &mut fits, 0));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A slide check without fonts: at most `max` words (about what fits at the house
/// size, Bookman Old Style 54 on 1024×768).
pub fn fits_words(max: usize) -> impl FnMut(&str) -> bool {
    move |t: &str| t.split_whitespace().count() <= max
}

fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_owned()
    } else {
        format!("{a} {b}")
    }
}

fn split_long(text: &str, fits: &mut impl FnMut(&str) -> bool, level: usize) -> Vec<String> {
    let parts: Vec<String> = match level {
        0 => clauses(text, &['.', ';', ':', '!', '?']),
        1 => clauses(text, &[',', '\u{2014}', '\u{2013}']),
        _ => text.split_whitespace().map(str::to_owned).collect(),
    };
    let mut out = Vec::new();
    let mut cur = String::new();
    for p in parts {
        let joined = join(&cur, &p);
        if fits(&joined) {
            cur = joined;
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if fits(&p) || level >= 2 {
            cur = p;
        } else {
            out.extend(split_long(&p, fits, level + 1));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `text` cut after each of `marks` (and any closing quotes or brackets after it)
/// that's followed by a space; a dash also cuts with no space after it.
fn clauses(text: &str, marks: &[char]) -> Vec<String> {
    let closing = ['\u{2019}', '\u{201D}', '"', '\'', ')', ']'];
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        cur.push(c);
        i += 1;
        if marks.contains(&c) {
            while i < chars.len() && closing.contains(&chars[i]) {
                cur.push(chars[i]);
                i += 1;
            }
            let dash = c == '\u{2014}' || c == '\u{2013}';
            if dash || chars.get(i).is_some_and(|n| n.is_whitespace()) {
                let t = cur.trim();
                if !t.is_empty() {
                    out.push(t.to_owned());
                }
                cur.clear();
            }
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> usize {
        BOOKS.iter().position(|b| b.name == name).unwrap()
    }

    #[test]
    fn the_table_has_every_chapter_and_verse() {
        assert_eq!(BOOKS.len(), 66);
        let chapters: usize = BOOKS.iter().map(|b| b.chapters.len()).sum();
        assert_eq!(chapters, 1189);
        let verses: usize = BOOKS
            .iter()
            .flat_map(|b| b.chapters)
            .map(|&v| usize::from(v))
            .sum();
        assert_eq!(
            verses, 31_104,
            "KJV 31,102 + 3 John 1:15 + Revelation 12:18"
        );
        let ps = &BOOKS[find("Psalms")];
        assert_eq!(ps.chapter_count(), 150);
        assert_eq!(ps.verses(119), 176);
        assert_eq!(BOOKS[find("Genesis")].verses(120), 0);
        assert_eq!(BOOKS[find("John")].verses(3), 36);
    }

    #[test]
    fn passages_must_exist() {
        let genesis = find("Genesis");
        assert!(matches!(
            Passage::new(genesis, VerseRef::new(120, 1), VerseRef::new(120, 1)),
            Err(PassageError::NoVerse { .. })
        ));
        assert_eq!(
            Passage::new(genesis, VerseRef::new(2, 1), VerseRef::new(1, 5)),
            Err(PassageError::Backwards)
        );
        assert!(matches!(
            Passage::new(genesis, VerseRef::new(1, 1), VerseRef::new(50, 26)),
            Err(PassageError::TooLong(1533))
        ));
        let john = find("John");
        let p = Passage::new(john, VerseRef::new(3, 16), VerseRef::new(4, 3)).unwrap();
        assert_eq!(p.verse_count(), 21 + 3);
    }

    #[test]
    fn labels_and_queries() {
        let p = |b: &str, f: (u16, u16), t: (u16, u16)| {
            Passage::new(find(b), VerseRef::new(f.0, f.1), VerseRef::new(t.0, t.1)).unwrap()
        };
        assert_eq!(p("John", (3, 16), (3, 16)).label(), "John 3:16");
        assert_eq!(p("John", (3, 16), (3, 21)).label(), "John 3:16–21");
        assert_eq!(p("John", (3, 16), (4, 3)).label(), "John 3:16–4:3");
        assert_eq!(p("Psalms", (23, 1), (23, 6)).label(), "Psalm 23");
        assert_eq!(p("Psalms", (23, 1), (23, 3)).label(), "Psalm 23:1–3");
        assert_eq!(p("Psalms", (1, 1), (2, 12)).label(), "Psalms 1–2");
        assert_eq!(p("Jude", (1, 3), (1, 5)).label(), "Jude 3–5");
        assert_eq!(p("John", (3, 16), (4, 3)).query(), "John 3:16-4:3");
        assert_eq!(p("John", (3, 16), (3, 21)).query(), "John 3:16-21");
    }

    #[test]
    fn esv_text_becomes_verses() {
        let text = "The Heading\n\n  [16] “For God so loved the world,\n    that he gave [his] only Son. [17] For God did not send\n\n [18] [4:1] Now when Jesus learned (ESV)";
        let v = esv_verses(text);
        assert_eq!(
            v,
            [
                "“For God so loved the world, that he gave [his] only Son.",
                "For God did not send",
                "Now when Jesus learned (ESV)",
            ]
        );
    }

    #[test]
    fn verses_share_slides_and_long_ones_split() {
        let verses: Vec<String> = ["one two three", "four five", "six seven eight nine"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            slides(&verses, fits_words(5)),
            ["one two three four five", "six seven eight nine"]
        );
        let long = vec!["Alpha beta, gamma delta. Epsilon zeta eta theta; iota kappa.".to_owned()];
        assert_eq!(
            slides(&long, fits_words(4)),
            [
                "Alpha beta, gamma delta.",
                "Epsilon zeta eta theta;",
                "iota kappa."
            ]
        );
        let commas = vec!["a b c, d e f, g h i j k l".to_owned()];
        assert_eq!(
            slides(&commas, fits_words(4)),
            ["a b c,", "d e f,", "g h i j", "k l"]
        );
        assert_eq!(
            clauses("He said, “Go.” Then they went.", &['.']),
            ["He said, “Go.”", "Then they went."]
        );
    }
}
