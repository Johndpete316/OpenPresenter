//! The core actor: a single thread that owns `ShowState`. All mutation goes through
//! `Command`s; each change is broadcast as an `Arc<ShowState>` snapshot. Disk work
//! (presentations, playlists) happens on an I/O thread, so the core never blocks.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use pro6::Document;

use crate::command::Command;
use crate::order::{SlideRef, find_nearest, next_enabled, prev_enabled, slide_order};
use crate::playlist::{Playlist, PlaylistItem};
use crate::restore::{LiveRestore, RestoreState};
use crate::state::{Live, Notice, NoticeLevel, ShowState};

/// Presentations kept loaded (beyond the live one and the playlist's).
const DOC_CACHE: usize = 40;
const NOTICES: usize = 50;

type Notify = Box<dyn Fn() + Send>;

enum Msg {
    Command(Command, Instant),
    Subscribe(Sender<Arc<ShowState>>, Option<Notify>),
    Loaded(PathBuf, Result<Arc<Document>, String>),
    PlaylistLoaded(PathBuf, Result<Playlist, String>),
    PlaylistSaved(PathBuf, Result<(), String>),
    Saved(PathBuf, Result<(), String>),
    /// Publish a final snapshot and stop.
    Shutdown,
}

enum IoJob {
    Load(PathBuf),
    LoadPlaylist(PathBuf),
    SavePlaylist(Playlist, PathBuf),
    SaveDocument(Arc<Document>),
}

/// Handle used by every command source.
#[derive(Clone)]
pub struct CoreHandle {
    tx: Sender<Msg>,
}

impl CoreHandle {
    /// Send a command. Never blocks.
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Msg::Command(cmd, Instant::now()));
    }

    /// Receive every new snapshot. `notify` is called after each send (e.g. to wake a UI).
    pub fn subscribe(&self, notify: Option<Box<dyn Fn() + Send>>) -> Receiver<Arc<ShowState>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let _ = self.tx.send(Msg::Subscribe(tx, notify));
        rx
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

#[derive(Default)]
pub struct CoreConfig {
    pub restore: Option<RestoreState>,
    pub restore_writer: Option<Sender<RestoreState>>,
    pub output_enabled: bool,
}

pub fn spawn(config: CoreConfig) -> std::io::Result<(CoreHandle, JoinHandle<()>)> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let (io_tx, io_rx) = crossbeam_channel::unbounded::<IoJob>();
    let back = tx.clone();
    std::thread::Builder::new()
        .name("core-io".into())
        .spawn(move || io_worker(&io_rx, &back))?;
    let handle = std::thread::Builder::new()
        .name("core".into())
        .spawn(move || {
            let mut core = Core::new(io_tx, config);
            core.run(&rx);
        })?;
    Ok((CoreHandle { tx }, handle))
}

fn io_worker(rx: &Receiver<IoJob>, back: &Sender<Msg>) {
    while let Ok(job) = rx.recv() {
        let msg = match job {
            IoJob::Load(path) => {
                let r = pro6::load(&path)
                    .map(Arc::new)
                    .map_err(|e| e.user_message());
                Msg::Loaded(path, r)
            }
            IoJob::LoadPlaylist(path) => {
                let r = Playlist::load(&path).map_err(|e| e.user_message());
                Msg::PlaylistLoaded(path, r)
            }
            IoJob::SaveDocument(doc) => {
                let path = doc.path.clone();
                Msg::Saved(path, save_with_backup(&doc))
            }
            IoJob::SavePlaylist(p, path) => {
                let r = p.save(&path).map_err(|e| e.user_message());
                Msg::PlaylistSaved(path, r)
            }
        };
        if back.send(msg).is_err() {
            return;
        }
    }
}

