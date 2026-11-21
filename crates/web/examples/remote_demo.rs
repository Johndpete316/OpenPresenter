//! Serve the phone remote against the fixtures, for trying the page in a browser:
//! `cargo run -p web --example remote_demo [port] [background-image]`. Prints the
//! URL and PIN.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Mutex;

use opcore::{Command, CoreConfig};
use web::{Images, WebConfig, WebRemote};

fn main() {
    let port = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(8686);
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let (core, _) = opcore::spawn(CoreConfig::default()).unwrap();
    let poster: web::PosterFn = std::sync::Arc::new(|path: &std::path::Path, w: u32| {
        let img = image::open(path).map_err(|e| e.to_string())?;
        Ok(img.thumbnail(w, w * 9 / 16).to_rgba8())
    });
    let images = Images {
        poster: Some(poster),
        ..Images::default()
    };
    let mut fonts = render::Fonts::system();
    fonts.add_dir(&fixtures.join("fonts"));
    let _ = images.fonts.set(Mutex::new(fonts));
    let remote = WebRemote::start(
        core.clone(),
        core.subscribe(None),
        &WebConfig {
            bind: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port,
            pin_length: 4,
        },
        images,
    )
    .unwrap();
    core.send(Command::PlaylistCreate("Sunday".into()));
    for f in [
        "render_1024x768.pro6",
        "two_fonts_1920x1080.pro6",
        "song_1024x768.pro6",
    ] {
        core.send(Command::PlaylistAdd {
            playlist: 0,
            path: fixtures.join(f),
            arrangement: None,
        });
    }
    core.send(Command::PlaylistCreate("Announcements".into()));
    core.send(Command::PlaylistAdd {
        playlist: 1,
        path: fixtures.join("announcements_synthetic.pro6"),
        arrangement: None,
    });
    core.send(Command::GoTo {
        presentation: fixtures.join("render_1024x768.pro6"),
        position: 0,
    });
    if let Some(bg) = std::env::args().nth(2) {
        core.send(Command::SetBackground {
            media: Some(bg.into()),
            next: None,
        });
    }
    println!("{}  PIN {}", remote.url(), remote.pin());
    std::thread::park();
}
