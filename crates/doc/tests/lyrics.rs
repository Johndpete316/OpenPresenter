#![allow(clippy::unwrap_used, clippy::expect_used)]
//! E8 acceptance: pasting a song with verses, chorus and repeats gives the right
//! groups, slides and arrangement, and round-trips through the text view.

use std::path::PathBuf;

use doc::lyrics::{LyricsMode, to_text};
use doc::{Document, Edit};
use pretty_assertions::assert_eq;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

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
And grace my fears relieved

[Chorus]

[Bridge]
Through many dangers toils and snares

[Chorus]
";

fn names(d: &Document) -> Vec<&str> {
    d.groups.iter().map(|g| g.name.as_str()).collect()
}

#[test]
fn paste_a_song_and_round_trip_it_through_the_text_view() {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let style = d
        .text_boxes()
        .next()
        .unwrap()
        .runs()
        .next()
        .unwrap()
        .clone();
    let op = Edit::lyrics(SONG, LyricsMode::Replace);
    assert_eq!(doc::check(&d, &op), doc::Check::Ok, ".pro6-safe");
    op.apply(&mut d).unwrap();
    assert_eq!(names(&d), ["Verse 1", "Chorus", "Verse 2", "Bridge"]);
    let counts: Vec<usize> = d.groups.iter().map(|g| g.slides.len()).collect();
    assert_eq!(counts, [2, 1, 1, 1]);
    let order: Vec<&str> = d
        .selected_arrangement()
        .unwrap()
        .groups
        .iter()
        .map(|&g| d.groups[g].name.as_str())
        .collect();
    assert_eq!(
        order,
        ["Verse 1", "Chorus", "Verse 2", "Chorus", "Bridge", "Chorus"]
    );
    // New slides take the presentation's look.
    let r = d.groups[3].slides[0]
        .text_boxes()
        .next()
        .unwrap()
        .runs()
        .next()
        .unwrap()
        .clone();
    assert_eq!((r.font.as_str(), r.size), (style.font.as_str(), style.size));
    // The text view shows exactly what was pasted.
    assert_eq!(to_text(&d), SONG);
    // Applying the text view unchanged changes nothing.
    let before = d.clone();
    Edit::lyrics(&to_text(&d), LyricsMode::Replace)
        .apply(&mut d)
        .unwrap();
    assert_eq!(d, before);
    // And the result saves to .pro6 and reads back the same.
    let back = doc::from_pro6(doc::to_pro6(&d).unwrap());
    assert_eq!(to_text(&back), SONG);
}

#[test]
fn reflowing_an_existing_presentation_keeps_untouched_slides_byte_identical() {
    let d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let text = to_text(&d);
    let mut same = d.clone();
    Edit::lyrics(&text, LyricsMode::Replace)
        .apply(&mut same)
        .unwrap();
    let a = pro6::write_string(&doc::to_pro6(&d).unwrap());
    let b = pro6::write_string(&doc::to_pro6(&same).unwrap());
    if d.slides()
        .all(|s| s.text_boxes().any(|t| !t.plain_text().trim().is_empty()))
    {
        assert_eq!(a, b, "unchanged text: unchanged file");
    }
    // One changed line: only that slide's text changes.
    let first_line = text.lines().nth(1).unwrap().to_owned();
    let edited = text.replacen(&first_line, "A brand new first line", 1);
    let mut changed = d.clone();
    Edit::lyrics(&edited, LyricsMode::Replace)
        .apply(&mut changed)
        .unwrap();
    assert_eq!(names(&changed), names(&d));
    assert_eq!(
        changed.groups[0].slides[0].uuid, d.groups[0].slides[0].uuid,
        "same slide"
    );
    assert!(to_text(&changed).contains("A brand new first line"));
}

#[test]
fn append_adds_new_groups_and_reuses_known_names() {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let before = d.groups.len();
    let existing = d.groups[0].name.clone();
    let text = format!("[{existing}]\n\n[Tag]\nOne more time\n");
    Edit::lyrics(&text, LyricsMode::Append)
        .apply(&mut d)
        .unwrap();
    assert_eq!(
        d.groups.len(),
        before + 1,
        "the known name isn't duplicated"
    );
    assert_eq!(d.groups.last().unwrap().name, "Tag");
}

#[test]
fn default_colours_apply_to_new_groups() {
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let gold = doc::Rgba {
        r: 1.0,
        g: 0.8,
        b: 0.0,
        a: 1.0,
    };
    Edit::SetLyrics {
        text: "[Bridge 1]\nX\n".into(),
        mode: LyricsMode::Append,
        colors: vec![("bridge".into(), gold)],
        theme: None,
    }
    .apply(&mut d)
    .unwrap();
    assert_eq!(d.groups.last().unwrap().color, Some(gold));
}
