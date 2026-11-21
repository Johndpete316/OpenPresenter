//! Phone remote. An axum server on its own thread (own tokio runtime), bound to the
//! PC's LAN address. A PIN shown on the operator screen logs a phone in (session
//! cookie); the page shows the current and next slide (rendered images) and sends
//! the same `Command`s as the clicker. On a wide screen it also shows the playlists
//! and a slide grid. State changes are pushed over a WebSocket.

mod auth;
mod images;
mod page;
mod state;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt as _, StreamExt as _};
use pp6core::{Command, CoreHandle, ShowState};
use serde::Deserialize;
use tokio::sync::{oneshot, watch};

pub use auth::{Auth, LoginResult, new_pin};
pub use images::{Images, PosterFn};
pub use state::{
    ItemInfo, LiveInfo, NextInfo, PlaylistInfo, PresentationInfo, RemoteState, ShowInfo, SlideInfo,
    slide_text,
};

const COOKIE: &str = "pp6_session";

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("couldn't listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("couldn't start the web server: {0}")]
    Start(String),
}

impl WebError {
    pub fn user_message(&self) -> String {
        match self {
            WebError::Bind { addr, source } if source.kind() == std::io::ErrorKind::AddrInUse => {
                format!(
                    "The phone remote couldn't start: port {} is already in use. Choose another port in Settings. Everything else works.",
                    addr.port()
                )
            }
            WebError::Bind { addr, .. } => format!(
                "The phone remote couldn't start on {addr}. Check the network settings. Everything else works."
            ),
            WebError::Start(_) => "The phone remote couldn't start. Everything else works.".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Address to listen on; `None` = this PC's LAN address.
    pub bind: Option<IpAddr>,
    pub port: u16,
    /// 4–6 digits.
    pub pin_length: u8,
}

struct Shared {
    core: CoreHandle,
    auth: Mutex<Auth>,
    /// Live/next JSON ([`RemoteState`]).
    state: watch::Receiver<Arc<String>>,
    /// Playlists JSON ([`ShowInfo`]).
    show: watch::Receiver<Arc<String>>,
    registry: Arc<Mutex<state::Registry>>,
    images: Images,
    cache: Mutex<images::Cache>,
    clients: AtomicUsize,
}

/// Running phone remote. Dropping it stops the server.
pub struct WebRemote {
    shared: Arc<Shared>,
    url: String,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// This PC's LAN IPv4 address (no packets are sent: a UDP "connect" just picks a route).
pub fn lan_address() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

impl WebRemote {
    /// Start the server. `snapshots` is a core subscription.
    pub fn start(
        core: CoreHandle,
        snapshots: crossbeam_channel::Receiver<Arc<ShowState>>,
        config: &WebConfig,
        images: Images,
    ) -> Result<WebRemote, WebError> {
        let ip = config
            .bind
            .or_else(lan_address)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let addr = SocketAddr::new(ip, config.port);
        let listener =
            std::net::TcpListener::bind(addr).map_err(|source| WebError::Bind { addr, source })?;
        listener
            .set_nonblocking(true)
            .map_err(|e| WebError::Start(e.to_string()))?;
        let local = listener
            .local_addr()
            .map_err(|e| WebError::Start(e.to_string()))?;
        let url = format!("http://{local}");

        let (state_tx, state_rx) = watch::channel(Arc::new(String::from("{}")));
        let (show_tx, show_rx) = watch::channel(Arc::new(String::from(
            r#"{"playlists":[],"presentations":{}}"#,
        )));
        let registry = Arc::new(Mutex::new(state::Registry::new()));
        let shared = Arc::new(Shared {
            core,
            auth: Mutex::new(Auth::new(config.pin_length)),
            state: state_rx,
            show: show_rx,
            registry: registry.clone(),
            images,
            cache: Mutex::new(images::Cache::default()),
            clients: AtomicUsize::new(0),
        });
        // Bridge core snapshots (crossbeam) into the async world, sending only real changes.
        std::thread::Builder::new()
            .name("web-state".into())
            .spawn(move || {
                let mut last = String::new();
                let mut last_show = String::new();
                let mut inputs: Option<state::ShowInputs> = None;
                while let Ok(mut s) = snapshots.recv() {
                    while let Ok(newer) = snapshots.try_recv() {
                        s = newer;
                    }
                    let Ok(mut reg) = registry.lock() else { return };
                    let json = serde_json::to_string(&RemoteState::build(&s, &mut reg))
                        .unwrap_or_default();
                    let now = state::ShowInputs::of(&s);
                    let show = if inputs.as_ref().is_some_and(|i| i.same(&now)) {
                        None
                    } else {
                        inputs = Some(now);
                        Some(
                            serde_json::to_string(&ShowInfo::build(&s, &mut reg))
                                .unwrap_or_default(),
                        )
                    };
                    reg.prune();
                    drop(reg);
                    if let Some(show) = show
                        && show != last_show
                    {
                        last_show.clone_from(&show);
                        if show_tx.send(Arc::new(show)).is_err() {
                            return;
                        }
                    }
                    if json != last {
                        last.clone_from(&json);
                        if state_tx.send(Arc::new(json)).is_err() {
                            return;
                        }
                    }
                }
            })
            .map_err(|e| WebError::Start(e.to_string()))?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let app = router(shared.clone());
        let thread = std::thread::Builder::new()
            .name("web".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!(error = %e, "web runtime failed to start");
                        return;
                    }
                };
                rt.block_on(async move {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(l) => l,
                        Err(e) => {
                            tracing::error!(error = %e, "web listener failed");
                            return;
                        }
                    };
                    let service = app.into_make_service_with_connect_info::<SocketAddr>();
                    let result = axum::serve(listener, service)
                        .with_graceful_shutdown(async {
                            let _ = shutdown_rx.await;
                        })
                        .await;
                    if let Err(e) = result {
                        tracing::error!(error = %e, "web server stopped");
                    }
                });
            })
            .map_err(|e| WebError::Start(e.to_string()))?;
        tracing::info!(%url, "phone remote listening");
        Ok(WebRemote {
            shared,
            url,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    /// Address phones should open, e.g. `http://192.168.1.20:8686`.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn pin(&self) -> String {
        self.shared
            .auth
            .lock()
            .map(|a| a.pin().to_owned())
            .unwrap_or_default()
    }

    /// New PIN; connected phones must log in again.
    pub fn regenerate_pin(&self) {
        if let Ok(mut a) = self.shared.auth.lock() {
            a.regenerate();
        }
        tracing::info!("phone remote PIN regenerated; all phones logged out");
    }

    /// Phones currently connected (open WebSockets).
    pub fn clients(&self) -> usize {
        self.shared.clients.load(Ordering::Relaxed)
    }
}

impl Drop for WebRemote {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/api/state", get(api_state))
        .route("/api/show", get(api_show))
        .route("/api/cmd/{name}", post(api_cmd))
        .route("/api/goto/{id}/{position}", post(api_goto))
        .route("/api/load/{id}", post(api_load))
        .route("/img/s/{key}/{flat}", get(slide_image))
        .route("/img/bg/{key}", get(background_image))
        .route("/ws", get(ws))
        .with_state(shared)
}

fn session(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_owned())
}

