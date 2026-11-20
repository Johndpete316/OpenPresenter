#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Drive the core actor with commands and check the snapshots.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use pp6core::{Command, CoreConfig, LiveRestore, Playlist, PlaylistItem, RestoreState, ShowState};

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

fn start(config: CoreConfig) -> (pp6core::CoreHandle, Receiver<Arc<ShowState>>) {
    let (core, _join) = pp6core::spawn(config).unwrap();
    let rx = core.subscribe(None);
    (core, rx)
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
    core.send(Command::ClearBackground);
    wait(&rx, "bg cleared", |s| {
        !s.background_visible && s.text_visible
    });
    core.send(Command::ClearAll);
    wait(&rx, "all cleared", |s| {
        !s.background_visible && !s.text_visible
    });
    core.send(Command::Next);
    let s = wait(&rx, "next", |s| pos(s) == Some(1));
    assert!(s.text_visible, "triggering a slide brings text back");
    assert!(!s.background_visible, "a cleared background stays cleared");
}

#[test]
fn playlist_editing_and_preparse() {
    let a = fixture("song_1024x768.pro6");
    let b = fixture("two_fonts_1920x1080.pro6");
    let c = fixture("render_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    for p in [&a, &b, &c] {
        core.send(Command::PlaylistAdd {
            path: p.clone(),
            arrangement: None,
        });
    }
    core.send(Command::PlaylistSelect(0));
    let s = wait(&rx, "item 0 and next pre-parsed", |s| {
        s.docs.contains_key(&a) && s.docs.contains_key(&b)
    });
    assert_eq!((s.playlist_item, s.selected.as_ref()), (Some(0), Some(&a)));
    core.send(Command::PlaylistMove { from: 0, to: 2 });
    let s = wait(&rx, "moved", |s| s.playlist.items[2].path == a);
    assert_eq!(s.playlist_item, Some(2), "selection follows the moved item");
    core.send(Command::PlaylistRemove(0));
    let s = wait(&rx, "removed", |s| s.playlist.items.len() == 2);
    assert_eq!(s.playlist_item, Some(1));
    // Load error is reported, not fatal.
    core.send(Command::Open(fixture("invalid/bad_rect.pro6")));
    let s = wait(&rx, "error", |s| !s.load_errors.is_empty());
    assert!(s.notices.last().unwrap().text.contains("bad_rect.pro6"));
}

#[test]
fn restore_recent_session_goes_live() {
    let song = fixture("song_1024x768.pro6");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let restore = RestoreState {
        saved_at_unix: now - 60,
        playlist_path: None,
        playlist: Playlist {
            name: "x".into(),
            items: vec![PlaylistItem {
                path: song.clone(),
                arrangement: None,
            }],
        },
        playlist_item: Some(0),
        live: Some(LiveRestore {
            path: song.clone(),
            arrangement: None,
            position: 3,
            text_visible: false,
            background_visible: true,
        }),
        background_media: None,
    };
    let (_core, rx) = start(CoreConfig {
        restore: Some(restore.clone()),
        ..Default::default()
    });
    let s = wait(&rx, "restored live", |s| s.live.is_some());
    assert_eq!(pos(&s), Some(3));
    assert!(!s.text_visible, "cleared state restored too");

    // An old session restores the selection but doesn't go live.
    let old = RestoreState {
        saved_at_unix: now - 7 * 3600,
        ..restore
    };
    let (core, rx) = start(CoreConfig {
        restore: Some(old),
        ..Default::default()
    });
    let s = wait(&rx, "loaded", |s| s.docs.contains_key(&song));
    assert!(s.live.is_none());
    assert_eq!(s.playlist_item, Some(0));
    core.shutdown();
}

#[test]
fn background_media_and_clears() {
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::ClearBackground);
    wait(&rx, "bg hidden", |s| !s.background_visible);
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
    assert!(!s.background_visible && !s.text_visible);
}

