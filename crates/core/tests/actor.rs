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