fn authed(shared: &Shared, headers: &HeaderMap) -> bool {
    session(headers).is_some_and(|t| shared.auth.lock().is_ok_and(|a| a.is_session(&t)))
}

fn no_store(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

async fn index(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if authed(&shared, &headers) {
        no_store(Html(page::REMOTE).into_response())
    } else {
        no_store(Html(page::login(None)).into_response())
    }
}

#[derive(Deserialize)]
struct LoginForm {
    pin: String,
}

async fn login(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Form(form): Form<LoginForm>,
) -> Response {
    let result = match shared.auth.lock() {
        Ok(mut a) => a.login(peer.ip(), &form.pin, Instant::now()),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match result {
        LoginResult::Ok(token) => {
            tracing::info!(peer = %peer.ip(), "phone remote: login");
            let mut r = Redirect::to("/").into_response();
            let cookie = format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/");
            if let Ok(v) = HeaderValue::from_str(&cookie) {
                r.headers_mut().insert(header::SET_COOKIE, v);
            }
            no_store(r)
        }
        LoginResult::WrongPin { attempts_left } => {
            tracing::warn!(peer = %peer.ip(), attempts_left, "phone remote: wrong PIN");
            let msg = format!("Wrong PIN. {attempts_left} attempt(s) left before a short lockout.");
            no_store((StatusCode::UNAUTHORIZED, Html(page::login(Some(&msg)))).into_response())
        }
        LoginResult::LockedOut { retry_after } => {
            tracing::warn!(peer = %peer.ip(), retry_after_s = retry_after.as_secs(), "phone remote: locked out");
            let msg = format!(
                "Too many wrong PINs. Try again in {} seconds.",
                retry_after.as_secs().max(1)
            );
            let mut r =
                (StatusCode::TOO_MANY_REQUESTS, Html(page::login(Some(&msg)))).into_response();
            if let Ok(v) = HeaderValue::from_str(&retry_after.as_secs().max(1).to_string()) {
                r.headers_mut().insert(header::RETRY_AFTER, v);
            }
            no_store(r)
        }
    }
}

async fn logout(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if let (Some(t), Ok(mut a)) = (session(&headers), shared.auth.lock()) {
        a.logout(&t);
    }
    Redirect::to("/").into_response()
}

async fn api_state(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let json = shared.state.borrow().clone();
    no_store(
        (
            [(header::CONTENT_TYPE, "application/json")],
            (*json).clone(),
        )
            .into_response(),
    )
}

async fn api_show(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let json = shared.show.borrow().clone();
    no_store(
        (
            [(header::CONTENT_TYPE, "application/json")],
            (*json).clone(),
        )
            .into_response(),
    )
}

/// A presentation the page may name: one in a playlist, or the live one.
fn presentation_path(shared: &Shared, id: &str) -> Option<std::path::PathBuf> {
    shared
        .registry
        .lock()
        .ok()
        .and_then(|r| r.presentations.get(id).cloned())
}

/// Put slide `position` (1-based, play order) of a presentation live.
async fn api_goto(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path((id, position)): Path<(String, usize)>,
) -> Response {
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(path) = presentation_path(&shared, &id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(position) = position.checked_sub(1) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    tracing::info!(path = %path.display(), position, "phone remote: go to slide");
    shared.core.send(Command::GoTo {
        presentation: path,
        position,
    });
    StatusCode::NO_CONTENT.into_response()
}

/// Load a presentation so its slides can be shown (the operator's screen is untouched).
async fn api_load(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(path) = presentation_path(&shared, &id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    shared.core.send(Command::Preload(path));
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct ImageQuery {
    w: Option<u32>,
}

fn image_response(content_type: &'static str, cache: &'static str, data: &[u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        data.to_vec(),
    )
        .into_response()
}

fn not_ready() -> Response {
    let mut r = (StatusCode::SERVICE_UNAVAILABLE, "fonts are still loading").into_response();
    r.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    no_store(r)
}

/// Slide image. Keys are unique per loaded document, so the browser may keep it.
async fn slide_image(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path((key, flat)): Path<(String, usize)>,
    Query(q): Query<ImageQuery>,
) -> Response {
    const CACHE: &str = "private, max-age=31536000, immutable";
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let w = images::width(q.w);
    let cache_key = format!("s/{key}/{flat}");
    if let Some(d) = shared.cache.lock().ok().and_then(|c| c.get(&cache_key, w)) {
        return image_response("image/png", CACHE, &d);
    }
    let doc = shared
        .registry
        .lock()
        .ok()
        .and_then(|r| r.docs.get(&key).and_then(std::sync::Weak::upgrade));
    let Some(doc) = doc else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let fonts = shared.images.fonts.clone();
    let made = tokio::task::spawn_blocking(move || {
        let f = fonts.get()?;
        let mut f = f.lock().ok()?;
        Some(images::slide_png(&mut f, &doc, flat, w))
    })
    .await;
    match made {
        Ok(Some(Ok(png))) => {
            let png = Arc::new(png);
            if let Ok(mut c) = shared.cache.lock() {
                c.put(cache_key, w, png.clone());
            }
            image_response("image/png", CACHE, &png)
        }
        Ok(None) => not_ready(),
        Ok(Some(Err(e))) => {
            tracing::debug!(error = %e, "phone remote: slide image failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Poster frame of the background media.
async fn background_image(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(q): Query<ImageQuery>,
) -> Response {
    const CACHE: &str = "private, max-age=86400";
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let w = images::width(q.w);
    let cache_key = format!("bg/{key}");
    if let Some(d) = shared.cache.lock().ok().and_then(|c| c.get(&cache_key, w)) {
        return image_response("image/jpeg", CACHE, &d);
    }
    let path = shared
        .registry
        .lock()
        .ok()
        .and_then(|r| r.media.get(&key).cloned());
    let (Some(path), Some(poster)) = (path, shared.images.poster.clone()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let made = tokio::task::spawn_blocking(move || images::poster_jpeg(&poster, &path, w)).await;
    match made {
        Ok(Ok(jpg)) => {
            let jpg = Arc::new(jpg);
            if let Ok(mut c) = shared.cache.lock() {
                c.put(cache_key, w, jpg.clone());
            }
            image_response("image/jpeg", CACHE, &jpg)
        }
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "phone remote: background poster failed");
            StatusCode::NOT_FOUND.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn api_cmd(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if !authed(&shared, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let cmd = match name.as_str() {
        "next" => Command::Next,
        "prev" => Command::Prev,
        "clear" => Command::ClearAll,
        "clear-text" => Command::ClearText,
        "clear-bg" => Command::ClearBackground,
        "output-on" => Command::OutputEnable(true),
        "output-off" => Command::OutputEnable(false),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    tracing::info!(?cmd, "phone remote command");
    shared.core.send(cmd);
    StatusCode::NO_CONTENT.into_response()
}

async fn ws(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let ok = authed(&shared, &headers);
    upgrade.on_upgrade(move |socket| async move {
        if ok {
            push(socket, shared).await;
        } else {
            // 4001 tells the page to show the PIN screen again.
            let mut socket = socket;
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: 4001,
                    reason: "login required".into(),
                })))
                .await;
        }
    })
}

async fn push(socket: WebSocket, shared: Arc<Shared>) {
    shared.clients.fetch_add(1, Ordering::Relaxed);
    let (mut tx, mut rx) = socket.split();
    let mut state = shared.state.clone();
    let mut show = shared.show.clone();
    let initial = state.borrow_and_update().clone();
    let initial_show = show.borrow_and_update().clone();
    if tx
        .send(Message::Text((*initial).clone().into()))
        .await
        .is_ok()
        && tx
            .send(Message::Text((*initial_show).clone().into()))
            .await
            .is_ok()
    {
        loop {
            tokio::select! {
                changed = show.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let json = show.borrow_and_update().clone();
                    if tx.send(Message::Text((*json).clone().into())).await.is_err() {
                        break;
                    }
                }
                changed = state.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let json = state.borrow_and_update().clone();
                    if tx.send(Message::Text((*json).clone().into())).await.is_err() {
                        break;
                    }
                }
                msg = rx.next() => match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    shared.clients.fetch_sub(1, Ordering::Relaxed);
}