#[test]
fn next_past_the_last_slide_moves_to_the_next_playlist_item() {
    let song = fixture("song_1024x768.pro6");
    let render = fixture("render_1024x768.pro6");
    let fonts = fixture("two_fonts_1920x1080.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::PlaylistAdd {
        path: song.clone(),
        arrangement: None,
    });
    // Second item uses an explicit arrangement; third is loaded only on demand.
    core.send(Command::PlaylistAdd {
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
        path: fonts.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistSelect(0));
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
        (Some(0), Some(1), Some(&render))
    );
    assert!(s.text_visible);

    // Prev on the first slide stays put (only Next crosses items).
    core.send(Command::Prev);
    core.send(Command::ClearBackground); // marker
    let s = wait(&rx, "prev processed", |s| !s.background_visible);
    assert!(s.is_live(&render) && pos(&s) == Some(0));

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
    assert_eq!((pos(&s), s.playlist_item), (Some(0), Some(2)));

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
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
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
        path: song.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistAdd {
        path: render.clone(),
        arrangement: None,
    });
    core.send(Command::PlaylistSelect(0));
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
fn edits_follow_the_live_slide_save_and_revert() {
    use pro6::ops::Op;
    let dir = std::env::temp_dir().join(format!("pp6core-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("song.pro6");
    std::fs::copy(fixture("song_1024x768.pro6"), &file).unwrap();
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::SelectArrangement {
        presentation: file.clone(),
        arrangement: None,
    });
    core.send(Command::GoTo {
        presentation: file.clone(),
        position: 1,
    });
    let s = wait(&rx, "live", |s| pos(s) == Some(1));
    let live_uuid = s
        .live
        .as_ref()
        .unwrap()
        .doc
        .slides()
        .nth(1)
        .unwrap()
        .uuid
        .clone();

    // Insert a slide before the live one: the live slide keeps showing, its position shifts.
    core.send(Command::Edit {
        presentation: file.clone(),
        op: Op::AddSlide {
            group: 0,
            at: 0,
            text: "Inserted".into(),
        },
    });
    let s = wait(&rx, "edited", |s| s.unsaved.contains(&file));
    let live = s.live.as_ref().unwrap();
    assert_eq!(live.position, Some(2));
    assert_eq!(
        live.doc.slides().nth(live.slide.flat).unwrap().uuid,
        live_uuid
    );
    assert!(
        Arc::ptr_eq(&live.doc, &s.docs[&file]),
        "live follows the edited document"
    );

    // Editing the live slide's text updates what's live.
    core.send(Command::Edit {
        presentation: file.clone(),
        op: Op::SetText {
            group: 0,
            slide: 2,
            text: "Changed".into(),
        },
    });
    let s = wait(&rx, "text edited", |s| {
        s.live.as_ref().is_some_and(|l| {
            Arc::ptr_eq(&l.doc, &s.docs[&file])
                && s.docs[&file]
                    .slides()
                    .nth(2)
                    .is_some_and(|x| x.uuid == live_uuid)
        }) && s.seq > 0
    });
    let t = s.docs[&file]
        .slides()
        .nth(2)
        .unwrap()
        .text_elements()
        .next()
        .unwrap()
        .rtf_doc(&file)
        .unwrap()
        .unwrap()
        .plain_text();
    assert_eq!(t, "Changed");

    // A refused edit is a notice, not a change.
    core.send(Command::Edit {
        presentation: file.clone(),
        op: Op::RemoveSlide {
            group: 0,
            slide: 99,
        },
    });
    let s = wait(&rx, "refused", |s| {
        s.notices
            .last()
            .is_some_and(|n| n.text.contains("Nothing was changed"))
    });
    assert_eq!(s.docs[&file].groups()[0].slides().len(), 3);

    // Save writes the file and keeps a backup of the previous version.
    core.send(Command::Save(file.clone()));
    wait(&rx, "saved", |s| !s.unsaved.contains(&file));
    assert_eq!(pro6::load(&file).unwrap().groups()[0].slides().len(), 3);
    assert_eq!(
        pro6::load(&dir.join("song.pro6.bak")).unwrap().groups()[0]
            .slides()
            .len(),
        2
    );

    // Revert throws away unsaved edits.
    core.send(Command::Edit {
        presentation: file.clone(),
        op: Op::RemoveGroup { group: 2 },
    });
    wait(&rx, "edited again", |s| s.unsaved.contains(&file));
    core.send(Command::Revert(file.clone()));
    let s = wait(&rx, "reverted", |s| {
        !s.unsaved.contains(&file) && s.docs.get(&file).is_some_and(|d| d.groups().len() == 3)
    });
    assert_eq!(s.docs[&file].groups()[0].slides().len(), 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The synthetic announcements fixture with all times divided by 10 (0.3 s, 0.5 s, 0.4 s slides).
fn fast_announcements(dir: &std::path::Path, wrap: bool) -> PathBuf {
    let src = std::fs::read_to_string(fixture("announcements_synthetic.pro6")).unwrap();
    let fast = src
        .replace(r#"timeStamp="3""#, r#"timeStamp="0.3""#)
        .replace(r#"timeStamp="8""#, r#"timeStamp="0.8""#)
        .replace(r#"timeStamp="9""#, r#"timeStamp="0.9""#)
        .replace(r#"duration="12""#, r#"duration="1.2""#)
        .replace(
            r#"loop="true""#,
            if wrap {
                r#"loop="true""#
            } else {
                r#"loop="false""#
            },
        );
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(if wrap { "ann.pro6" } else { "ann-once.pro6" });
    std::fs::write(&p, fast).unwrap();
    p
}

#[test]
fn loop_advances_on_its_own_skips_disabled_and_wraps() {
    let dir = std::env::temp_dir().join(format!("pp6core-loop-{}", std::process::id()));
    let ann = fast_announcements(&dir, true);
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::Open(ann.clone()));
    wait(&rx, "loaded", |s| s.docs.contains_key(&ann));
    let t0 = std::time::Instant::now();
    core.send(Command::StartLoop);
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
        t1 >= Duration::from_millis(290) && t1 < Duration::from_millis(600),
        "{t1:?}"
    );
    let (_, t2) = at(3); // position 2 is disabled
    assert!(
        t2 >= Duration::from_millis(790) && t2 < Duration::from_millis(1200),
        "{t2:?}"
    );
    let (s, t3) = at(0); // wraps: RVTimeline loop="true"
    assert!(
        t3 >= Duration::from_millis(1190) && t3 < Duration::from_millis(1700),
        "{t3:?}"
    );
    assert!(s.looping.is_some());
    core.send(Command::StopLoop);
    wait(&rx, "stopped", |s| s.looping.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn loop_rules_next_restarts_clear_stops_other_live_stops_select_keeps() {
    let dir = std::env::temp_dir().join(format!("pp6core-loop-rules-{}", std::process::id()));
    let ann = fast_announcements(&dir, true);
    let song = fixture("song_1024x768.pro6");
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::Open(song.clone()));
    core.send(Command::Open(ann.clone()));
    wait(&rx, "loaded", |s| {
        s.docs.contains_key(&ann) && s.docs.contains_key(&song)
    });

    // Next/Prev step as usual and restart the timer for the slide they land on.
    core.send(Command::StartLoop);
    wait(&rx, "looping", |s| s.looping.is_some());
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

    // Clear stops it too.
    core.send(Command::GoTo {
        presentation: ann.clone(),
        position: 0,
    });
    core.send(Command::StartLoop);
    wait(&rx, "looping again", |s| s.looping.is_some());
    core.send(Command::ClearText);
    let s = wait(&rx, "cleared", |s| !s.text_visible);
    assert!(s.looping.is_none());

    // No timing in the file: refuses with a clear message.
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    core.send(Command::StartLoop);
    let s = wait(&rx, "refused", |s| {
        s.notices
            .last()
            .is_some_and(|n| n.text.contains("no slide timing"))
    });
    assert!(s.looping.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn non_repeating_timeline_stops_at_the_end() {
    let dir = std::env::temp_dir().join(format!("pp6core-loop-once-{}", std::process::id()));
    let ann = fast_announcements(&dir, false);
    let (core, rx) = start(CoreConfig::default());
    core.send(Command::GoTo {
        presentation: ann.clone(),
        position: 3,
    });
    wait(&rx, "last slide", |s| pos(s) == Some(3));
    core.send(Command::StartLoop);
    wait(&rx, "looping", |s| s.looping.is_some());
    let s = wait(&rx, "finished", |s| s.looping.is_none());
    assert_eq!(pos(&s), Some(3), "doesn't wrap");
    assert!(s.notices.last().unwrap().text.contains("Loop finished"));
    std::fs::remove_dir_all(&dir).unwrap();
}
