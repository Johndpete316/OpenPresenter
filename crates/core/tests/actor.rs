#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Drive the core actor with commands and check the snapshots.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use opcore::{Command, CoreConfig, ItemRef, ShowState};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn wait(
    rx: &Receiver<Arc<ShowState>>,
    what: &str,
    pred: impl Fn(&ShowState) -> bool,
) -> Arc<ShowState> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let s = rx
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        if pred(&s) {
            return s;
        }
    }
}

/// Poll the disk (written by the core's I/O thread) for up to 10 s.
fn wait_for_disk(what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn tomls(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect()
        })
        .unwrap_or_default()
}

fn start(config: CoreConfig) -> (opcore::CoreHandle, Receiver<Arc<ShowState>>) {
    let (core, _join) = opcore::spawn(config).unwrap();
    let rx = core.subscribe(None);
    (core, rx)
}

fn r(playlist: usize, item: usize) -> ItemRef {
    ItemRef { playlist, item }
}

fn pos(s: &ShowState) -> Option<usize> {
    s.live.as_ref().and_then(|l| l.position)
}

#[test]
fn navigation_skips_disabled_and_follows_arrangement() {
    let song = fixture("song_1024x768.pro6");
    let (core, rx) = start(CoreConfig {
        output_enabled: true,
        ..Default::default()
    });
    core.send(Command::Open(song.clone()));
    let s = wait(&rx, "song loaded", |s| s.docs.contains_key(&song));
    // Default arrangement (from selectedArrangementID): V1 C V2 C = 2+2+1+2 positions.
    assert_eq!(
        s.arrangement_for(&song).as_deref(),
        Some("AA000000-0001-4000-8000-000000000001")
    );
    let order = s.order_for(&song).unwrap();
    assert_eq!(order.len(), 7);
    assert_eq!(
        order.iter().map(|r| r.enabled).collect::<Vec<_>>(),
        [true, true, true, false, true, true, false]
    );

    // Next with nothing live starts the selected presentation.
    core.send(Command::Next);
    let s = wait(&rx, "live", |s| s.live.is_some());
    assert_eq!(pos(&s), Some(0));

    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 2,
    });
    wait(&rx, "pos 2", |s| pos(s) == Some(2));
    core.send(Command::Next);
    let s = wait(&rx, "skip disabled", |s| pos(s) == Some(4));
    assert_eq!(s.live.as_ref().unwrap().slide.group, 2, "Verse 2");
    core.send(Command::Prev);
    wait(&rx, "prev skips disabled", |s| pos(s) == Some(2));

    // Disabled slides can still be shown directly.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 3,
    });
    let s = wait(&rx, "disabled goto", |s| pos(s) == Some(3));
    assert!(!s.live.as_ref().unwrap().slide.enabled);

    // At the end, Next stays put.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 5,
    });
    wait(&rx, "pos 5", |s| pos(s) == Some(5));
    core.send(Command::Next);
    core.send(Command::ClearText); // marker so we can see Next was processed
    let s = wait(&rx, "end", |s| !s.text_visible);
    assert_eq!(pos(&s), Some(5));

    // Switch arrangement live: V2 isn't in "Short" → slide stays, position detaches.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 4,
    });
    wait(&rx, "V2", |s| pos(s) == Some(4) && s.text_visible);
    core.send(Command::SelectArrangement {
        presentation: song.clone(),
        arrangement: Some("AA000000-0001-4000-8000-000000000002".into()),
    });
    let s = wait(&rx, "short", |s| {
        s.live.as_ref().is_some_and(|l| l.order.len() == 4)
    });
    let live = s.live.as_ref().unwrap();
    assert_eq!(
        (live.position, live.slide.group),
        (None, 2),
        "still showing Verse 2"
    );
    core.send(Command::Next);
    wait(&rx, "first of short", |s| pos(s) == Some(0));

    // Switch to natural order while on a slide that exists in both: position maps.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 2,
    }); // Chorus 1 in Short
    wait(&rx, "chorus", |s| {
        pos(s) == Some(2) && s.live.as_ref().unwrap().order.len() == 4
    });
    core.send(Command::SelectArrangement {
        presentation: song.clone(),
        arrangement: None,
    });
    let s = wait(&rx, "natural", |s| {
        s.live.as_ref().is_some_and(|l| l.order.len() == 5)
    });
    assert_eq!(pos(&s), Some(2));
}

