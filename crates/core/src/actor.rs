//! The core actor: a single thread that owns `ShowState`. All mutation goes through
//! `Command`s; each change is broadcast as an `Arc<ShowState>` snapshot. Disk work
//! (presentations, playlists) happens on an I/O thread, so the core never blocks.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use doc::Document;

use crate::command::Command;
use crate::order::{SlideRef, find_nearest, next_enabled, prev_enabled, slide_order};
use crate::playlist::{ItemRef, Playlist, PlaylistFile, PlaylistItem, new_file_name};
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
    /// `(presentation, document sent to save, result)`.
    Saved(PathBuf, Arc<Document>, Result<Document, String>),
    /// Publish a final snapshot and stop.
    Shutdown,
}

enum IoJob {
    Load(PathBuf),
    LoadPlaylist(PathBuf),
    SavePlaylist(Playlist, PathBuf),
    /// Move a deleted playlist's file into `deleted/` (kept, not destroyed).
    DeletePlaylist(PathBuf),
    SaveDocument(PathBuf, Arc<Document>),
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
    /// Where playlists live (one TOML file each); `None` keeps them in memory only.
    pub playlists_dir: Option<PathBuf>,
    pub output_enabled: bool,
}

/// `dir/deleted/<name>` (with a number if that exists).
fn move_to_deleted(path: &std::path::Path) -> std::io::Result<()> {
    let dir = path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("deleted");
    std::fs::create_dir_all(&dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut target = dir.join(&*name);
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{n} {name}"));
        n += 1;
    }
    std::fs::rename(path, target)
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
                let r = doc::open(&path).map(Arc::new).map_err(|e| e.user_message());
                Msg::Loaded(path, r)
            }
            IoJob::LoadPlaylist(path) => {
                let r = Playlist::load(&path).map_err(|e| e.user_message());
                Msg::PlaylistLoaded(path, r)
            }
            IoJob::SaveDocument(path, d) => {
                let r = doc::save(&d).map_err(|e| e.user_message());
                Msg::Saved(path, d, r)
            }
            IoJob::SavePlaylist(p, path) => {
                let r = p.save(&path).map_err(|e| e.user_message());
                Msg::PlaylistSaved(path, r)
            }
            IoJob::DeletePlaylist(path) => {
                let r = move_to_deleted(&path).map_err(|e| {
                    format!(
                        "The deleted playlist's file '{}' couldn't be moved aside ({e}); it may reappear next time.",
                        path.display()
                    )
                });
                Msg::PlaylistSaved(path, r)
            }
        };
        // The core may already have stopped (quitting); keep going so queued saves
        // still reach the disk. The loop ends when the queue is empty and closed.
        let _ = back.send(msg);
    }
}

struct PendingGoTo {
    path: PathBuf,
    /// `None` = the first enabled slide (known once the presentation is loaded), or the
    /// last one with `from_end`.
    position: Option<usize>,
    from_end: bool,
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
    /// Presentation the "not in the playlist" hint was last shown for.
    edge_hint_for: Option<PathBuf>,
    playlists_dir: Option<PathBuf>,
    /// Start Loop was asked for while this presentation was still loading.
    pending_loop: Option<PathBuf>,
    /// Loop stopped by the operator (Stop / Clear) for this live presentation: don't
    /// auto-start again until it is put live again or Start Loop is pressed.
    loop_suppressed: Option<PathBuf>,
    /// Undo history of each editor copy.
    histories: std::collections::HashMap<PathBuf, doc::History>,
    /// Themes to apply once these presentations have loaded.
    pending_theme: std::collections::HashMap<PathBuf, Vec<PendingTheme>>,
}

