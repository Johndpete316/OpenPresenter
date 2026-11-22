//! The show state. Owned and mutated only by the core actor; everyone else sees
//! cheap `Arc` snapshots.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use doc::Document;

use crate::order::{SlideRef, slide_order};
use crate::playlist::{ItemRef, PlaylistFile};

#[derive(Debug, Clone)]
pub struct Live {
    pub path: PathBuf,
    pub doc: Arc<Document>,
    pub arrangement: Option<String>,
    pub order: Arc<Vec<SlideRef>>,
    /// Position in `order`; `None` when the live slide isn't in the current
    /// arrangement (it stays on screen; Next goes to the arrangement's first slide).
    pub position: Option<usize>,
    pub slide: SlideRef,
}

/// A running auto-advance loop (see `Command::StartLoop`).
#[derive(Debug, Clone)]
pub struct LoopState {
    pub path: PathBuf,
    pub timings: Arc<doc::SlideTimings>,
    /// When the current slide advances; `None` = it has no stored time (waits for the operator).
    pub next_at: Option<Instant>,
    /// How long the current slide stays up.
    pub slide_duration: Option<std::time::Duration>,
    /// After the current slide, go back to the first slide (`loopToBeginning`).
    pub loops_back: bool,
}

/// A presentation open in the editor. The editor works on this copy; the output and
/// the operator's grid keep showing the saved document until Save.
#[derive(Debug, Clone)]
pub struct Editing {
    pub doc: Arc<Document>,
    /// Label of the step Undo would revert / Redo would reapply.
    pub undo: Option<String>,
    pub redo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub level: NoticeLevel,
    pub text: String,
    pub at: Instant,
}

/// A slide's video playing as the background.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaCue {
    /// The slide it belongs to (UUID).
    pub slide: String,
    pub video: doc::SlideVideo,
}

#[derive(Debug, Clone)]
pub struct ShowState {
    /// Increments on every change.
    pub seq: u64,
    /// When the command that produced this state arrived (for latency measurement).
    pub command_at: Option<Instant>,
    pub live: Option<Live>,
    pub text_visible: bool,
    pub background_visible: bool,
    pub output_enabled: bool,
    /// The output holds what it showed when this was set (see `Command::Freeze`).
    pub frozen: bool,
    /// The output is black and silent (see `Command::Blank`); wins over `frozen`.
    pub blanked: bool,
    /// Background media playing under the text (when the background layer is visible).
    pub background_media: Option<PathBuf>,
    /// Media to pre-load for an instant switch.
    pub background_next: Option<PathBuf>,
    /// How `background_media` plays when it's a slide's video (`None` = from the
    /// Video/Image panel: loops at full volume).
    pub background_cue: Option<MediaCue>,
    /// The background video is paused (a video that waits for Play, or paused by the
    /// operator).
    pub media_paused: bool,
    /// Bumped to start the background video again from the beginning.
    pub media_restart: u64,
    /// Presentation shown in the operator's grid (selected in library or playlist).
    pub selected: Option<PathBuf>,
    /// All playlists (each saved in its own file), in display order.
    pub playlists: Arc<Vec<PlaylistFile>>,
    /// The selected / followed playlist item.
    pub playlist_item: Option<ItemRef>,
    pub docs: Arc<HashMap<PathBuf, Arc<Document>>>,
    pub load_errors: Arc<HashMap<PathBuf, String>>,
    /// Arrangement chosen per presentation (`None` inside = natural order).
    pub arrangements: Arc<HashMap<PathBuf, Option<String>>>,
    pub notices: Arc<Vec<Notice>>,
    /// Editor copies, by presentation (see [`Editing`]).
    pub editing: Arc<HashMap<PathBuf, Editing>>,
    /// Presentations whose editor copy differs from what's saved.
    pub unsaved: Arc<std::collections::HashSet<PathBuf>>,
    pub looping: Option<LoopState>,
}

impl Default for ShowState {
    fn default() -> Self {
        ShowState {
            seq: 0,
            command_at: None,
            live: None,
            text_visible: true,
            background_visible: true,
            output_enabled: true,
            frozen: false,
            blanked: false,
            background_media: None,
            background_next: None,
            background_cue: None,
            media_paused: false,
            media_restart: 0,
            selected: None,
            playlists: Arc::default(),
            playlist_item: None,
            docs: Arc::default(),
            load_errors: Arc::default(),
            arrangements: Arc::default(),
            notices: Arc::default(),
            editing: Arc::default(),
            unsaved: Arc::default(),
            looping: None,
        }
    }
}