#[test]
fn clear_layers_and_text_returns_on_next() {
    let song = fixture("song_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    wait(&rx, "live after load", |s| pos(s) == Some(0));
    core.send(Command::SetBackground {
        media: Some("/m/bg.mp4".into()),
        next: None,
    });
    wait(&rx, "bg", |s| s.background_media.is_some());
    core.send(Command::ClearBackground);
    wait(&rx, "bg cleared", |s| {
        s.background_media.is_none() && s.text_visible
    });
    core.send(Command::SetBackground {
        media: Some("/m/bg.mp4".into()),
        next: None,
    });
    wait(&rx, "bg again", |s| s.background_media.is_some());
    core.send(Command::ClearAll);
    wait(&rx, "all cleared", |s| {
        s.background_media.is_none() && !s.text_visible
    });
    core.send(Command::Next);
    let s = wait(&rx, "next", |s| pos(s) == Some(1));
    assert!(s.text_visible, "triggering a slide brings text back");
    assert!(
        s.background_media.is_none(),
        "a cleared background stays stopped"
    );
}

#[test]
fn playlist_editing_and_preparse() {
    let a = fixture("song_1024x768.pro6");
    let b = fixture("two_fonts_1920x1080.pro6");
    let c = fixture("render_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    for p in [&a, &b, &c] {
        core.send(Command::PlaylistAdd {
            playlist: 0,
            path: p.clone(),
            arrangement: None,
        });
    }
    core.send(Command::PlaylistSelect(r(0, 0)));
    let s = wait(&rx, "item 0 and next pre-parsed", |s| {
        s.docs.contains_key(&a) && s.docs.contains_key(&b)
    });
    assert_eq!(
        (s.playlist_item, s.selected.as_ref()),
        (Some(r(0, 0)), Some(&a))
    );
    core.send(Command::PlaylistMove {
        playlist: 0,
        from: 0,
        to: 2,
    });
    let s = wait(&rx, "moved", |s| s.playlists[0].playlist.items[2].path == a);
    assert_eq!(
        s.playlist_item,
        Some(r(0, 2)),
        "selection follows the moved item"
    );
    core.send(Command::PlaylistRemove(r(0, 0)));
    let s = wait(&rx, "removed", |s| s.playlists[0].playlist.items.len() == 2);
    assert_eq!(s.playlist_item, Some(r(0, 1)));
    // Load error is reported, not fatal.
    core.send(Command::Open(fixture("invalid/bad_rect.pro6")));
    let s = wait(&rx, "error", |s| !s.load_errors.is_empty());
    assert!(s.notices.last().unwrap().text.contains("bad_rect.pro6"));
}

#[test]
fn playlists_are_saved_reloaded_and_deleted_to_a_folder() {
    let dir = std::env::temp_dir().join(format!("opcore-playlists-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let song = fixture("song_1024x768.pro6");
    let render = fixture("render_1024x768.pro6");
    let config = || CoreConfig {
        playlists_dir: Some(dir.clone()),
        ..Default::default()
    };
    let (core, rx) = start(config());
    core.send(Command::PlaylistCreate("Sunday AM".into()));
    core.send(Command::PlaylistCreate("Sunday PM".into()));
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: song.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
        playlist: 1,
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistRename {
        playlist: 1,
        name: "Evening".into(),
    });
    wait(&rx, "edited", |s| {
        s.playlists.len() == 2 && s.playlists[1].playlist.name == "Evening"
    });
    core.shutdown();
    // Saves run on the I/O thread: wait until both files are on disk, renamed.
    wait_for_disk("saved", || {
        let t = tomls(&dir);
        t.len() == 2
            && t.iter()
                .any(|f| std::fs::read_to_string(f).is_ok_and(|x| x.contains("Evening")))
    });

    // A new start finds both, in name order, with their items.
    let (core, rx) = start(config());
    let s = wait(&rx, "reloaded", |s| s.playlists.len() == 2);
    let names: Vec<&str> = s
        .playlists
        .iter()
        .map(|p| p.playlist.name.as_str())
        .collect();
    assert_eq!(names, ["Evening", "Sunday AM"]);
    assert_eq!(s.playlists[0].playlist.items[0].path, render);
    assert_eq!(s.playlists[1].playlist.items[0].path, song);
    assert!(s.live.is_none(), "starts blank");

    // Delete keeps the file aside, out of the list.
    core.send(Command::PlaylistDelete(0));
    wait(&rx, "deleted", |s| s.playlists.len() == 1);
    wait_for_disk("moved aside", || {
        tomls(&dir).len() == 1
            && std::fs::read_dir(dir.join("deleted")).is_ok_and(|d| d.count() == 1)
    });
    assert_eq!(std::fs::read_dir(dir.join("deleted")).unwrap().count(), 1);
    core.shutdown();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn next_and_prev_stay_inside_the_live_presentations_playlist() {
    let song = fixture("song_1024x768.pro6");
    let render = fixture("render_1024x768.pro6");
    let fonts = fixture("two_fonts_1920x1080.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::PlaylistCreate("AM".into()));
    core.send(Command::PlaylistCreate("PM".into()));
    for (pl, p) in [(0, &song), (1, &render), (1, &fonts)] {
        core.send(Command::PlaylistAdd {
            playlist: pl,
            path: p.clone(),
            arrangement: None,
        });
    }
    wait(&rx, "loaded", |s| {
        [&song, &render, &fonts]
            .iter()
            .all(|p| s.docs.contains_key(*p))
    });
    // End of the only item in AM: doesn't jump into PM.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 5,
    });
    wait(&rx, "song end", |s| pos(s) == Some(5));
    core.send(Command::Next);
    core.send(Command::ClearText); // marker
    let s = wait(&rx, "marker", |s| !s.text_visible);
    assert!(s.is_live(&song));
    // Inside PM: Next crosses render -> fonts, Prev comes back.
    core.send(Command::GoTo {
        presentation: render.clone(),
        position: 3,
    });
    wait(&rx, "render end", |s| {
        s.is_live(&render) && pos(s) == Some(3)
    });
    core.send(Command::Next);
    let s = wait(&rx, "fonts", |s| s.is_live(&fonts));
    assert_eq!(s.playlist_item, Some(r(1, 1)));
    core.send(Command::Prev);
    let s = wait(&rx, "back", |s| s.is_live(&render));
    assert_eq!((pos(&s), s.playlist_item), (Some(3), Some(r(1, 0))));
}

#[test]
fn background_media_and_clears() {
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::SetBackground {
        media: Some("/m/loop.mp4".into()),
        next: Some("/m/next.mp4".into()),
    });
    let s = wait(&rx, "bg set", |s| s.background_media.is_some());
    assert!(
        s.background_visible,
        "setting media shows the background layer"
    );
    assert_eq!(
        s.background_next.as_deref(),
        Some(std::path::Path::new("/m/next.mp4"))
    );
    core.send(Command::ClearAll);
    let s = wait(&rx, "cleared", |s| s.background_media.is_none());
    assert!(!s.text_visible && s.background_next.is_none());
}

#[test]
fn next_past_the_last_slide_moves_to_the_next_playlist_item() {
    let song = fixture("song_1024x768.pro6");
    let render = fixture("render_1024x768.pro6");
    let fonts = fixture("two_fonts_1920x1080.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: song.clone(),
        arrangement: None,
    });
    // Second item uses an explicit arrangement; third is loaded only on demand.
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: fonts.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistSelect(r(0, 0)));
    wait(&rx, "song loaded", |s| {
        s.docs.contains_key(&song) && s.docs.contains_key(&render)
    });

    // Last position of the default arrangement (V1 C V2 C) is disabled; last enabled is 5.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 5,
    });
    wait(&rx, "song end", |s| pos(s) == Some(5));
    core.send(Command::Next);
    let s = wait(&rx, "next item", |s| s.is_live(&render));
    assert_eq!(
        (pos(&s), s.playlist_item, s.selected.as_ref()),
        (Some(0), Some(r(0, 1)), Some(&render))
    );
    assert!(s.text_visible);

    // Prev on the first slide goes back to the previous item's last enabled slide.
    core.send(Command::Prev);
    let s = wait(&rx, "previous item", |s| s.is_live(&song));
    assert_eq!((pos(&s), s.playlist_item), (Some(5), Some(r(0, 0))));

    // Prev on the first slide of the first item stays put.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    wait(&rx, "song start", |s| pos(s) == Some(0) && s.is_live(&song));
    core.send(Command::Prev);
    core.send(Command::ClearText); // marker
    let s = wait(&rx, "prev processed", |s| !s.text_visible);
    assert!(s.is_live(&song) && pos(&s) == Some(0));

    // Into an item that may not be loaded yet: goes live when it is.
    core.send(Command::GoTo {
        presentation: render.clone(),
        position: 3,
    });
    wait(&rx, "render end", |s| {
        pos(s) == Some(3) && s.is_live(&render)
    });
    core.send(Command::Next);
    let s = wait(&rx, "third item", |s| s.is_live(&fonts));
    assert_eq!((pos(&s), s.playlist_item), (Some(0), Some(r(0, 2))));

    // End of the last item: stays.
    core.send(Command::GoTo {
        presentation: fonts.clone(),
        position: 1,
    });
    wait(&rx, "fonts end", |s| pos(s) == Some(1));
    core.send(Command::Next);
    core.send(Command::ClearText); // marker
    let s = wait(&rx, "end of playlist", |s| !s.text_visible);
    assert!(s.is_live(&fonts) && pos(&s) == Some(1));
}

