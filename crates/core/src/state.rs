//! The show state. Owned and mutated only by the core actor; everyone else sees
//! cheap `Arc` snapshots.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use pro6::Document;

use crate::order::{SlideRef, slide_order};
use crate::playlist::Playlist;

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
    /// Background media playing under the text (when the background layer is visible).
    pub background_media: Option<PathBuf>,
    /// Media to pre-load for an instant switch.
    pub background_next: Option<PathBuf>,
    /// Presentation shown in the operator's grid (selected in library or playlist).
    pub selected: Option<PathBuf>,
    pub playlist: Arc<Playlist>,
    pub playlist_path: Option<PathBuf>,
    pub playlist_item: Option<usize>,
    pub docs: Arc<HashMap<PathBuf, Arc<Document>>>,
    pub load_errors: Arc<HashMap<PathBuf, String>>,
    /// Arrangement chosen per presentation (`None` inside = natural order).
    pub arrangements: Arc<HashMap<PathBuf, Option<String>>>,
    pub notices: Arc<Vec<Notice>>,
    /// Presentations with unsaved edits.
    pub unsaved: Arc<std::collections::HashSet<PathBuf>>,
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
            background_media: None,
            background_next: None,
            selected: None,
            playlist: Arc::new(Playlist::default()),
            playlist_path: None,
            playlist_item: None,
            docs: Arc::default(),
            load_errors: Arc::default(),
            arrangements: Arc::default(),
            notices: Arc::default(),
            unsaved: Arc::default(),
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
        let items = &self.playlist.items;
        let i = match self.playlist_item {
            Some(i) if items.get(i).is_some_and(|it| it.path == l.path) => i,
            _ => items.iter().position(|it| it.path == l.path)?,
        };
        let next = items.get(i + 1)?;
        let doc = self.docs.get(&next.path)?.clone();
        let arrangement = next
            .arrangement
            .clone()
            .or_else(|| self.arrangement_for(&next.path));
        let order = slide_order(&doc, arrangement.as_deref());
        let n = crate::order::next_enabled(&order, None)?;
        Some((next.path.clone(), doc, n, order[n].flat))
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
            "playlist {:?} ({} items), selected item {:?}, selected presentation {:?}",
            self.playlist_path,
            self.playlist.items.len(),
            self.playlist_item,
            self.selected
        );
        let _ = writeln!(s, "loaded presentations: {}", self.docs.len());
        s
    }
}
