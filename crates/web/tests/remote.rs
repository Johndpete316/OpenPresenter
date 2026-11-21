#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real server on 127.0.0.1, real HTTP and WebSocket, real core.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use pp6core::{Command, CoreConfig, ShowState};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use web::{Images, RemoteState, ShowInfo, WebConfig, WebRemote};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn start() -> (
    pp6core::CoreHandle,
    crossbeam_channel::Receiver<Arc<ShowState>>,
    WebRemote,
) {
    let (core, _) = pp6core::spawn(CoreConfig::default()).unwrap();
    let watch = core.subscribe(None);
    let remote = WebRemote::start(
        core.clone(),
        core.subscribe(None),
        &WebConfig {
            bind: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port: 0,
            pin_length: 4,
        },
        images(),
    )
    .unwrap();
    (core, watch, remote)
}

fn images() -> Images {
    let images = Images::default();
    let dir = fixture("fonts");
    let _ = images
        .fonts
        .set(std::sync::Mutex::new(render::Fonts::from_dirs(&[&dir])));
    images
}

fn wait(
    rx: &crossbeam_channel::Receiver<Arc<ShowState>>,
    pred: impl Fn(&ShowState) -> bool,
) -> Arc<ShowState> {
    loop {
        let s = rx.recv_timeout(Duration::from_secs(5)).expect("core state");
        if pred(&s) {
            return s;
        }
    }
}

struct Resp {
    status: u16,
    headers: String,
    body: String,
}