#[test]
fn next_item_uses_its_stored_arrangement() {
    let song = fixture("song_1024x768.pro6");
    let render = fixture("render_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: song.clone(),
        arrangement: Some("AA000000-0001-4000-8000-000000000002".into()),
    });
    core.send(Command::GoTo {
        presentation: render.clone(),
        position: 3,
    });
    wait(&rx, "render end", |s| pos(s) == Some(3));
    core.send(Command::Next);
    let s = wait(&rx, "song live", |s| s.is_live(&song));
    let live = s.live.as_ref().unwrap();
    assert_eq!(
        live.arrangement.as_deref(),
        Some("AA000000-0001-4000-8000-000000000002")
    );
    assert_eq!(live.order.len(), 4, "Short arrangement: V1 C");
}

#[test]
fn next_slide_looks_into_the_next_playlist_item() {
    let song = fixture("song_1024x768.pro6");
    let render = fixture("render_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: song.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistSelect(r(0, 0)));
    wait(&rx, "loaded", |s| {
        s.docs.contains_key(&song) && s.docs.contains_key(&render)
    });
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 4,
    });
    let s = wait(&rx, "pos 4", |s| pos(s) == Some(4));
    assert_eq!(s.next_slide().map(|n| (n.0, n.2)), Some((song.clone(), 5)));
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 5,
    });
    let s = wait(&rx, "pos 5", |s| pos(s) == Some(5));
    assert_eq!(s.next_slide().map(|n| (n.0, n.2)), Some((render, 0)));
}