/// Keep the previous version as `<name>.pro6.bak` (replacing an older backup), then
/// save atomically.
pub fn save_with_backup(doc: &Document) -> Result<(), String> {
    let path = &doc.path;
    if path.exists() {
        let mut name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".bak");
        let bak = path.with_file_name(name);
        if let Err(e) = std::fs::copy(path, &bak) {
            return Err(format!(
                "'{}' wasn't saved: the backup copy couldn't be made ({e}). Your edits are still open; nothing on disk changed.",
                path.display()
            ));
        }
    }
    pro6::save(doc, path).map_err(|e| e.user_message())
}

struct PendingGoTo {
    path: PathBuf,
    /// `None` = the first enabled slide (known once the presentation is loaded).
    position: Option<usize>,
    at: Option<Instant>,
    text_visible: bool,
    background_visible: bool,
}

struct Core {
    s: ShowState,
    subscribers: Vec<(Sender<Arc<ShowState>>, Option<Notify>)>,
    io: Sender<IoJob>,
    loading: Vec<PathBuf>,
    doc_lru: VecDeque<PathBuf>,
    pending_goto: Option<PendingGoTo>,
    restore_writer: Option<Sender<RestoreState>>,
}

impl Core {
    fn new(io: Sender<IoJob>, config: CoreConfig) -> Core {
        let mut core = Core {
            s: ShowState {
                output_enabled: config.output_enabled,
                ..ShowState::default()
            },
            subscribers: Vec::new(),
            io,
            loading: Vec::new(),
            doc_lru: VecDeque::new(),
            pending_goto: None,
            restore_writer: config.restore_writer,
        };
        if let Some(r) = config.restore {
            core.apply_restore(r);
        }
        core
    }

    fn apply_restore(&mut self, r: RestoreState) {
        tracing::info!(
            age_s = r.age().as_secs(),
            live = r.restore_live(),
            "restoring previous session"
        );
        let go_live = r.restore_live();
        if go_live {
            self.s.background_media = r.background_media.clone();
        }
        self.s.playlist = Arc::new(r.playlist);
        self.s.playlist_path = r.playlist_path;
        if let Some(i) = r.playlist_item {
            self.select_item(i);
        }
        if let Some(l) = r.live {
            Arc::make_mut(&mut self.s.arrangements).insert(l.path.clone(), l.arrangement.clone());
            self.s.selected = Some(l.path.clone());
            self.request_load(&l.path);
            if go_live {
                self.pending_goto = Some(PendingGoTo {
                    path: l.path,
                    position: Some(l.position),
                    at: None,
                    text_visible: l.text_visible,
                    background_visible: l.background_visible,
                });
            }
        }
    }