impl ShowState {
    /// The arrangement in effect for a presentation: the operator's choice, else the
    /// document's `selectedArrangementID` if that arrangement exists, else natural order.
    pub fn arrangement_for(&self, path: &Path) -> Option<String> {
        if let Some(choice) = self.arrangements.get(path) {
            return choice.clone();
        }
        self.docs
            .get(path)
            .and_then(|d| d.selected_arrangement())
            .map(|a| a.uuid)
    }

    /// The document the editor shows: the editor copy if there is one, else the saved one.
    pub fn edited_doc(&self, path: &Path) -> Option<&Arc<Document>> {
        self.editing
            .get(path)
            .map(|e| &e.doc)
            .or_else(|| self.docs.get(path))
    }

    /// Play order for a loaded presentation under its current arrangement.
    pub fn order_for(&self, path: &Path) -> Option<Vec<SlideRef>> {
        let doc = self.docs.get(path)?;
        Some(slide_order(doc, self.arrangement_for(path).as_deref()))
    }

    /// What Next would show: the next enabled slide of the live presentation, or the
    /// first enabled slide of the next playlist item when the live one is at its end
    /// (only if that presentation is loaded). Returns `(path, doc, position, flat)`.
    pub fn next_slide(&self) -> Option<(PathBuf, Arc<Document>, usize, usize)> {
        let l = self.live.as_ref()?;
        if let Some(n) = crate::order::next_enabled(&l.order, l.position) {
            return Some((l.path.clone(), l.doc.clone(), n, l.order[n].flat));
        }
        let at = self.playlist_ref_of(&l.path)?;
        let next = self.playlists[at.playlist]
            .playlist
            .items
            .get(at.item + 1)?;
        let doc = self.docs.get(&next.path)?.clone();
        let arrangement = next
            .arrangement
            .clone()
            .or_else(|| self.arrangement_for(&next.path));
        let order = slide_order(&doc, arrangement.as_deref());
        let n = crate::order::next_enabled(&order, None)?;
        Some((next.path.clone(), doc, n, order[n].flat))
    }

    /// The playlist item a presentation is playing as: the selected item if it is that
    /// presentation (it may appear more than once), else its first appearance.
    pub fn playlist_ref_of(&self, path: &Path) -> Option<ItemRef> {
        if let Some(r) = self.playlist_item
            && self.item(r).is_some_and(|it| it.path == path)
        {
            return Some(r);
        }
        self.playlists.iter().enumerate().find_map(|(pi, p)| {
            p.playlist
                .items
                .iter()
                .position(|it| it.path == path)
                .map(|item| ItemRef { playlist: pi, item })
        })
    }

    pub fn item(&self, r: ItemRef) -> Option<&crate::PlaylistItem> {
        self.playlists.get(r.playlist)?.playlist.items.get(r.item)
    }

    /// Whether any playlist contains `path`.
    pub fn in_any_playlist(&self, path: &Path) -> bool {
        self.playlists
            .iter()
            .any(|p| p.playlist.items.iter().any(|i| i.path == path))
    }

    pub fn is_live(&self, path: &Path) -> bool {
        self.live.as_ref().is_some_and(|l| l.path == path)
    }

    /// Short human-readable summary (crash reports, logs).
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "seq {}", self.seq);
        match &self.live {
            Some(l) => {
                let _ = writeln!(
                    s,
                    "live: {} position {:?} (group {}, slide {}), arrangement {:?}",
                    l.path.display(),
                    l.position,
                    l.slide.group,
                    l.slide.slide,
                    l.arrangement
                );
            }
            None => s.push_str("live: none\n"),
        }
        let _ = writeln!(
            s,
            "text visible {}, background visible {}, output enabled {}, background media {:?}",
            self.text_visible, self.background_visible, self.output_enabled, self.background_media
        );
        let _ = writeln!(
            s,
            "{} playlist(s) ({} items), selected item {:?}, selected presentation {:?}",
            self.playlists.len(),
            self.playlists
                .iter()
                .map(|p| p.playlist.items.len())
                .sum::<usize>(),
            self.playlist_item,
            self.selected
        );
        let _ = writeln!(s, "loaded presentations: {}", self.docs.len());
        if let Some(l) = &self.looping {
            let _ = writeln!(
                s,
                "loop running: {} (slide duration {:?})",
                l.path.display(),
                l.slide_duration
            );
        }
        s
    }
}