async fn http(
    url: &str,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Resp {
    let host = url.trim_start_matches("http://");
    let mut s = tokio::net::TcpStream::connect(host).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(c) = cookie {
        req.push_str(&format!("Cookie: {c}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            b.len()
        ));
    } else if method == "POST" {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    Resp {
        status,
        headers: head.to_owned(),
        body: body.to_owned(),
    }
}

fn cookie_from(r: &Resp) -> String {
    let line = r
        .headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
        .expect("cookie");
    let v = line.split_once(':').unwrap().1.trim();
    assert!(
        v.contains("HttpOnly") && v.contains("SameSite=Strict"),
        "{v}"
    );
    v.split(';').next().unwrap().to_owned()
}

fn wrong(pin: &str) -> &'static str {
    if pin == "0000" { "1111" } else { "0000" }
}

#[tokio::test]
async fn pin_login_lockout_commands_and_logout_on_new_pin() {
    let (core, watch, remote) = start();
    let url = remote.url().to_owned();

    // Not logged in: PIN page, API refused.
    let r = http(&url, "GET", "/", None, None).await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("Enter the PIN"));
    assert_eq!(
        http(&url, "POST", "/api/cmd/next", None, None).await.status,
        401
    );
    assert_eq!(
        http(&url, "GET", "/api/state", None, None).await.status,
        401
    );

    // Wrong PIN is rejected.
    let pin = remote.pin();
    let r = http(
        &url,
        "POST",
        "/login",
        None,
        Some(&format!("pin={}", wrong(&pin))),
    )
    .await;
    assert_eq!(r.status, 401);
    assert!(r.body.contains("Wrong PIN"));

    // Right PIN → session cookie → remote page.
    let r = http(&url, "POST", "/login", None, Some(&format!("pin={pin}"))).await;
    assert_eq!(r.status, 303);
    let cookie = cookie_from(&r);
    let r = http(&url, "GET", "/", Some(&cookie), None).await;
    assert!(r.body.contains("Next ▶"));

    // State and commands go through the real core.
    let song = fixture("song_1024x768.pro6");
    core.send(Command::GoTo {
        presentation: song.clone(),
        position: 0,
    });
    wait(&watch, |s| s.live.is_some());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let r = http(&url, "GET", "/api/state", Some(&cookie), None).await;
    let st: RemoteState = serde_json::from_str(&r.body).unwrap();
    let live = st.live.unwrap();
    assert_eq!(
        (live.title.as_str(), live.group.as_str(), live.position),
        ("song_1024x768", "Verse 1", 1)
    );
    assert_eq!(
        live.text,
        "Amazing grace, how sweet the sound\nThat saved a wretch like me"
    );
    assert!(st.next.unwrap().text.starts_with("I once was lost"));

    assert_eq!(
        http(&url, "POST", "/api/cmd/next", Some(&cookie), None)
            .await
            .status,
        204
    );
    wait(&watch, |s| {
        s.live.as_ref().and_then(|l| l.position) == Some(1)
    });
    assert_eq!(
        http(&url, "POST", "/api/cmd/clear", Some(&cookie), None)
            .await
            .status,
        204
    );
    let s = wait(&watch, |s| !s.text_visible);
    assert!(
        s.background_media.is_none(),
        "Clear from the phone is Clear All"
    );
    assert_eq!(
        http(&url, "POST", "/api/cmd/bogus", Some(&cookie), None)
            .await
            .status,
        404
    );

    // A new PIN logs every phone out.
    remote.regenerate_pin();
    assert_eq!(
        http(&url, "POST", "/api/cmd/next", Some(&cookie), None)
            .await
            .status,
        401
    );
    assert_eq!(
        http(&url, "GET", "/api/state", Some("pp6_session=forged"), None)
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn repeated_wrong_pins_lock_out() {
    let (_core, _watch, remote) = start();
    let url = remote.url().to_owned();
    let pin = remote.pin();
    for _ in 0..4 {
        assert_eq!(
            http(
                &url,
                "POST",
                "/login",
                None,
                Some(&format!("pin={}", wrong(&pin)))
            )
            .await
            .status,
            401
        );
    }
    let r = http(
        &url,
        "POST",
        "/login",
        None,
        Some(&format!("pin={}", wrong(&pin))),
    )
    .await;
    assert_eq!(r.status, 429);
    assert!(r.headers.to_ascii_lowercase().contains("retry-after: 30"));
    // Locked out: even the right PIN is refused for now.
    assert_eq!(
        http(&url, "POST", "/login", None, Some(&format!("pin={pin}")))
            .await
            .status,
        429
    );
}

#[tokio::test]
async fn websocket_pushes_state_and_rejects_without_session() {
    let (core, watch, remote) = start();
    let url = remote.url().to_owned();
    let host = url.trim_start_matches("http://").to_owned();

    // Without a session: closed with 4001.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{host}/ws"))
        .await
        .unwrap();
    match ws.next().await {
        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(Some(f)))) => {
            assert_eq!(u16::from(f.code), 4001)
        }
        other => panic!("expected close 4001, got {other:?}"),
    }

    let r = http(
        &url,
        "POST",
        "/login",
        None,
        Some(&format!("pin={}", remote.pin())),
    )
    .await;
    let cookie = cookie_from(&r);
    let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{host}/ws"),
    )
    .unwrap();
    req.headers_mut().insert("Cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let first = ws.next().await.unwrap().unwrap().into_text().unwrap();
    let st: RemoteState = serde_json::from_str(&first).unwrap();
    assert!(st.live.is_none());
    let second = ws.next().await.unwrap().unwrap().into_text().unwrap();
    let show: ShowInfo = serde_json::from_str(&second).unwrap();
    assert!(show.playlists.is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(remote.clients(), 1);

    // A change pushes a new state.
    core.send(Command::GoTo {
        presentation: fixture("song_1024x768.pro6"),
        position: 2,
    });
    wait(&watch, |s| s.live.is_some());
    let pushed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let msg = ws.next().await.unwrap().unwrap().into_text().unwrap();
            if msg.starts_with(r#"{"playlists""#) {
                continue; // the playlists message
            }
            let st: RemoteState = serde_json::from_str(&msg).unwrap();
            if let Some(l) = st.live {
                return l;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((pushed.group.as_str(), pushed.position), ("Chorus", 3));
    ws.close(None).await.unwrap();
    let _ = ws.next().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(remote.clients(), 0);
    drop(remote); // stops the server cleanly
}

async fn logged_in(remote: &WebRemote) -> (String, String) {
    let url = remote.url().to_owned();
    let r = http(
        &url,
        "POST",
        "/login",
        None,
        Some(&format!("pin={}", remote.pin())),
    )
    .await;
    (url, cookie_from(&r))
}

#[tokio::test]
async fn slide_images_are_rendered_and_cached() {
    let (core, watch, remote) = start();
    let (url, cookie) = logged_in(&remote).await;
    core.send(Command::GoTo {
        presentation: fixture("render_1024x768.pro6"),
        position: 0,
    });
    wait(&watch, |s| s.live.is_some());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let st: RemoteState = serde_json::from_str(
        &http(&url, "GET", "/api/state", Some(&cookie), None)
            .await
            .body,
    )
    .unwrap();
    let live = st.live.unwrap();
    assert!(live.image.starts_with("/img/s/"), "{}", live.image);
    let next = st.next.unwrap();
    assert_eq!(next.position, 2);
    assert_ne!(next.image, live.image);

    let img = format!("{}?w=320", live.image);
    assert_eq!(http(&url, "GET", &img, None, None).await.status, 401);
    for _ in 0..2 {
        let r = http(&url, "GET", &img, Some(&cookie), None).await;
        assert_eq!(r.status, 200);
        let h = r.headers.to_ascii_lowercase();
        assert!(h.contains("content-type: image/png"), "{h}");
        assert!(h.contains("immutable"), "{h}");
        assert!(r.body.contains("PNG"));
    }
    assert_eq!(
        http(&url, "GET", "/img/s/nope/0", Some(&cookie), None)
            .await
            .status,
        404
    );
    // A slide whose font isn't installed: an error the page shows as text instead.
    core.send(Command::GoTo {
        presentation: fixture("song_1024x768.pro6"),
        position: 0,
    });
    wait(&watch, |s| {
        s.live
            .as_ref()
            .is_some_and(|l| l.path.ends_with("song_1024x768.pro6"))
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let song: RemoteState = serde_json::from_str(
        &http(&url, "GET", "/api/state", Some(&cookie), None)
            .await
            .body,
    )
    .unwrap();
    let r = http(&url, "GET", &song.live.unwrap().image, Some(&cookie), None).await;
    assert_eq!(r.status, 500);
    assert!(r.body.contains("Arial"), "{}", r.body);
    // No background media: no poster.
    assert!(st.background.is_none());
}

#[tokio::test]
async fn images_wait_for_fonts() {
    let (core, _) = pp6core::spawn(CoreConfig::default()).unwrap();
    let watch = core.subscribe(None);
    let remote = WebRemote::start(
        core.clone(),
        core.subscribe(None),
        &WebConfig {
            bind: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port: 0,
            pin_length: 4,
        },
        Images::default(),
    )
    .unwrap();
    let (url, cookie) = logged_in(&remote).await;
    core.send(Command::GoTo {
        presentation: fixture("song_1024x768.pro6"),
        position: 0,
    });
    wait(&watch, |s| s.live.is_some());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let st: RemoteState = serde_json::from_str(
        &http(&url, "GET", "/api/state", Some(&cookie), None)
            .await
            .body,
    )
    .unwrap();
    let r = http(&url, "GET", &st.live.unwrap().image, Some(&cookie), None).await;
    assert_eq!(r.status, 503);
    assert!(r.headers.to_ascii_lowercase().contains("retry-after: 1"));
}

#[tokio::test]
async fn playlists_browse_load_and_go_to_without_touching_the_operator() {
    let (core, watch, remote) = start();
    let (url, cookie) = logged_in(&remote).await;
    let song = fixture("song_1024x768.pro6");
    core.send(Command::PlaylistCreate("Sunday".into()));
    core.send(Command::PlaylistAdd {
        playlist: 0,
        path: song.clone(),
        arrangement: None,
    });
    wait(&watch, |s| {
        s.playlists
            .first()
            .is_some_and(|p| !p.playlist.items.is_empty())
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let show: ShowInfo = serde_json::from_str(
        &http(&url, "GET", "/api/show", Some(&cookie), None)
            .await
            .body,
    )
    .unwrap();
    assert_eq!(show.playlists[0].name, "Sunday");
    let item = &show.playlists[0].items[0];
    assert_eq!(item.title, "song_1024x768");
    assert!(!item.id.contains('/'), "paths never reach the phone");

    // Loading for the phone doesn't change what the operator is looking at.
    let id = item.id.clone();
    let load = format!("/api/load/{id}");
    assert_eq!(
        http(&url, "POST", &load, Some(&cookie), None).await.status,
        204
    );
    let s = wait(&watch, |s| s.docs.contains_key(&song));
    assert!(s.selected.is_none());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let show: ShowInfo = serde_json::from_str(
        &http(&url, "GET", "/api/show", Some(&cookie), None)
            .await
            .body,
    )
    .unwrap();
    let p = &show.presentations[&id];
    assert!(p.loaded);
    assert!(p.slides.len() >= 3);
    assert_eq!(p.slides[0].group, "Verse 1");
    assert!(p.slides[0].image.starts_with("/img/s/"));

    // Go to slide 3 (1-based).
    let go = format!("/api/goto/{id}/3");
    assert_eq!(
        http(&url, "POST", &go, Some(&cookie), None).await.status,
        204
    );
    let s = wait(&watch, |s| s.live.is_some());
    assert_eq!(s.live.as_ref().unwrap().position, Some(2));
    assert!(s.selected.is_none());

    for bad in [
        format!("/api/goto/{id}/0"),
        "/api/goto/pnope/1".to_owned(),
        "/api/load/pnope".to_owned(),
    ] {
        assert_eq!(
            http(&url, "POST", &bad, Some(&cookie), None).await.status,
            404,
            "{bad}"
        );
    }

    // The extra desktop commands.
    assert_eq!(
        http(&url, "POST", "/api/cmd/clear-text", Some(&cookie), None)
            .await
            .status,
        204
    );
    wait(&watch, |s| !s.text_visible);
    assert_eq!(
        http(&url, "POST", "/api/cmd/output-off", Some(&cookie), None)
            .await
            .status,
        204
    );
    wait(&watch, |s| !s.output_enabled);
}