struct PendingTheme {
    theme: Arc<doc::Theme>,
    targets: crate::ThemeTargets,
    save: bool,
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
            edge_hint_for: None,
            playlists_dir: config.playlists_dir.clone(),
            pending_loop: None,
            loop_suppressed: None,
            histories: std::collections::HashMap::new(),
            pending_theme: std::collections::HashMap::new(),
        };
        if let Some(dir) = &config.playlists_dir {
            let (playlists, errors) = crate::playlist::load_dir(dir);
            tracing::info!(dir = %dir.display(), playlists = playlists.len(), "playlists loaded");
            core.s.playlists = Arc::new(playlists);
            for e in errors {
                core.notice(NoticeLevel::Warning, e.user_message());
            }
            core.preload_playlist_heads();
        }
        core
    }

    fn run(&mut self, rx: &Receiver<Msg>) {
        loop {
            // The loop timer lives here, on the core thread: a UI stall can't delay it.
            let msg = match self.s.looping.as_ref().and_then(|l| l.next_at) {
                Some(deadline) => match rx.recv_deadline(deadline) {
                    Ok(m) => m,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        self.loop_tick();
                        self.publish();
                        continue;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(m) => m,
                    Err(_) => return,
                },
            };
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
                    Ok(mut p) => {
                        if p.name.trim().is_empty() {
                            p.name = path
                                .file_stem()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "Playlist".into());
                        }
                        let i = self.add_playlist(p);
                        self.save_playlist(i);
                        self.preload_playlist_heads();
                    }
                    Err(e) => self.notice(NoticeLevel::Error, e),
                },
                Msg::Saved(path, sent, result) => match result {
                    Ok(saved) => self.saved(path, &sent, saved),
                    Err(e) => self.notice(NoticeLevel::Error, e),
                },
                // Playlists save on every change; only failures are worth a notice.
                Msg::PlaylistSaved(path, result) => match result {
                    Ok(()) => tracing::debug!(path = %path.display(), "playlist saved"),
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
                for w in doc.pro6.iter().flat_map(|p| &p.warnings) {
                    tracing::debug!(file = %path.display(), path = %w.xml_path, msg = %w.message, "parse warning");
                }
                Arc::make_mut(&mut self.s.docs).insert(path.clone(), doc);
                self.touch(&path);
                self.evict();
                if self.pending_loop.as_ref() == Some(&path) {
                    self.pending_loop = None;
                    self.start_loop(Instant::now());
                }
                for p in self.pending_theme.remove(&path).unwrap_or_default() {
                    self.apply_theme(&path, p);
                }
                if self.pending_goto.as_ref().is_some_and(|p| p.path == path)
                    && let Some(p) = self.pending_goto.take()
                {
                    let position = p.position.or_else(|| {
                        self.s.order_for(&p.path).and_then(|o| {
                            if p.from_end {
                                prev_enabled(&o, o.len())
                            } else {
                                next_enabled(&o, None)
                            }
                        })
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
                self.pending_theme.remove(&path);
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
                    || s.editing.contains_key(p)
                    || s.selected.as_ref() == Some(p)
                    || s.in_any_playlist(p)
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
        let playlist_edit = matches!(
            cmd,
            Command::PlaylistCreate(_)
                | Command::PlaylistRename { .. }
                | Command::PlaylistDelete(_)
                | Command::PlaylistAdd { .. }
                | Command::PlaylistRemove(_)
                | Command::PlaylistMove { .. }
        );
        self.command_inner(cmd, at);
        if playlist_edit {
            for p in self.s.playlists.iter() {
                let titles: Vec<String> = p
                    .playlist
                    .items
                    .iter()
                    .map(|i| {
                        i.path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                tracing::info!(playlist = %p.playlist.name, items = ?titles, "playlist");
            }
        }
    }

    fn command_inner(&mut self, cmd: Command, at: Instant) {
        match cmd {
            Command::Next => self.step(true, at),
            Command::Prev => self.step(false, at),
            // Clears are one-shot actions (as in PP6), not toggled states: Clear Text
            // takes the slide off until the next slide is shown; Clear Background stops
            // the background (choosing one again brings it back).
            Command::ClearText => {
                self.s.text_visible = false;
                self.stop_loop_by_operator("Loop stopped (cleared).");
            }
            Command::ClearBackground => {
                self.s.background_media = None;
                self.s.background_next = None;
                self.stop_loop_by_operator("Loop stopped (cleared).");
            }
            Command::ClearAll => {
                self.s.text_visible = false;
                self.s.background_media = None;
                self.s.background_next = None;
                self.stop_loop_by_operator("Loop stopped (cleared).");
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
                        from_end: false,
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
            Command::Preload(path) => self.request_load(&path),
            Command::OutputEnable(on) => self.s.output_enabled = on,
            Command::PlaylistCreate(name) => {
                let i = self.add_playlist(Playlist {
                    name,
                    items: Vec::new(),
                });
                self.save_playlist(i);
            }
            Command::PlaylistRename { playlist, name } => {
                if let Some(p) = Arc::make_mut(&mut self.s.playlists).get_mut(playlist) {
                    p.playlist.name = name;
                    self.save_playlist(playlist);
                }
            }
            Command::PlaylistDelete(playlist) => {
                if playlist < self.s.playlists.len() {
                    let removed = Arc::make_mut(&mut self.s.playlists).remove(playlist);
                    if let Some(f) = removed.file {
                        let _ = self.io.send(IoJob::DeletePlaylist(f));
                    }
                    self.s.playlist_item = match self.s.playlist_item {
                        Some(r) if r.playlist == playlist => None,
                        Some(r) if r.playlist > playlist => Some(ItemRef {
                            playlist: r.playlist - 1,
                            item: r.item,
                        }),
                        other => other,
                    };
                }
            }
            Command::PlaylistImport(path) => {
                let _ = self.io.send(IoJob::LoadPlaylist(path));
            }
            Command::PlaylistAdd {
                playlist,
                path,
                arrangement,
            } => {
                let playlist = if self.s.playlists.is_empty() {
                    self.add_playlist(Playlist {
                        name: "Playlist".into(),
                        items: Vec::new(),
                    })
                } else {
                    playlist
                };
                if let Some(p) = Arc::make_mut(&mut self.s.playlists).get_mut(playlist) {
                    p.playlist.items.push(PlaylistItem {
                        path: path.clone(),
                        arrangement,
                    });
                    self.save_playlist(playlist);
                    self.request_load(&path);
                }
            }
            Command::PlaylistRemove(r) => {
                if let Some(p) = Arc::make_mut(&mut self.s.playlists).get_mut(r.playlist)
                    && r.item < p.playlist.items.len()
                {
                    p.playlist.items.remove(r.item);
                    self.s.playlist_item = match self.s.playlist_item {
                        Some(sel) if sel == r => None,
                        Some(sel) if sel.playlist == r.playlist && sel.item > r.item => {
                            Some(ItemRef {
                                playlist: sel.playlist,
                                item: sel.item - 1,
                            })
                        }
                        other => other,
                    };
                    self.save_playlist(r.playlist);
                }
            }
            Command::PlaylistMove { playlist, from, to } => {
                if let Some(p) = Arc::make_mut(&mut self.s.playlists).get_mut(playlist) {
                    let items = &mut p.playlist.items;
                    if from < items.len() && to < items.len() && from != to {
                        let item = items.remove(from);
                        items.insert(to, item);
                        self.s.playlist_item = self.s.playlist_item.map(|sel| {
                            if sel.playlist != playlist {
                                return sel;
                            }
                            let i = sel.item;
                            let item = if i == from {
                                to
                            } else if from < i && i <= to {
                                i - 1
                            } else if to <= i && i < from {
                                i + 1
                            } else {
                                i
                            };
                            ItemRef { playlist, item }
                        });
                        self.save_playlist(playlist);
                    }
                }
            }
            Command::PlaylistSelect(r) => self.select_item(r),
            Command::Edit {
                presentation,
                edit,
                accept_native,
            } => self.edit(presentation, &edit, accept_native, None),
            Command::EditMerging {
                presentation,
                edit,
                key,
            } => self.edit(presentation, &edit, false, Some(&key)),
            Command::Undo(path) => self.undo_redo(&path, true),
            Command::Redo(path) => self.undo_redo(&path, false),
            Command::Save(path) => match self.s.edited_doc(&path) {
                Some(d) => {
                    let _ = self.io.send(IoJob::SaveDocument(path.clone(), d.clone()));
                }
                None => self.notice(
                    NoticeLevel::Warning,
                    format!(
                        "'{}' isn't open, so there is nothing to save.",
                        path.display()
                    ),
                ),
            },
            Command::ApplyTheme {
                presentations,
                theme,
                targets,
                save,
            } => {
                for path in presentations {
                    let p = PendingTheme {
                        theme: theme.clone(),
                        targets,
                        save,
                    };
                    if self.s.edited_doc(&path).is_some() {
                        self.apply_theme(&path, p);
                    } else {
                        self.pending_theme.entry(path.clone()).or_default().push(p);
                        self.request_load(&path);
                    }
                }
            }
            Command::StartLoop => self.start_loop(at),
            Command::StopLoop => self.stop_loop_by_operator("Loop stopped."),
            Command::Revert(path) => {
                Arc::make_mut(&mut self.s.editing).remove(&path);
                Arc::make_mut(&mut self.s.unsaved).remove(&path);
                self.histories.remove(&path);
            }
        }
    }

    /// Apply a theme to a loaded presentation (see [`Command::ApplyTheme`]).
    fn apply_theme(&mut self, path: &PathBuf, p: PendingTheme) {
        let Some(d) = self.s.edited_doc(path).cloned() else {
            return;
        };
        let slides: Vec<(usize, usize)> = match p.targets {
            crate::ThemeTargets::All => Vec::new(),
            crate::ThemeTargets::Linked => d
                .groups
                .iter()
                .enumerate()
                .flat_map(|(g, gr)| {
                    gr.slides
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| s.theme.as_ref().is_some_and(|l| l.uuid == p.theme.uuid))
                        .map(move |(s, _)| (g, s))
                })
                .collect(),
        };
        if p.targets == crate::ThemeTargets::Linked && slides.is_empty() {
            return;
        }
        let becomes_native = p.theme.features().into_iter().any(doc::needs_native);
        // Saving would also save someone's other unsaved edits: leave it to them.
        let had_unsaved = self.s.unsaved.contains(path);
        let edit = doc::Edit::ApplyTheme {
            slides,
            theme: Box::new(p.theme.for_document(&d.path)),
            all: p.targets == crate::ThemeTargets::All,
            link: d.mode == doc::Mode::Native || becomes_native,
        };
        self.edit(path.clone(), &edit, true, None);
        if p.save && had_unsaved {
            self.notice(
                NoticeLevel::Info,
                format!(
                    "'{}' was restyled to match the theme but not saved, because it has other unsaved changes.",
                    path.display()
                ),
            );
        } else if p.save
            && let Some(d) = self.s.edited_doc(path)
        {
            let _ = self.io.send(IoJob::SaveDocument(path.clone(), d.clone()));
        }
    }

    fn edit(&mut self, path: PathBuf, edit: &doc::Edit, accept_native: bool, merge: Option<&str>) {
        let Some(current) = self.s.edited_doc(&path).cloned() else {
            self.notice(
                NoticeLevel::Warning,
                format!("'{}' isn't open, so it can't be edited.", path.display()),
            );
            return;
        };
        let check = doc::check(&current, edit);
        if let doc::Check::NeedsNative { features, .. } = &check
            && !accept_native
        {
            let names: Vec<&str> = features.iter().map(|f| f.name()).collect();
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "{} can't be saved in a ProPresenter 6 file. Nothing was changed.",
                    names.join(", ")
                ),
            );
            return;
        }
        let mut d = (*current).clone();
        if let Err(e) = edit.apply(&mut d) {
            self.notice(NoticeLevel::Warning, e.user_message());
            return;
        }
        if let doc::Check::NeedsNative { slides, .. } = &check {
            doc::mark_native(&mut d, slides);
        }
        let h = self.histories.entry(path.clone()).or_default();
        match merge {
            Some(key) => h.record_merging(edit.label(), current, key, Instant::now()),
            None => h.record(edit.label(), current),
        }
        self.set_editing(&path, Arc::new(d));
    }

    fn undo_redo(&mut self, path: &PathBuf, undo: bool) {
        let Some(current) = self.s.editing.get(path).map(|e| e.doc.clone()) else {
            return;
        };
        let h = self.histories.entry(path.clone()).or_default();
        let to = if undo {
            h.undo(current)
        } else {
            h.redo(current)
        };
        if let Some(d) = to {
            self.set_editing(path, d);
        }
    }

    /// Make `d` the editor copy of `path`; it's unsaved unless it is the saved document.
    fn set_editing(&mut self, path: &PathBuf, d: Arc<Document>) {
        let h = self.histories.get(path);
        let clean = self.s.docs.get(path).is_some_and(|s| Arc::ptr_eq(s, &d));
        Arc::make_mut(&mut self.s.editing).insert(
            path.clone(),
            crate::state::Editing {
                doc: d,
                undo: h.and_then(|h| h.undo_label()).map(str::to_owned),
                redo: h.and_then(|h| h.redo_label()).map(str::to_owned),
            },
        );
        let unsaved = Arc::make_mut(&mut self.s.unsaved);
        if clean {
            unsaved.remove(path);
        } else {
            unsaved.insert(path.clone());
        }
    }

    /// A save finished: the saved document becomes the one the grid and output use
    /// (the output switches at its next slide change; see `follow_saved`).
    fn saved(&mut self, path: PathBuf, sent: &Arc<Document>, saved: Document) {
        let first_native = saved.mode == doc::Mode::Native && !doc::is_native(&sent.path);
        let file = saved.path.clone();
        let saved = Arc::new(saved);
        Arc::make_mut(&mut self.s.docs).insert(path.clone(), saved.clone());
        if self
            .s
            .editing
            .get(&path)
            .is_some_and(|e| Arc::ptr_eq(&e.doc, sent))
        {
            self.set_editing(&path, saved.clone());
        }
        // A removed arrangement can't stay chosen.
        if let Some(Some(a)) = self.s.arrangements.get(&path)
            && !saved
                .arrangements
                .iter()
                .any(|x| x.uuid.eq_ignore_ascii_case(a))
        {
            Arc::make_mut(&mut self.s.arrangements).insert(path.clone(), None);
        }
        let text = if first_native {
            format!(
                "Saved as {} (the ProPresenter 6 file is unchanged).",
                file.display()
            )
        } else {
            format!("Saved {}", file.display())
        };
        self.notice(NoticeLevel::Info, text);
    }

    /// If the live presentation was saved since it went live, switch to the saved
    /// version, keeping the live slide (by UUID). Called on slide changes, so a save
    /// never changes what's on screen by itself.
    fn follow_saved(&mut self) {
        let Some(live) = &self.s.live else { return };
        let Some(d) = self.s.docs.get(&live.path).cloned() else {
            return;
        };
        if Arc::ptr_eq(&d, &live.doc) {
            return;
        }
        let path = live.path.clone();
        let uuid = live
            .doc
            .slides()
            .nth(live.slide.flat)
            .map(|s| s.uuid.clone());
        let arrangement = self.s.arrangement_for(&path);
        let order = slide_order(&d, arrangement.as_deref());
        let found = uuid.and_then(|u| {
            let flat = d.slides().position(|s| s.uuid == u)?;
            let probe = SlideRef {
                flat,
                group: 0,
                slide: 0,
                enabled: true,
            };
            find_nearest(&order, probe, live.position.unwrap_or(0))
        });
        self.s.live = Some(match found {
            Some(pos) => Live {
                path,
                doc: d,
                arrangement,
                slide: order[pos],
                order: Arc::new(order),
                position: Some(pos),
            },
            // The live slide was deleted: it stays on screen (old document), and the
            // step goes on in the saved order (Next = its first enabled slide).
            None => Live {
                path,
                doc: live.doc.clone(),
                arrangement,
                slide: live.slide,
                order: Arc::new(order),
                position: None,
            },
        });
    }

    fn start_loop(&mut self, at: Instant) {
        let path = match (&self.s.live, &self.s.selected) {
            (Some(l), _) => l.path.clone(),
            (None, Some(p)) => p.clone(),
            (None, None) => {
                self.notice(
                    NoticeLevel::Warning,
                    "Choose the presentation to loop first.".into(),
                );
                return;
            }
        };
        self.loop_suppressed = None;
        let Some(doc) = self.s.docs.get(&path).cloned() else {
            // Still loading: start as soon as it's ready.
            self.request_load(&path);
            self.pending_loop = Some(path);
            return;
        };
        let Some(timings) = doc.slide_timings() else {
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "'{}' has no slide timers, so it can't loop on its own. Slides can still be advanced by hand.",
                    path.display()
                ),
            );
            return;
        };
        if self.s.looping.as_ref().is_some_and(|l| l.path == path) {
            return;
        }
        tracing::info!(path = %path.display(), timers = timings.timers.len(), "loop started");
        self.s.looping = Some(crate::state::LoopState {
            path: path.clone(),
            timings: Arc::new(timings),
            next_at: None,
            slide_duration: None,
            loops_back: false,
        });
        if self.s.is_live(&path) {
            self.restart_loop_timer();
        } else if let Some(first) = self.s.order_for(&path).and_then(|o| next_enabled(&o, None)) {
            self.go_live(&path, first, Some(at));
        }
    }

    fn stop_loop_by_operator(&mut self, why: &str) {
        if let Some(l) = self.s.looping.as_ref() {
            self.loop_suppressed = Some(l.path.clone());
        }
        self.pending_loop = None;
        self.stop_loop(why);
    }

    fn stop_loop(&mut self, why: &str) {
        if self.s.looping.take().is_some() {
            self.notice(NoticeLevel::Info, why.to_owned());
        }
    }

    /// Time the live slide according to the loop's timings.
    fn restart_loop_timer(&mut self) {
        let Some(live) = &self.s.live else { return };
        let uuid = live
            .doc
            .slides()
            .nth(live.slide.flat)
            .map(|s| s.uuid.clone())
            .unwrap_or_default();
        if let Some(l) = self.s.looping.as_mut().filter(|l| l.path == live.path) {
            let timer = l.timings.timers.get(&uuid).copied();
            l.slide_duration = timer.map(|t| t.duration);
            l.loops_back = timer.is_some_and(|t| t.loop_to_beginning);
            l.next_at = l.slide_duration.map(|d| Instant::now() + d);
        }
    }

    /// The live slide's time is up: advance (wrapping if the timeline loops).
    fn loop_tick(&mut self) {
        let Some(l) = self.s.looping.clone() else {
            return;
        };
        let Some(live) = self.s.live.clone().filter(|live| live.path == l.path) else {
            self.stop_loop("Loop stopped.");
            return;
        };
        // Never bring cleared text back on a timer.
        if !self.s.text_visible {
            self.stop_loop("Loop stopped (cleared).");
            return;
        }
        // "Go to Next", or back to the first slide after a loopToBeginning slide.
        let target = if l.loops_back {
            next_enabled(&live.order, None)
        } else {
            next_enabled(&live.order, live.position)
        };
        match target {
            Some(t) => {
                tracing::debug!(position = t, "loop advance");
                self.go_live(&live.path, t, Some(Instant::now()));
            }
            None => self.stop_loop("Loop finished (the last slide doesn't loop back)."),
        }
    }

    fn select_item(&mut self, r: ItemRef) {
        let Some(item) = self.s.item(r).cloned() else {
            return;
        };
        self.s.playlist_item = Some(r);
        self.s.selected = Some(item.path.clone());
        if let Some(a) = item.arrangement {
            Arc::make_mut(&mut self.s.arrangements).insert(item.path.clone(), Some(a));
        }
        self.request_load(&item.path);
        // Pre-parse the next item so moving on is instant.
        if let Some(next) = self.s.item(ItemRef {
            playlist: r.playlist,
            item: r.item + 1,
        }) {
            let next = next.path.clone();
            self.request_load(&next);
        }
    }

    fn add_playlist(&mut self, playlist: Playlist) -> usize {
        let file = self
            .playlists_dir
            .as_ref()
            .map(|d| new_file_name(d, &playlist.name));
        let list = Arc::make_mut(&mut self.s.playlists);
        list.push(PlaylistFile { file, playlist });
        list.len() - 1
    }

    /// Save a playlist to its file (on the I/O thread). Every edit saves.
    fn save_playlist(&mut self, i: usize) {
        if let Some(PlaylistFile {
            file: Some(f),
            playlist,
        }) = self.s.playlists.get(i)
        {
            let _ = self
                .io
                .send(IoJob::SavePlaylist(playlist.clone(), f.clone()));
        }
    }

    /// Parse the first two items of each playlist ahead of time.
    fn preload_playlist_heads(&mut self) {
        let first: Vec<PathBuf> = self
            .s
            .playlists
            .iter()
            .flat_map(|p| p.playlist.items.iter().take(2).map(|i| i.path.clone()))
            .collect();
        for p in first {
            self.request_load(&p);
        }
    }

    fn go_live(&mut self, path: &PathBuf, position: usize, at: Option<Instant>) {
        let Some(doc) = self.s.docs.get(path).cloned() else {
            return;
        };
        if self.s.looping.as_ref().is_some_and(|l| &l.path != path) {
            self.stop_loop("Loop stopped: another presentation went live.");
        }
        // Put live (again) from elsewhere: an earlier Stop/Clear no longer applies.
        if !self.s.is_live(path) {
            self.loop_suppressed = None;
        }
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
        // Timed presentations loop on their own, like PP6 (unless the operator stopped it).
        if self.s.looping.is_none()
            && self.loop_suppressed.as_ref() != Some(path)
            && let Some(timings) = self.s.live.as_ref().and_then(|l| l.doc.slide_timings())
        {
            tracing::info!(path = %path.display(), timers = timings.timers.len(), "loop started (slide timers)");
            self.s.looping = Some(crate::state::LoopState {
                path: path.clone(),
                timings: Arc::new(timings),
                next_at: None,
                slide_duration: None,
                loops_back: false,
            });
        }
        // While looping, every slide change (operator or timer) restarts the timer for it.
        self.restart_loop_timer();
        self.touch(path);
        // If this presentation is a playlist item, follow it and pre-parse the next one.
        if let Some(r) = self.s.playlist_ref_of(path)
            && self.s.playlist_item != Some(r)
        {
            self.s.playlist_item = Some(r);
            if let Some(next) = self.s.item(ItemRef {
                playlist: r.playlist,
                item: r.item + 1,
            }) {
                let next = next.path.clone();
                self.request_load(&next);
            }
        }
    }

    fn step(&mut self, forward: bool, at: Instant) {
        self.follow_saved();
        if let Some(live) = &self.s.live {
            let target = match (forward, live.position) {
                (true, pos) => next_enabled(&live.order, pos),
                (false, Some(pos)) => prev_enabled(&live.order, pos),
                (false, None) => None,
            };
            if let Some(t) = target {
                let path = live.path.clone();
                self.go_live(&path, t, Some(at));
            } else {
                // Past the last slide: the next playlist item's first slide. Before the
                // first slide: the previous item's last slide.
                self.adjacent_playlist_item(forward, at);
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

    /// Go live on the playlist item after the live one (its first enabled slide) or,
    /// with `forward == false`, before it (its last enabled slide). If it isn't loaded
    /// yet, it goes live as soon as it is (the core never waits).
    fn adjacent_playlist_item(&mut self, forward: bool, at: Instant) {
        let Some(live) = &self.s.live else { return };
        // Only within the live presentation's own playlist.
        let current = self.s.playlist_ref_of(&live.path);
        let pl = current.map_or(0, |r| r.playlist);
        let index = match (current.map(|r| r.item), forward) {
            (None, _) => {
                tracing::debug!(
                    "live presentation isn't in the playlist; no next/previous item to step to"
                );
                // Say so once per presentation: silently doing nothing looks like a bug.
                if self.edge_hint_for.as_ref() != Some(&live.path) {
                    let path = live.path.clone();
                    let name = path.file_stem().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into(),
                    );
                    self.edge_hint_for = Some(path);
                    self.notice(
                        NoticeLevel::Warning,
                        format!(
                            "{} '{name}'. It isn't in the playlist, so there's no {} presentation to go to. Drag it into the playlist (or double-click it in the library).",
                            if forward { "End of" } else { "Start of" },
                            if forward { "next" } else { "previous" },
                        ),
                    );
                }
                return;
            }
            (Some(i), true) => i + 1,
            (Some(i), false) => match i.checked_sub(1) {
                Some(p) => p,
                None => {
                    tracing::debug!("at the first slide of the first playlist item");
                    return;
                }
            },
        };
        let Some(next) = self
            .s
            .item(ItemRef {
                playlist: pl,
                item: index,
            })
            .cloned()
        else {
            tracing::debug!("at the end of the playlist");
            return;
        };
        let index = ItemRef {
            playlist: pl,
            item: index,
        };
        if let Some(a) = &next.arrangement {
            Arc::make_mut(&mut self.s.arrangements).insert(next.path.clone(), Some(a.clone()));
        }
        self.s.playlist_item = Some(index);
        self.s.selected = Some(next.path.clone());
        let target = self.s.order_for(&next.path).and_then(|o| {
            if forward {
                next_enabled(&o, None)
            } else {
                prev_enabled(&o, o.len())
            }
        });
        if let Some(first) = target {
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
                from_end: !forward,
                at: Some(at),
                text_visible: true,
                background_visible: self.s.background_visible,
            });
        }
    }

    fn select_arrangement(&mut self, path: PathBuf, arrangement: Option<String>) {
        Arc::make_mut(&mut self.s.arrangements).insert(path.clone(), arrangement.clone());
        if let Some(r) = self.s.playlist_item
            && self.s.item(r).is_some_and(|it| it.path == path)
        {
            Arc::make_mut(&mut self.s.playlists)[r.playlist]
                .playlist
                .items[r.item]
                .arrangement = arrangement.clone();
            self.save_playlist(r.playlist);
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
