//! Font database and RTF font-name resolution. Missing fonts are an error, never a
//! silent substitution.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use cosmic_text::{FontSystem, SwashCache};
use fontdb::{Family, Query, Stretch, Style, Weight};

/// System (or test) fonts plus the glyph rasteriser cache.
pub struct Fonts {
    pub(crate) system: FontSystem,
    pub(crate) swash: SwashCache,
    line_heights: HashMap<(fontdb::ID, u16, u32), f32>,
}

/// A concrete face for an RTF font name + bold/italic.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFont {
    /// Family name as the font database spells it.
    pub family: String,
    pub weight: Weight,
    pub style: Style,
    pub id: fontdb::ID,
    /// The family has no face of the requested weight/style; a nearer one is used
    /// (the same family, e.g. regular for bold).
    pub inexact: bool,
}

impl Fonts {
    /// All fonts installed on this machine. Slow on machines with many fonts:
    /// do it off the show-critical path.
    pub fn system() -> Fonts {
        let started = Instant::now();
        let system = FontSystem::new();
        tracing::info!(
            faces = system.db().len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "system fonts loaded"
        );
        Fonts::wrap(system)
    }

    /// Only the fonts in `dirs` (deterministic; used by tests and `--fonts-only`).
    pub fn from_dirs(dirs: &[&Path]) -> Fonts {
        let mut db = fontdb::Database::new();
        for d in dirs {
            db.load_fonts_dir(d);
        }
        tracing::info!(faces = db.len(), "fonts loaded from directories");
        Fonts::wrap(FontSystem::new_with_locale_and_db("en-US".to_owned(), db))
    }

    /// Add the fonts in `dir` (e.g. fonts shipped with a library).
    pub fn add_dir(&mut self, dir: &Path) {
        self.system.db_mut().load_fonts_dir(dir);
    }

    fn wrap(system: FontSystem) -> Fonts {
        Fonts {
            system,
            swash: SwashCache::new(),
            line_heights: HashMap::new(),
        }
    }

    pub fn face_count(&self) -> usize {
        self.system.db().len()
    }

    /// Resolve an RTF font-table name. Tries the family name, then the PostScript
    /// name (Cocoa RTF may use names like `Helvetica-Bold`), case-insensitively.
    pub fn resolve(&self, name: &str, bold: bool, italic: bool) -> Option<ResolvedFont> {
        let db = self.system.db();
        let want_weight = if bold { Weight::BOLD } else { Weight::NORMAL };
        let want_style = if italic { Style::Italic } else { Style::Normal };

        let family = db
            .faces()
            .flat_map(|f| f.families.iter().map(|(n, _)| n))
            .find(|n| n.eq_ignore_ascii_case(name))
            .cloned();
        if let Some(family) = family {
            let id = db.query(&Query {
                families: &[Family::Name(&family)],
                weight: want_weight,
                stretch: Stretch::Normal,
                style: want_style,
            })?;
            let face = db.face(id)?;
            let inexact = face.weight != want_weight || face.style != want_style;
            return Some(ResolvedFont {
                family,
                weight: face.weight,
                style: face.style,
                id,
                inexact,
            });
        }

        let face = db
            .faces()
            .find(|f| f.post_script_name.eq_ignore_ascii_case(name))?;
        let family = face.families.first()?.0.clone();
        // The PostScript name already picks a face; RTF \b / \i may add to it.
        let weight = if bold && face.weight < Weight::BOLD {
            Weight::BOLD
        } else {
            face.weight
        };
        let style = if italic { Style::Italic } else { face.style };
        let id = db.query(&Query {
            families: &[Family::Name(&family)],
            weight,
            stretch: face.stretch,
            style,
        })?;
        let got = db.face(id)?;
        let inexact = got.weight != weight || got.style != style;
        Some(ResolvedFont {
            family,
            weight: got.weight,
            style: got.style,
            id,
            inexact,
        })
    }

    /// Family names of a face (for detecting per-glyph fallback).
    pub(crate) fn face_families(&self, id: fontdb::ID) -> Vec<String> {
        self.system
            .db()
            .face(id)
            .map(|f| f.families.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    /// Natural line height (ascent + descent + line gap) for a face at `px` size.
    pub(crate) fn line_height(&mut self, font: &ResolvedFont, px: f32) -> f32 {
        let key = (font.id, font.weight.0, px.to_bits());
        if let Some(h) = self.line_heights.get(&key) {
            return *h;
        }
        let h = self
            .system
            .get_font(font.id, font.weight)
            .map(|f| {
                let m = f.as_swash().metrics(&[]).scale(px);
                m.ascent + m.descent + m.leading
            })
            .filter(|h| *h > 0.0)
            .unwrap_or(px * 1.2);
        self.line_heights.insert(key, h);
        h
    }
}
