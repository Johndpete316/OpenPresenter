//! What the remote page is told, as JSON. Two messages, sent separately so a slide
//! change doesn't resend the whole playlist:
//! - [`RemoteState`]: what's live and next, layer visibility (every slide change).
//! - [`ShowInfo`]: playlists and the slides of each presentation in them (only when
//!   playlists, loaded documents or arrangements change).
//!
//! Presentations are named by opaque ids (a hash of the path) so file paths never
//! reach the phone; the server maps them back.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use doc::Document;
use pp6core::ShowState;
use serde::{Deserialize, Serialize};

/// Live / next state (JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteState {
    pub live: Option<LiveInfo>,
    pub next: Option<NextInfo>,
    pub text_visible: bool,
    pub background_visible: bool,
    pub output_enabled: bool,
    /// Poster frame of the background media, if any (`?w=` sets the width).
    #[serde(default)]
    pub background: Option<String>,
    /// Followed playlist item as `[playlist, item]`.
    #[serde(default)]
    pub item: Option<[usize; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveInfo {
    /// Presentation id (as in [`ShowInfo::presentations`]).
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub group: String,
    /// 1-based position in play order (0 = not in the current arrangement).
    pub position: usize,
    pub total: usize,
    pub text: String,
    /// Slide image URL (`?w=` sets the width).
    #[serde(default)]
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NextInfo {
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub text: String,
    pub same_presentation: bool,
    /// 1-based position in its presentation's play order.
    #[serde(default)]
    pub position: usize,
    #[serde(default)]
    pub image: String,
}

/// Playlists and their presentations (JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowInfo {
    pub playlists: Vec<PlaylistInfo>,
    pub presentations: BTreeMap<String, PresentationInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistInfo {
    pub name: String,
    pub items: Vec<ItemInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemInfo {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationInfo {
    pub title: String,
    /// `false` until the presentation is loaded (POST `/api/load/{id}`).
    pub loaded: bool,
    pub error: Option<String>,
    /// Slides in play order (position = index + 1).
    pub slides: Vec<SlideInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlideInfo {
    pub group: String,
    /// Group colour as CSS, e.g. `rgb(200,40,40)`.
    pub color: Option<String>,
    pub label: String,
    pub enabled: bool,
    pub text: String,
    pub image: String,
}

pub(crate) fn title(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    )
}

/// Plain text of a slide (text boxes with text, one after another).
pub fn slide_text(doc: &Document, flat: usize) -> String {
    doc.slides()
        .nth(flat)
        .map(doc::Slide::plain_text)
        .unwrap_or_default()
}

fn hash_hex(v: impl Hash) -> String {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub(crate) fn presentation_id(path: &Path) -> String {
    format!("p{}", hash_hex(path))
}

fn css(c: &doc::Rgba) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("rgb({},{},{})", b(c.r), b(c.g), b(c.b))
}

/// Ids handed to the page, and what they stand for. Image keys are unique per loaded
/// document (an edit makes a new document, so a new key), so images can be cached
/// by the browser for good.
pub(crate) struct Registry {
    /// Differs per run, so a browser never reuses an image from an earlier run.
    nonce: String,
    counter: u64,
    by_ptr: HashMap<usize, (Weak<Document>, String)>,
    pub docs: HashMap<String, Weak<Document>>,
    pub presentations: HashMap<String, PathBuf>,
    pub media: HashMap<String, PathBuf>,
}

impl Registry {
    pub fn new() -> Registry {
        let mut n = [0u8; 4];
        let _ = getrandom::fill(&mut n);
        Registry {
            nonce: format!("{:08x}", u32::from_le_bytes(n)),
            counter: 0,
            by_ptr: HashMap::new(),
            docs: HashMap::new(),
            presentations: HashMap::new(),
            media: HashMap::new(),
        }
    }

    fn doc_key(&mut self, doc: &Arc<Document>) -> String {
        let ptr = Arc::as_ptr(doc) as usize;
        if let Some((w, k)) = self.by_ptr.get(&ptr)
            && w.upgrade().is_some_and(|d| Arc::ptr_eq(&d, doc))
        {
            return k.clone();
        }
        self.counter += 1;
        let k = format!("{}{:x}", self.nonce, self.counter);
        self.by_ptr.insert(ptr, (Arc::downgrade(doc), k.clone()));
        self.docs.insert(k.clone(), Arc::downgrade(doc));
        k
    }

    fn slide_url(&mut self, doc: &Arc<Document>, flat: usize) -> String {
        format!("/img/s/{}/{flat}", self.doc_key(doc))
    }

    fn media_url(&mut self, path: &Path) -> String {
        let k = format!("{}{}", self.nonce, hash_hex(path));
        self.media.insert(k.clone(), path.to_path_buf());
        format!("/img/bg/{k}")
    }

    /// Forget documents nobody holds any more.
    pub fn prune(&mut self) {
        self.by_ptr.retain(|_, (w, _)| w.strong_count() > 0);
        self.docs.retain(|_, w| w.strong_count() > 0);
        if self.media.len() > 64 {
            self.media.clear();
        }
    }
}

impl RemoteState {
    /// Without image URLs or ids (tests, and anyone without a registry).
    pub fn from_show(s: &ShowState) -> RemoteState {
        RemoteState::build(s, &mut Registry::new())
    }

    pub(crate) fn build(s: &ShowState, reg: &mut Registry) -> RemoteState {
        let live = s.live.as_ref().map(|l| LiveInfo {
            id: presentation_id(&l.path),
            title: title(&l.path),
            group: l
                .doc
                .groups()
                .get(l.slide.group)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
            position: l.position.map_or(0, |p| p + 1),
            total: l.order.len(),
            text: slide_text(&l.doc, l.slide.flat),
            image: reg.slide_url(&l.doc, l.slide.flat),
        });
        let next = s.next_slide().map(|(path, doc, pos, flat)| NextInfo {
            id: presentation_id(&path),
            same_presentation: s.live.as_ref().is_some_and(|l| l.path == path),
            title: title(&path),
            text: slide_text(&doc, flat),
            position: pos + 1,
            image: reg.slide_url(&doc, flat),
        });
        let background = s.background_media.as_ref().map(|p| reg.media_url(p));
        RemoteState {
            live,
            next,
            text_visible: s.text_visible,
            background_visible: s.background_visible,
            output_enabled: s.output_enabled,
            background,
            item: s.playlist_item.map(|r| [r.playlist, r.item]),
        }
    }
}

/// The parts of a show state [`ShowInfo`] is made from; it's only rebuilt when one
/// of them changes (holding the `Arc`s keeps the pointer comparison sound).
pub(crate) struct ShowInputs {
    playlists: Arc<Vec<pp6core::PlaylistFile>>,
    docs: Arc<HashMap<PathBuf, Arc<Document>>>,
    errors: Arc<HashMap<PathBuf, String>>,
    arrangements: Arc<HashMap<PathBuf, Option<String>>>,
    live: Option<PathBuf>,
}

impl ShowInputs {
    pub fn of(s: &ShowState) -> ShowInputs {
        ShowInputs {
            playlists: s.playlists.clone(),
            docs: s.docs.clone(),
            errors: s.load_errors.clone(),
            arrangements: s.arrangements.clone(),
            live: s.live.as_ref().map(|l| l.path.clone()),
        }
    }

    pub fn same(&self, o: &ShowInputs) -> bool {
        Arc::ptr_eq(&self.playlists, &o.playlists)
            && Arc::ptr_eq(&self.docs, &o.docs)
            && Arc::ptr_eq(&self.errors, &o.errors)
            && Arc::ptr_eq(&self.arrangements, &o.arrangements)
            && self.live == o.live
    }
}

impl ShowInfo {
    pub(crate) fn build(s: &ShowState, reg: &mut Registry) -> ShowInfo {
        let mut presentations = BTreeMap::new();
        let mut ids = HashMap::new();
        let playlists = s
            .playlists
            .iter()
            .map(|p| PlaylistInfo {
                name: p.playlist.name.clone(),
                items: p
                    .playlist
                    .items
                    .iter()
                    .map(|it| {
                        let id = presentation_id(&it.path);
                        ids.insert(id.clone(), it.path.clone());
                        ItemInfo {
                            id,
                            title: title(&it.path),
                        }
                    })
                    .collect(),
            })
            .collect();
        if let Some(l) = &s.live {
            ids.insert(presentation_id(&l.path), l.path.clone());
        }
        for (id, path) in &ids {
            presentations.insert(id.clone(), presentation(s, reg, path));
        }
        reg.presentations = ids;
        ShowInfo {
            playlists,
            presentations,
        }
    }
}

fn presentation(s: &ShowState, reg: &mut Registry, path: &Path) -> PresentationInfo {
    let Some(doc) = s.docs.get(path) else {
        return PresentationInfo {
            title: title(path),
            loaded: false,
            error: s.load_errors.get(path).cloned(),
            slides: Vec::new(),
        };
    };
    let groups = doc.groups();
    let slides = s
        .order_for(path)
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let g = groups.get(r.group);
            SlideInfo {
                group: g.map(|g| g.name.clone()).unwrap_or_default(),
                color: g.and_then(|g| g.color.as_ref()).map(css),
                label: g
                    .and_then(|g| g.slides().get(r.slide))
                    .map(|sl| sl.label.clone())
                    .unwrap_or_default(),
                enabled: r.enabled,
                text: slide_text(doc, r.flat),
                image: reg.slide_url(doc, r.flat),
            }
        })
        .collect();
    PresentationInfo {
        title: title(path),
        loaded: true,
        error: None,
        slides,
    }
}