#[test]
fn edits_work_on_a_copy_until_saved_with_undo_and_revert() {
    use doc::Edit;
    let dir = std::env::temp_dir().join(format!("opcore-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let (core, rx) = start(CoreConfig::default());
    let edit = |e: Edit| Command::Edit {
        presentation: file.clone(),
        edit: e,
        accept_native: false,
    };
    core.send(Command::GoTo {
        presentation: file.clone(),
        position: 1,
    });
    let s = wait(&rx, "live", |s| pos(s) == Some(1));
    let saved0 = s.docs[&file].clone();
    let live_uuid = saved0.slides().nth(1).unwrap().uuid.clone();
    let text_of = |d: &doc::Document, flat: usize| d.slides().nth(flat).unwrap().plain_text();
    let live_text = text_of(&saved0, 1);

    // Edits go to the editor copy: the saved document and the output are untouched.
    core.send(edit(Edit::AddSlide {
        group: 0,
        at: 0,
        text: "Inserted".into(),
    }));
    core.send(edit(Edit::SetText {
        group: 0,
        slide: 2,
        text: "Changed".into(),
    }));
    let s = wait(&rx, "edited", |s| {
        s.editing
            .get(&file)
            .is_some_and(|e| e.undo.as_deref() == Some("Edit text"))
    });
    assert!(s.unsaved.contains(&file));
    let e = &s.editing[&file];
    assert_eq!(e.doc.groups[0].slides.len(), 3);
    assert_eq!(text_of(&e.doc, 2), "Changed");
    assert!(
        Arc::ptr_eq(&s.docs[&file], &saved0),
        "saved document untouched"
    );
    let live = s.live.as_ref().unwrap();
    assert!(Arc::ptr_eq(&live.doc, &saved0), "output untouched");
    assert_eq!(live.position, Some(1));

    // Undo / redo walk the editor copy.
    core.send(Command::Undo(file.clone()));
    let s = wait(&rx, "undone", |s| {
        s.editing
            .get(&file)
            .is_some_and(|e| e.redo.as_deref() == Some("Edit text"))
    });
    assert_eq!(text_of(&s.editing[&file].doc, 2), live_text);
    core.send(Command::Redo(file.clone()));
    let s = wait(&rx, "redone", |s| {
        s.editing
            .get(&file)
            .is_some_and(|e| e.redo.is_none() && e.undo.as_deref() == Some("Edit text"))
    });
    assert_eq!(text_of(&s.editing[&file].doc, 2), "Changed");

    // A refused edit is a notice, not a change (and not an undo step).
    core.send(edit(Edit::RemoveSlide {
        group: 0,
        slide: 99,
    }));
    let s = wait(&rx, "refused", |s| {
        s.notices
            .last()
            .is_some_and(|n| n.text.contains("Nothing was changed"))
    });
    assert_eq!(s.editing[&file].doc.groups[0].slides.len(), 3);
    assert_eq!(s.editing[&file].undo.as_deref(), Some("Edit text"));

    // Save writes the file (with a backup) and becomes the saved document; the
    // output keeps the old version until the next slide change.
    core.send(Command::Save(file.clone()));
    let s = wait(&rx, "saved", |s| !s.unsaved.contains(&file));
    assert_eq!(s.docs[&file].groups[0].slides.len(), 3);
    assert!(Arc::ptr_eq(&s.docs[&file], &s.editing[&file].doc));
    assert_eq!(doc::load(&file).unwrap().groups[0].slides.len(), 3);
    assert_eq!(
        doc::load(&dir.join("song.pro6.bak")).unwrap().groups[0]
            .slides
            .len(),
        2
    );
    assert!(Arc::ptr_eq(&s.live.as_ref().unwrap().doc, &saved0));
    core.send(Command::Next);
    let s = wait(&rx, "next on the saved version", |s| {
        s.live
            .as_ref()
            .is_some_and(|l| Arc::ptr_eq(&l.doc, &s.docs[&file]))
    });
    let live = s.live.as_ref().unwrap();
    // The live slide (now third in Verse 1) was followed by UUID; Next went past it.
    assert_eq!(live.position, Some(3));
    let prev = &live.doc.slides().nth(live.order[2].flat).unwrap().uuid;
    assert_eq!(prev, &live_uuid);

    // Undo after saving makes the copy unsaved again.
    core.send(Command::Undo(file.clone()));
    let s = wait(&rx, "undo after save", |s| s.unsaved.contains(&file));
    assert_eq!(text_of(&s.editing[&file].doc, 2), live_text);

    // Revert throws the editor copy away.
    core.send(edit(Edit::RemoveGroup { group: 2 }));
    wait(&rx, "edited again", |s| {
        s.editing
            .get(&file)
            .is_some_and(|e| e.doc.groups.len() == 2)
    });
    core.send(Command::Revert(file.clone()));
    let s = wait(&rx, "reverted", |s| {
        !s.unsaved.contains(&file) && !s.editing.contains_key(&file)
    });
    assert_eq!(s.docs[&file].groups.len(), 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn native_only_edits_need_consent_and_save_opres() {
    let dir = std::env::temp_dir().join(format!("opcore-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let original = std::fs::read(&file).unwrap();
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::Open(file.clone()));
    let s = wait(&rx, "open", |s| s.docs.contains_key(&file));
    // No edit uses a native-only feature yet (E2/E3 add them), so mark the copy
    // native the way "Continue anyway" does and check the save rule end to end.
    let mut d = (*s.docs[&file]).clone();
    let first = d.groups[0].slides[0].uuid.clone();
    doc::mark_native(&mut d, &[first]);
    let saved = doc::save(&d).unwrap();
    assert_eq!(saved.path, dir.join("song.opres"));
    assert_eq!(std::fs::read(&file).unwrap(), original);
    // Opening the .pro6 path now loads the .opres.
    let (core2, rx2) = start(CoreConfig::default());
    core2.send(Command::Open(file.clone()));
    let s = wait(&rx2, "open native", |s| s.docs.contains_key(&file));
    assert_eq!(s.docs[&file].mode, doc::Mode::Native);
    assert_eq!(s.docs[&file].path, dir.join("song.opres"));
    drop(core);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The announcements fixture with slide timers divided by 10 (0.3 s, 0.5 s, 0.4 s, 0.4 s).
fn fast_announcements(dir: &std::path::Path, loops_back: bool) -> PathBuf {
    let src = std::fs::read_to_string(fixture("announcements_synthetic.pro6")).unwrap();
    let mut fast = src
        .replace(r#"duration="3""#, r#"duration="0.3""#)
        .replace(r#"duration="5""#, r#"duration="0.5""#)
        .replace(r#"duration="4""#, r#"duration="0.4""#);
    if !loops_back {
        fast = fast.replace(r#"loopToBeginning="true""#, r#"loopToBeginning="false""#);
    }
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(if loops_back {
        "ann.pro6"
    } else {
        "ann-once.pro6"
    });
    std::fs::write(&p, fast).unwrap();
    p
}

#[test]
fn timed_presentation_loops_on_its_own_skips_disabled_and_loops_back() {
    let dir = std::env::temp_dir().join(format!("opcore-loop-{}", std::process::id()));
    let ann = fast_announcements(&dir, true);
    let (core, rx) = start(CoreConfig::default());
    let t0 = std::time::Instant::now();
    // Going live is enough: slide timers start the loop.
    core.send(Command::GoTo {
        presentation: ann.clone(),
        position: 0,
    });
    let s = wait(&rx, "looping", |s| s.looping.is_some() && pos(s) == Some(0));
    assert_eq!(
        s.looping.as_ref().unwrap().slide_duration,
        Some(Duration::from_millis(300))
    );
    let at = |p: usize| {
        let s = wait(&rx, "advance", |s| pos(s) == Some(p));
        (s, t0.elapsed())
    };
    let (_, t1) = at(1);
    assert!(
        t1 >= Duration::from_millis(290) && t1 < Duration::from_millis(650),
        "{t1:?}"
    );
    let (s, t2) = at(3); // position 2 is disabled
    assert!(
        t2 >= Duration::from_millis(790) && t2 < Duration::from_millis(1250),
        "{t2:?}"
    );
    assert!(s.looping.as_ref().unwrap().loops_back);
    let (s, t3) = at(0); // last slide has loopToBeginning="true"
    assert!(
        t3 >= Duration::from_millis(1190) && t3 < Duration::from_millis(1750),
        "{t3:?}"
    );
    assert!(s.looping.is_some());
    core.send(Command::StopLoop);
    wait(&rx, "stopped", |s| s.looping.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn loop_rules_next_restarts_clear_stops_other_live_stops_select_keeps() {
    let dir = std::env::temp_dir().join(format!("opcore-loop-rules-{}", std::process::id()));
    let ann = fast_announcements(&dir, true);
    let song = fixture("song_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::Open(song.clone()));
    core.send(Command::Open(ann.clone()));
    wait(&rx, "loaded", |s| {
        s.docs.contains_key(&ann) && s.docs.contains_key(&song)
    });

    // Start Loop with nothing live puts the presentation live and loops.
    core.send(Command::StartLoop);
    wait(&rx, "looping", |s| s.looping.is_some() && s.is_live(&ann));
    // Next/Prev step as usual and restart the timer for the slide they land on.
    core.send(Command::Next);
    let s = wait(&rx, "next", |s| pos(s) == Some(1));
    let l = s.looping.as_ref().unwrap();
    assert_eq!(l.slide_duration, Some(Duration::from_millis(500)));
    let left = l
        .next_at
        .unwrap()
        .saturating_duration_since(std::time::Instant::now());
    assert!(
        left > Duration::from_millis(350),
        "timer restarted: {left:?}"
    );

    // Selecting another presentation keeps the loop going.
    core.send(Command::Open(song.clone()));
    let s = wait(&rx, "selected song", |s| s.selected.as_ref() == Some(&song));
    assert!(s.looping.is_some());

    // Putting another presentation live stops it.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    let s = wait(&rx, "song live", |s| s.is_live(&song));
    assert!(s.looping.is_none());
    assert!(
        s.notices
            .last()
            .unwrap()
            .text
            .contains("another presentation went live")
    );

    // Coming back starts it again; Clear stops it, and Next doesn't restart it.
    core.send(Command::GoTo {
        presentation: ann.clone(),
        position: 0,
    });
    wait(&rx, "looping again", |s| {
        s.looping.is_some() && s.is_live(&ann)
    });
    core.send(Command::ClearText);
    let s = wait(&rx, "cleared", |s| !s.text_visible);
    assert!(s.looping.is_none());
    core.send(Command::Next);
    let s = wait(&rx, "next after clear", |s| pos(s) == Some(1));
    assert!(s.looping.is_none(), "a cleared loop stays stopped");
    core.send(Command::StartLoop);
    let s = wait(&rx, "resumed", |s| s.looping.is_some());
    assert!(s.looping.as_ref().unwrap().next_at.is_some());

    // No timers: refuses with a clear message.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    core.send(Command::StartLoop);
    let s = wait(&rx, "refused", |s| {
        s.notices
            .last()
            .is_some_and(|n| n.text.contains("no slide timers"))
    });
    assert!(s.looping.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn start_loop_waits_for_loading_and_stops_at_end_without_loop_back() {
    let dir = std::env::temp_dir().join(format!("opcore-loop-once-{}", std::process::id()));
    let ann = fast_announcements(&dir, false);
    let (core, rx) = start(CoreConfig::default());
    // Start Loop before the file is loaded: starts once it is.
    core.send(Command::Open(ann.clone()));
    core.send(Command::StartLoop);
    wait(&rx, "looping", |s| s.looping.is_some() && pos(s) == Some(0));
    let s = wait(&rx, "finished", |s| s.looping.is_none());
    assert_eq!(pos(&s), Some(3), "stays on the last slide");
    assert!(s.notices.last().unwrap().text.contains("Loop finished"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn stepping_past_a_presentation_outside_the_playlist_says_why() {
    let render = fixture("render_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::Open(render.clone()));
    wait(&rx, "loaded", |s| s.docs.contains_key(&render));
    core.send(Command::GoTo {
        presentation: render.clone(),
        position: 3,
    });
    wait(&rx, "end", |s| pos(s) == Some(3));
    core.send(Command::Next);
    let s = wait(&rx, "hint", |s| {
        s.notices
            .iter()
            .any(|n| n.text.contains("isn't in the playlist"))
    });
    assert_eq!(pos(&s), Some(3));
    // Only once per presentation.
    core.send(Command::Next);
    core.send(Command::ClearText); // marker
    let s = wait(&rx, "marker", |s| !s.text_visible);
    let hints = s
        .notices
        .iter()
        .filter(|n| n.text.contains("isn't in the playlist"))
        .count();
    assert_eq!(hints, 1);
}

/// John's report: after Clear during the Welcome loop, the cleared slide kept coming back.
#[test]
fn clear_during_a_loop_keeps_everything_off_until_the_operator_shows_a_slide() {
    let dir = std::env::temp_dir().join(format!("opcore-loop-clear-{}", std::process::id()));
    let ann = fast_announcements(&dir, true);
    let song = fixture("song_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::Open(song.clone()));
    core.send(Command::GoTo {
        presentation: ann.clone(),
        position: 0,
    });
    wait(&rx, "looping", |s| {
        s.looping.is_some() && s.docs.contains_key(&song)
    });
    for clear in [Command::ClearAll, Command::ClearText] {
        core.send(Command::GoTo {
            presentation: ann.clone(),
            position: 0,
        });
        core.send(Command::StartLoop);
        wait(&rx, "looping again", |s| s.looping.is_some());
        core.send(clear);
        let s = wait(&rx, "cleared", |s| !s.text_visible);
        assert!(s.looping.is_none());
        let seq = s.seq;
        // Well past every slide timer: nothing changes on its own.
        std::thread::sleep(Duration::from_millis(900));
        let mut last = s;
        while let Ok(n) = rx.try_recv() {
            last = n;
        }
        assert!(
            !last.text_visible && last.looping.is_none(),
            "seq {seq} -> {}",
            last.seq
        );
        // Showing a song slide shows that slide, and it stays.
        core.send(Command::GoTo {
            presentation: song.clone(),
            position: 1,
        });
        let s = wait(&rx, "song", |s| s.is_live(&song) && s.text_visible);
        assert_eq!(pos(&s), Some(1));
        std::thread::sleep(Duration::from_millis(700));
        let mut last = s;
        while let Ok(n) = rx.try_recv() {
            last = n;
        }
        assert!(last.is_live(&song) && pos(&last) == Some(1) && last.looping.is_none());
    }
    // Clear Background is one-shot: it stops the background, no lasting "cleared" state.
    core.send(Command::SetBackground {
        media: Some("/m/a.mp4".into()),
        next: None,
    });
    wait(&rx, "bg", |s| s.background_media.is_some());
    core.send(Command::ClearBackground);
    let s = wait(&rx, "bg cleared", |s| s.background_media.is_none());
    assert!(s.background_visible && s.text_visible);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// E7 acceptance: a theme applied to 10 presentations in one action; each matches
/// the theme; one undo per presentation reverts it.
#[test]
fn theme_applies_to_ten_presentations_with_one_undo_each() {
    let dir = std::env::temp_dir().join(format!("opcore-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sources = [
        "song_1024x768.pro6",
        "render_1024x768.pro6",
        "two_fonts_1920x1080.pro6",
        "announcements_synthetic.pro6",
        "unknowns.pro6",
    ];
    let mut files = Vec::new();
    for copy in 0..2 {
        for s in sources {
            let f = dir.join(format!("{copy}-{s}"));
            std::fs::copy(fixture(s), &f).unwrap();
            files.push(f);
        }
    }
    // A theme made from the song's first slide, restyled.
    let song = doc::load(&files[0]).unwrap();
    let mut theme = doc::Theme::from_slide("Blue", &song, (0, 0)).unwrap();
    theme.background = Some(doc::Rgba {
        r: 0.0,
        g: 0.1,
        b: 0.4,
        a: 1.0,
    });
    theme.text.run.color = Some([255, 230, 120]);
    theme.text.run.bold = true;
    theme.text.align = doc::Align::Left;
    theme.text_frame = doc::Rect {
        x: 40.0,
        y: 500.0,
        width: 944.0,
        height: 220.0,
    };
    let theme = theme
        .save(&dir.join("Themes").join("Blue.optheme"))
        .unwrap();
    let before: Vec<doc::Document> = files.iter().map(|f| doc::load(f).unwrap()).collect();

    let (core, rx) = start(CoreConfig::default());
    core.send(Command::ApplyTheme {
        presentations: files.clone(),
        theme: Arc::new(theme.clone()),
        targets: opcore::ThemeTargets::All,
        save: false,
    });
    let s = wait(&rx, "themed", |s| {
        files.iter().all(|f| s.editing.contains_key(f))
    });
    for (f, b) in files.iter().zip(&before) {
        let e = &s.editing[f];
        assert_eq!(
            e.undo.as_deref(),
            Some("Apply theme Blue"),
            "{}",
            f.display()
        );
        let d = &e.doc;
        assert_eq!(d.canvas, theme.canvas, "rescaled to the theme's canvas");
        assert_eq!(d.mode, doc::Mode::Pro6, "nothing native-only in this theme");
        for slide in d.slides() {
            assert_eq!(slide.background, theme.background);
            if let Some(t) = slide.text_boxes().find(|t| t.problem.is_none()) {
                assert_eq!(t.frame, theme.text_frame, "{}", f.display());
                for r in t.runs() {
                    assert_eq!(r.color, Some([255, 230, 120]));
                    assert!(r.bold);
                    assert_eq!(r.font, theme.text.run.font);
                }
                assert!(t.paragraphs.iter().all(|p| p.align == doc::Align::Left));
            }
        }
        let _ = b;
    }
    // One undo per presentation restores each.
    for f in &files {
        core.send(Command::Undo(f.clone()));
    }
    let s = wait(&rx, "undone", |s| {
        files
            .iter()
            .all(|f| s.editing.get(f).is_none_or(|e| e.undo.is_none()))
    });
    for (f, b) in files.iter().zip(&before) {
        let d = s.edited_doc(f).unwrap();
        assert_eq!(d.groups, b.groups, "{}", f.display());
        assert_eq!(d.canvas, b.canvas);
    }
    drop(core);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn editing_a_theme_restyles_linked_slides_in_native_presentations() {
    let dir = std::env::temp_dir().join(format!("opcore-themelink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let mut d = doc::load(&file).unwrap();
    let mut theme = doc::Theme::from_slide("Linked", &d, (0, 0)).unwrap();
    theme.text.outline = Some(doc::Outline {
        width: 2.0,
        color: doc::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
    });
    let theme = theme.save(&dir.join("Linked.optheme")).unwrap();
    // A native presentation with only its first slide linked.
    doc::Edit::ApplyTheme {
        slides: vec![(0, 0)],
        theme: Box::new(theme.for_document(&file)),
        all: false,
        link: true,
    }
    .apply(&mut d)
    .unwrap();
    let first = d.groups[0].slides[0].uuid.clone();
    doc::mark_native(&mut d, &[first]);
    let saved = doc::save(&d).unwrap();
    // The theme is edited: a gold text colour.
    let mut edited = theme.clone();
    edited.text.run.color = Some([212, 175, 55]);
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::ApplyTheme {
        presentations: vec![saved.path.clone()],
        theme: Arc::new(edited),
        targets: opcore::ThemeTargets::Linked,
        save: true,
    });
    wait(&rx, "saved", |s| {
        s.editing.contains_key(&saved.path) && !s.unsaved.contains(&saved.path)
    });
    let back = doc::load(&saved.path).unwrap();
    let colours = |s: &doc::Slide| {
        s.text_boxes()
            .flat_map(|t| t.runs().map(|r| r.color))
            .collect::<Vec<_>>()
    };
    assert!(
        colours(&back.groups[0].slides[0])
            .iter()
            .all(|c| *c == Some([212, 175, 55]))
    );
    assert!(
        colours(&back.groups[0].slides[1])
            .iter()
            .all(|c| *c != Some([212, 175, 55])),
        "unlinked slides stay"
    );
    drop(core);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn slide_videos_play_pause_and_follow_their_end_setting() {
    // A native copy of the song: slide 1 has a video that waits for Play and goes to
    // the next slide at its end; slide 2's video goes away at its end.
    let dir = std::env::temp_dir().join(format!("opcore-video-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = doc::load(&fixture("song_1024x768.pro6")).unwrap();
    let order = opcore::slide_order(&d, None);
    let (a, b) = (order[0], order[1]);
    let mut first = doc::SlideVideo::new(doc::MediaRef {
        path: "intro.mp4".into(),
        sha256: None,
    });
    first.start = doc::VideoStart::OnClick;
    first.end = doc::VideoEnd::Next;
    let mut second = doc::SlideVideo::new(doc::MediaRef {
        path: "/videos/missions.mp4".into(),
        sha256: None,
    });
    second.end = doc::VideoEnd::Stop;
    for (r, v) in [(a, first), (b, second)] {
        doc::Edit::SetSlideVideo {
            group: r.group,
            slide: r.slide,
            video: Some(v),
        }
        .apply(&mut d)
        .unwrap();
    }
    d.path = dir.join("Video Sunday.opres");
    d.mode = doc::Mode::Native;
    let path = doc::save(&d).unwrap().path;

    let (core, rx) = start(CoreConfig::default());
    core.send(Command::GoTo {
        presentation: path.clone(),
        position: 0,
    });
    let s = wait(&rx, "first slide live", |s| pos(s) == Some(0));
    let intro = dir.join("intro.mp4");
    assert_eq!(
        s.background_media.as_deref(),
        Some(intro.as_path()),
        "relative to the file"
    );
    assert!(s.media_paused, "waits for Play");
    assert!(s.background_cue.is_some());
    core.send(Command::MediaPlay);
    let s = wait(&rx, "playing", |s| !s.media_paused);
    let restarts = s.media_restart;
    core.send(Command::MediaRestart);
    wait(&rx, "restarted", |s| s.media_restart > restarts);
    // An end report for something no longer playing is ignored.
    core.send(Command::MediaEnded("/other.mp4".into()));
    // Its end: the next slide, whose own video takes over.
    core.send(Command::MediaEnded(intro.clone()));
    let s = wait(&rx, "next slide", |s| pos(s) == Some(1));
    let missions = std::path::PathBuf::from("/videos/missions.mp4");
    assert_eq!(s.background_media.as_deref(), Some(missions.as_path()));
    assert!(!s.media_paused, "starts when the slide is shown");
    // Its end: the video goes away; the slide stays.
    core.send(Command::MediaEnded(missions));
    let s = wait(&rx, "video gone", |s| s.background_media.is_none());
    assert_eq!(pos(&s), Some(1));
    assert!(s.background_cue.is_none());
    // Moving on to a slide without a video ends the playing one.
    core.send(Command::GoTo {
        presentation: path.clone(),
        position: 1,
    });
    wait(&rx, "video again", |s| s.background_media.is_some());
    core.send(Command::Next);
    let s = wait(&rx, "third slide", |s| pos(s) == Some(2));
    assert!(s.background_media.is_none(), "moving on ends the video");
    // A background from the Video/Image panel stays through slide changes.
    let panel = std::path::PathBuf::from("/videos/motion.mp4");
    core.send(Command::SetBackground {
        media: Some(panel.clone()),
        next: None,
    });
    wait(&rx, "panel background", |s| s.background_media.is_some());
    core.send(Command::GoTo {
        presentation: path.clone(),
        position: 2,
    });
    // Commands run in order: once the pause shows, the slide change has been made.
    core.send(Command::MediaPause);
    let s = wait(&rx, "slide shown again", |s| s.media_paused);
    assert_eq!(s.background_media.as_deref(), Some(panel.as_path()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn freeze_and_blank_are_output_states_that_leave_live_alone() {
    let (core, rx) = start(CoreConfig::default());
    let song = fixture("song_1024x768.pro6");
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    wait(&rx, "live", |s| pos(s) == Some(0));
    core.send(Command::Freeze(true));
    wait(&rx, "frozen", |s| s.frozen);
    // The operator's live slide still moves on (the output holds; see raster).
    core.send(Command::Next);
    wait(&rx, "moved on while frozen", |s| {
        s.frozen && pos(s) == Some(1)
    });
    core.send(Command::Blank(true));
    wait(&rx, "blanked", |s| s.blanked && s.frozen);
    core.send(Command::Freeze(false));
    core.send(Command::Blank(false));
    let s = wait(&rx, "back to live", |s| !s.blanked && !s.frozen);
    assert_eq!(pos(&s), Some(1));
}

#[test]
fn renamed_and_deleted_presentations_update_the_playlists() {
    let dir = std::env::temp_dir().join(format!("openpresenter-moved-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("A.pro6");
    let b = dir.join("B.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &a).unwrap();
    std::fs::copy(fixture("song_1024x768.pro6"), &b).unwrap();
    let (core, rx) = start(CoreConfig::default());
    for p in [&a, &b, &a] {
        core.send(Command::PlaylistAdd {
            playlist: 0,
            path: p.clone(),
            arrangement: None,
        });
    }
    core.send(Command::Open(a.clone()));
    wait(&rx, "loaded", |s| {
        s.docs.contains_key(&a)
            && s.playlists
                .first()
                .is_some_and(|p| p.playlist.items.len() == 3)
    });
    let renamed = dir.join("A2.pro6");
    std::fs::rename(&a, &renamed).unwrap();
    core.send(Command::PresentationMoved {
        from: a.clone(),
        to: Some(renamed.clone()),
    });
    let s = wait(&rx, "renamed", |s| s.selected.as_ref() == Some(&renamed));
    let paths: Vec<&PathBuf> = s.playlists[0]
        .playlist
        .items
        .iter()
        .map(|i| &i.path)
        .collect();
    assert_eq!(paths, [&renamed, &b, &renamed]);
    assert!(!s.docs.contains_key(&a));
    core.send(Command::PresentationMoved {
        from: b.clone(),
        to: None,
    });
    let s = wait(&rx, "deleted", |s| s.playlists[0].playlist.items.len() == 2);
    assert!(
        s.playlists[0]
            .playlist
            .items
            .iter()
            .all(|i| i.path == renamed)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