    fn run(&mut self, rx: &Receiver<Msg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Command(cmd, at) => {
                    let _span = tracing::info_span!("command", ?cmd).entered();
                    self.s.command_at = Some(at);
                    self.command(cmd, at);
                }
                Msg::Subscribe(tx, notify) => {
                    let _ = tx.send(Arc::new(self.s.clone()));
                    if let Some(n) = &notify {
                        n();
                    }
                    self.subscribers.push((tx, notify));
                    continue;
                }
                Msg::Loaded(path, result) => self.loaded(path, result),
                Msg::PlaylistLoaded(path, result) => match result {
                    Ok(p) => {
                        self.s.playlist = Arc::new(p);
                        self.s.playlist_path = Some(path);
                        self.s.playlist_item = None;
                        self.preload_playlist_head();
                    }
                    Err(e) => self.notice(NoticeLevel::Error, e),
                },
                Msg::Saved(path, result) => match result {
                    Ok(()) => {
                        Arc::make_mut(&mut self.s.unsaved).remove(&path);
                        self.notice(NoticeLevel::Info, format!("Saved {}", path.display()));
                    }
                    Err(e) => self.notice(NoticeLevel::Error, e),
                },
                Msg::PlaylistSaved(path, result) => match result {
                    Ok(()) => {
                        self.s.playlist_path = Some(path.clone());
                        self.notice(
                            NoticeLevel::Info,
                            format!("Playlist saved to {}", path.display()),
                        );
                    }
                    Err(e) => self.notice(NoticeLevel::Error, e),
                },
                Msg::Shutdown => {
                    self.publish();
                    return;
                }
            }
            self.publish();
        }
    }

    fn publish(&mut self) {
        self.s.seq += 1;
        let snap = Arc::new(self.s.clone());
        self.subscribers.retain(|(tx, notify)| {
            let ok = tx.send(snap.clone()).is_ok();
            if ok && let Some(n) = notify {
                n();
            }
            ok
        });
        if let Some(w) = &self.restore_writer {
            let live = self.s.live.as_ref().map(|l| LiveRestore {
                path: l.path.clone(),
                arrangement: l.arrangement.clone(),
                position: l.position.unwrap_or(0),
                text_visible: self.s.text_visible,
                background_visible: self.s.background_visible,
            });
            let _ = w.send(RestoreState {
                saved_at_unix: 0,
                playlist_path: self.s.playlist_path.clone(),
                playlist: (*self.s.playlist).clone(),
                playlist_item: self.s.playlist_item,
                live,
                background_media: self.s.background_media.clone(),
            });
        }
    }

    fn notice(&mut self, level: NoticeLevel, text: String) {
        match level {
            NoticeLevel::Error => tracing::error!(%text, "notice"),
            NoticeLevel::Warning => tracing::warn!(%text, "notice"),
            NoticeLevel::Info => tracing::info!(%text, "notice"),
        }
        let n = Arc::make_mut(&mut self.s.notices);
        n.push(Notice {
            level,
            text,
            at: Instant::now(),
        });
        if n.len() > NOTICES {
            let excess = n.len() - NOTICES;
            n.drain(..excess);
        }
    }

    fn request_load(&mut self, path: &PathBuf) {
        if self.s.docs.contains_key(path) {
            self.touch(path);
            return;
        }
        if !self.loading.contains(path) {
            self.loading.push(path.clone());
            let _ = self.io.send(IoJob::Load(path.clone()));
        }
    }

    fn touch(&mut self, path: &PathBuf) {
        self.doc_lru.retain(|p| p != path);
        self.doc_lru.push_back(path.clone());
    }

    fn loaded(&mut self, path: PathBuf, result: Result<Arc<Document>, String>) {
        self.loading.retain(|p| p != &path);
        match result {
            Ok(doc) => {
                Arc::make_mut(&mut self.s.load_errors).remove(&path);
                for w in &doc.warnings {
                    tracing::debug!(file = %path.display(), path = %w.xml_path, msg = %w.message, "parse warning");
                }
                Arc::make_mut(&mut self.s.docs).insert(path.clone(), doc);
                self.touch(&path);
                self.evict();
                if self.pending_goto.as_ref().is_some_and(|p| p.path == path)
                    && let Some(p) = self.pending_goto.take()
                {
                    let position = p.position.or_else(|| {
                        self.s
                            .order_for(&p.path)
                            .and_then(|o| next_enabled(&o, None))
                    });
                    match position {
                        Some(pos) => {
                            self.go_live(&p.path, pos, p.at);
                            self.s.text_visible = p.text_visible;
                            self.s.background_visible = p.background_visible;
                        }
                        None => self.notice(
                            NoticeLevel::Warning,
                            format!("'{}' has no enabled slides to show.", p.path.display()),
                        ),
                    }
                }
            }
            Err(e) => {
                Arc::make_mut(&mut self.s.load_errors).insert(path.clone(), e.clone());
                if self.pending_goto.as_ref().is_some_and(|p| p.path == path) {
                    self.pending_goto = None;
                }
                self.notice(NoticeLevel::Error, e);
            }
        }
    }

    /// Drop least-recently-used presentations that aren't live, selected, in the playlist,
    /// or holding unsaved edits.
    fn evict(&mut self) {
        while self.doc_lru.len() > DOC_CACHE {
            let keep = |p: &PathBuf, s: &ShowState| {
                s.is_live(p)
                    || s.unsaved.contains(p)
                    || s.selected.as_ref() == Some(p)
                    || s.playlist.items.iter().any(|i| &i.path == p)
            };
            let Some(pos) = self.doc_lru.iter().position(|p| !keep(p, &self.s)) else {
                break;
            };
            if let Some(p) = self.doc_lru.remove(pos) {
                Arc::make_mut(&mut self.s.docs).remove(&p);
            }
        }
    }

    fn command(&mut self, cmd: Command, at: Instant) {
        match cmd {
            Command::Next => self.step(true, at),
            Command::Prev => self.step(false, at),
            Command::ClearText => self.s.text_visible = false,
            Command::ClearBackground => {
                self.s.background_visible = false;
                self.s.background_media = None;
            }
            Command::ClearAll => {
                self.s.text_visible = false;
                self.s.background_visible = false;
                self.s.background_media = None;
            }
            Command::SetBackground { media, next } => {
                if media.is_some() {
                    self.s.background_visible = true;
                }
                self.s.background_media = media;
                self.s.background_next = next;
            }
            Command::GoTo {
                presentation,
                position,
            } => {
                if self.s.docs.contains_key(&presentation) {
                    self.go_live(&presentation, position, Some(at));
                } else {
                    self.request_load(&presentation);
                    self.pending_goto = Some(PendingGoTo {
                        path: presentation,
                        position: Some(position),
                        at: Some(at),
                        text_visible: true,
                        background_visible: self.s.background_visible,
                    });
                }
            }
            Command::SelectArrangement {
                presentation,
                arrangement,
            } => {
                self.select_arrangement(presentation, arrangement);
            }
            Command::Open(path) => {
                self.s.selected = Some(path.clone());
                self.request_load(&path);
            }
            Command::OutputEnable(on) => self.s.output_enabled = on,
            Command::PlaylistNew => {
                self.s.playlist = Arc::new(Playlist::default());
                self.s.playlist_path = None;
                self.s.playlist_item = None;
            }
            Command::PlaylistLoad(path) => {
                let _ = self.io.send(IoJob::LoadPlaylist(path));
            }
            Command::PlaylistSave(path) => match path.or_else(|| self.s.playlist_path.clone()) {
                Some(p) => {
                    let _ = self
                        .io
                        .send(IoJob::SavePlaylist((*self.s.playlist).clone(), p));
                }
                None => self.notice(
                    NoticeLevel::Warning,
                    "Choose where to save the playlist first.".into(),
                ),
            },
            Command::PlaylistAdd { path, arrangement } => {
                Arc::make_mut(&mut self.s.playlist)
                    .items
                    .push(PlaylistItem {
                        path: path.clone(),
                        arrangement,
                    });
                self.request_load(&path);
            }
            Command::PlaylistRemove(i) => {
                let items = &mut Arc::make_mut(&mut self.s.playlist).items;
                if i < items.len() {
                    items.remove(i);
                    self.s.playlist_item = match self.s.playlist_item {
                        Some(sel) if sel == i => None,
                        Some(sel) if sel > i => Some(sel - 1),
                        other => other,
                    };
                }
            }
            Command::PlaylistMove { from, to } => {
                let items = &mut Arc::make_mut(&mut self.s.playlist).items;
                if from < items.len() && to < items.len() && from != to {
                    let item = items.remove(from);
                    items.insert(to, item);
                    self.s.playlist_item = self.s.playlist_item.map(|sel| {
                        if sel == from {
                            to
                        } else if from < sel && sel <= to {
                            sel - 1
                        } else if to <= sel && sel < from {
                            sel + 1
                        } else {
                            sel
                        }
                    });
                }
            }
            Command::PlaylistSelect(i) => self.select_item(i),
            Command::Edit { presentation, op } => self.edit(presentation, &op),
            Command::Save(path) => match self.s.docs.get(&path) {
                Some(doc) => {
                    let _ = self.io.send(IoJob::SaveDocument(doc.clone()));
                }
                None => self.notice(
                    NoticeLevel::Warning,
                    format!(
                        "'{}' isn't open, so there is nothing to save.",
                        path.display()
                    ),
                ),
            },
            Command::Revert(path) => {
                Arc::make_mut(&mut self.s.unsaved).remove(&path);
                Arc::make_mut(&mut self.s.docs).remove(&path);
                self.request_load(&path);
            }
        }
    }

    fn edit(&mut self, path: PathBuf, op: &pro6::ops::Op) {
        let Some(old) = self.s.docs.get(&path).cloned() else {
            self.notice(
                NoticeLevel::Warning,
                format!("'{}' isn't open, so it can't be edited.", path.display()),
            );
            return;
        };
        let mut doc = (*old).clone();
        if let Err(e) = op.apply(&mut doc) {
            self.notice(NoticeLevel::Warning, e.user_message());
            return;
        }
        let doc = Arc::new(doc);
        Arc::make_mut(&mut self.s.docs).insert(path.clone(), doc.clone());
        Arc::make_mut(&mut self.s.unsaved).insert(path.clone());
        // A removed arrangement can't stay chosen.
        let arrangements = doc.arrangements();
        if let Some(Some(a)) = self.s.arrangements.get(&path)
            && !arrangements.iter().any(|x| x.uuid.eq_ignore_ascii_case(a))
        {
            Arc::make_mut(&mut self.s.arrangements).insert(path.clone(), None);
        }
        // Follow the live slide (by UUID) into the edited document so edits show on
        // screen; if it was deleted, the screen keeps the old version until the next trigger.
        if let Some(live) = self.s.live.as_ref().filter(|l| l.path == path) {
            let uuid = live
                .doc
                .slides()
                .nth(live.slide.flat)
                .map(|s| s.uuid.clone());
            let arrangement = self.s.arrangement_for(&path);
            let order = slide_order(&doc, arrangement.as_deref());
            let found = uuid.and_then(|u| {
                let flat = doc.slides().position(|s| s.uuid == u)?;
                let probe = SlideRef {
                    flat,
                    group: 0,
                    slide: 0,
                    enabled: true,
                };
                find_nearest(&order, probe, live.position.unwrap_or(0))
            });
            if let Some(pos) = found {
                let slide = order[pos];
                self.s.live = Some(Live {
                    path: path.clone(),
                    doc,
                    arrangement,
                    order: Arc::new(order),
                    position: Some(pos),
                    slide,
                });
            } else if let Some(l) = self.s.live.as_mut() {
                l.position = None;
            }
        }
    }

    fn select_item(&mut self, i: usize) {
        let Some(item) = self.s.playlist.items.get(i).cloned() else {
            return;
        };
        self.s.playlist_item = Some(i);
        self.s.selected = Some(item.path.clone());
        if let Some(a) = item.arrangement {
            Arc::make_mut(&mut self.s.arrangements).insert(item.path.clone(), Some(a));
        }
        self.request_load(&item.path);
        // Pre-parse the next item so moving on is instant.
        if let Some(next) = self.s.playlist.items.get(i + 1).map(|n| n.path.clone()) {
            self.request_load(&next);
        }
    }

    fn preload_playlist_head(&mut self) {
        let first: Vec<PathBuf> = self
            .s
            .playlist
            .items
            .iter()
            .take(2)
            .map(|i| i.path.clone())
            .collect();
        for p in first {
            self.request_load(&p);
        }
    }

    fn go_live(&mut self, path: &PathBuf, position: usize, at: Option<Instant>) {
        let Some(doc) = self.s.docs.get(path).cloned() else {
            return;
        };
        let arrangement = self.s.arrangement_for(path);
        let order = Arc::new(slide_order(&doc, arrangement.as_deref()));
        let Some(&slide) = order.get(position) else {
            self.notice(
                NoticeLevel::Warning,
                format!("Slide {} doesn't exist in this presentation.", position + 1),
            );
            return;
        };
        if let Some(at) = at {
            self.s.command_at = Some(at);
        }
        self.s.live = Some(Live {
            path: path.clone(),
            doc,
            arrangement,
            order,
            position: Some(position),
            slide,
        });
        self.s.text_visible = true;
        self.touch(path);
        // If this presentation is a playlist item, follow it and pre-parse the next one.
        if let Some(i) = self.s.playlist.items.iter().position(|it| &it.path == path)
            && self.s.playlist_item != Some(i)
        {
            self.s.playlist_item = Some(i);
            if let Some(next) = self.s.playlist.items.get(i + 1).map(|n| n.path.clone()) {
                self.request_load(&next);
            }
        }
    }

    fn step(&mut self, forward: bool, at: Instant) {
        if let Some(live) = &self.s.live {
            let target = match (forward, live.position) {
                (true, pos) => next_enabled(&live.order, pos),
                (false, Some(pos)) => prev_enabled(&live.order, pos),
                (false, None) => None,
            };
            if let Some(t) = target {
                let path = live.path.clone();
                self.go_live(&path, t, Some(at));
            } else if forward {
                // Past the last slide: continue with the next playlist item.
                self.next_playlist_item(at);
            } else {
                tracing::debug!("at the first slide; nothing before it");
            }
            return;
        }
        // Nothing live: Next starts the selected presentation.
        if forward
            && let Some(path) = self.s.selected.clone()
            && let Some(order) = self.s.order_for(&path)
            && let Some(first) = next_enabled(&order, None)
        {
            self.go_live(&path, first, Some(at));
        }
    }

    /// Go live on the first enabled slide of the playlist item after the live one.
    /// If it isn't loaded yet, it goes live as soon as it is (the core never waits).
    fn next_playlist_item(&mut self, at: Instant) {
        let Some(live) = &self.s.live else { return };
        let items = &self.s.playlist.items;
        // Prefer the selected item if it is the live one (a presentation can appear twice).
        let current = match self.s.playlist_item {
            Some(i) if items.get(i).is_some_and(|it| it.path == live.path) => Some(i),
            _ => items.iter().position(|it| it.path == live.path),
        };
        let Some(next) = current.and_then(|i| items.get(i + 1).cloned()) else {
            tracing::debug!("at the end of the presentation and the playlist; nothing to step to");
            return;
        };
        let index = current.map_or(0, |i| i + 1);
        if let Some(a) = &next.arrangement {
            Arc::make_mut(&mut self.s.arrangements).insert(next.path.clone(), Some(a.clone()));
        }
        self.s.playlist_item = Some(index);
        self.s.selected = Some(next.path.clone());
        if let Some(first) = self
            .s
            .order_for(&next.path)
            .and_then(|o| next_enabled(&o, None))
        {
            self.go_live(&next.path, first, Some(at));
            self.s.playlist_item = Some(index);
        } else if self.s.docs.contains_key(&next.path) {
            self.notice(
                NoticeLevel::Warning,
                format!("'{}' has no enabled slides to show.", next.path.display()),
            );
        } else {
            self.request_load(&next.path);
            self.pending_goto = Some(PendingGoTo {
                path: next.path,
                position: None,
                at: Some(at),
                text_visible: true,
                background_visible: self.s.background_visible,
            });
        }
    }

    fn select_arrangement(&mut self, path: PathBuf, arrangement: Option<String>) {
        Arc::make_mut(&mut self.s.arrangements).insert(path.clone(), arrangement.clone());
        if let Some(i) = self.s.playlist_item
            && self
                .s
                .playlist
                .items
                .get(i)
                .is_some_and(|it| it.path == path)
        {
            Arc::make_mut(&mut self.s.playlist).items[i].arrangement = arrangement.clone();
        }
        if let Some(live) = self.s.live.as_mut().filter(|l| l.path == path) {
            let order: Vec<SlideRef> = slide_order(&live.doc, arrangement.as_deref());
            live.position = find_nearest(&order, live.slide, live.position.unwrap_or(0));
            live.order = Arc::new(order);
            live.arrangement = arrangement;
            // The slide on screen doesn't change.
        }
    }
}
