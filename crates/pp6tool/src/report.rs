//! `pp6tool report <dir>`: corpus inventory of `.pro6` files.
//!
//! Output is deterministic (sorted, relative paths, no timings) so it can be
//! snapshot-tested and diffed between runs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use pro6::raw::{RawElement, RawNode};
use pro6::{Document, ParseError, schema};
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub files: Vec<FileResult>,
    pub summary: Summary,
}

#[derive(Debug, Serialize)]
pub struct FileResult {
    /// Path relative to the scanned directory, `/`-separated.
    pub path: String,
    pub ok: bool,
    /// Operator-facing message (`ParseError::user_message`).
    pub error: Option<String>,
    /// Technical detail: error kind and XML path.
    pub error_detail: Option<String>,
    pub canvas: Option<String>,
    pub groups: usize,
    pub slides: usize,
    pub disabled_slides: usize,
    pub media: Option<MediaUse>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaUse {
    pub slide_cues: usize,
    pub time_cues: usize,
    pub media_tracks: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub files_total: usize,
    pub files_ok: usize,
    pub files_failed: usize,
    pub canvas_sizes: BTreeMap<String, usize>,
    pub fonts: BTreeMap<String, FontUse>,
    pub group_names: BTreeMap<String, GroupUse>,
    pub elements: BTreeMap<String, ElementUse>,
    pub unhandled_rtf_words: BTreeMap<String, usize>,
    pub files_with_media: Vec<String>,
    pub disabled_slides: usize,
    pub warnings: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct FontUse {
    pub files: usize,
    pub runs: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct GroupUse {
    pub count: usize,
    pub colors: BTreeSet<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ElementUse {
    pub count: usize,
    /// Described by the unofficial spec.
    pub known: bool,
    pub attributes: BTreeMap<String, AttrUse>,
}

#[derive(Debug, Default, Serialize)]
pub struct AttrUse {
    pub count: usize,
    pub known: bool,
}

/// Scan `dir` recursively for `.pro6` files and build the report.
pub fn build(dir: &Path) -> anyhow::Result<Report> {
    let mut report = Report::default();
    for f in crate::walk::pro6_files(dir)? {
        let result = scan_file(&f.path, f.rel, &mut report.summary);
        report.files.push(result);
    }

    let s = &mut report.summary;
    s.files_total = report.files.len();
    s.files_ok = report.files.iter().filter(|f| f.ok).count();
    s.files_failed = s.files_total - s.files_ok;
    s.disabled_slides = report.files.iter().map(|f| f.disabled_slides).sum();
    s.warnings = report.files.iter().map(|f| f.warnings.len()).sum();
    s.files_with_media = report
        .files
        .iter()
        .filter(|f| f.media.is_some())
        .map(|f| f.path.clone())
        .collect();
    Ok(report)
}

fn scan_file(path: &Path, rel: String, summary: &mut Summary) -> FileResult {
    let mut result = FileResult {
        path: rel,
        ok: false,
        error: None,
        error_detail: None,
        canvas: None,
        groups: 0,
        slides: 0,
        disabled_slides: 0,
        media: None,
        warnings: Vec::new(),
    };
    let fail = |result: &mut FileResult, e: ParseError| {
        result.error = Some(e.user_message());
        result.error_detail = Some(format!("{} (at {})", e.kind(), e.xml_path()));
    };

    let src = match pro6::read_utf8(path) {
        Ok(s) => s,
        Err(e) => {
            fail(&mut result, e);
            return result;
        }
    };
    let raw = match pro6::parse_raw(&src, path) {
        Ok(r) => r,
        Err(e) => {
            fail(&mut result, e);
            return result;
        }
    };
    // Inventory from the raw tree, so even files the typed model rejects contribute.
    inventory(&raw.root, &mut summary.elements);

    let doc = match Document::from_raw(raw, path) {
        Ok(d) => d,
        Err(e) => {
            fail(&mut result, e);
            return result;
        }
    };

    // RTF: fonts and unhandled words. Collect into locals so a failing file adds nothing.
    let mut fonts: BTreeMap<String, usize> = BTreeMap::new();
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    for el in doc.text_elements() {
        match el.rtf_doc(path) {
            Ok(Some(rtf)) => {
                for run in rtf.runs() {
                    let name = run.font.clone().unwrap_or_else(|| {
                        format!("(font index {} not in font table)", run.font_index)
                    });
                    *fonts.entry(name).or_default() += 1;
                }
                for (w, n) in rtf.unhandled_words {
                    *words.entry(w).or_default() += n;
                }
            }
            Ok(None) => {}
            Err(e) => {
                fail(&mut result, e);
                return result;
            }
        }
    }

    result.ok = true;
    let canvas = format!("{}x{}", doc.canvas.width, doc.canvas.height);
    *summary.canvas_sizes.entry(canvas.clone()).or_default() += 1;
    result.canvas = Some(canvas);
    for (font, runs) in fonts {
        let f = summary.fonts.entry(font).or_default();
        f.files += 1;
        f.runs += runs;
    }
    for (w, n) in words {
        *summary.unhandled_rtf_words.entry(w).or_default() += n;
    }
    for g in doc.groups() {
        let name = if g.name.is_empty() {
            "(unnamed)".to_owned()
        } else {
            g.name.clone()
        };
        let entry = summary.group_names.entry(name).or_default();
        entry.count += 1;
        if let Some(c) = g.extra.element.attr("color") {
            entry.colors.insert(c.raw_value.clone());
        }
    }
    result.groups = doc.groups().len();
    result.slides = doc.slides().count();
    result.disabled_slides = doc.slides().filter(|s| !s.enabled).count();

    let media = MediaUse {
        slide_cues: doc.slides().map(|s| s.cue_count()).sum(),
        time_cues: doc.timeline.as_ref().map_or(0, |t| t.time_cue_count()),
        media_tracks: doc.timeline.as_ref().map_or(0, |t| t.media_track_count()),
    };
    if media.slide_cues + media.time_cues + media.media_tracks > 0 {
        result.media = Some(media);
    }
    result.warnings = doc
        .warnings
        .iter()
        .map(|w| format!("{}: {}", w.xml_path, w.message))
        .collect();
    result
}

fn inventory(el: &RawElement, out: &mut BTreeMap<String, ElementUse>) {
    let e = out.entry(el.name.clone()).or_default();
    e.count += 1;
    e.known = schema::is_known_element(&el.name);
    let known_attrs = schema::known_attrs(&el.name).unwrap_or(&[]);
    for a in &el.attrs {
        let u = e.attributes.entry(a.name.clone()).or_default();
        u.count += 1;
        u.known = known_attrs.contains(&a.name.as_str());
    }
    for child in &el.children {
        if let RawNode::Element(c) = child {
            inventory(c, out);
        }
    }
}

/// Human-readable report. `title` names the scanned directory.
pub fn render_text(r: &Report, title: &str) -> String {
    let s = &r.summary;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "pp6tool report: {title}\n{} file(s): {} OK, {} failed\n",
        s.files_total, s.files_ok, s.files_failed
    );

    let _ = writeln!(o, "FILES");
    for f in &r.files {
        if f.ok {
            let _ = writeln!(
                o,
                "  OK    {}  {}  {} group(s), {} slide(s), {} disabled{}",
                f.path,
                f.canvas.as_deref().unwrap_or("?"),
                f.groups,
                f.slides,
                f.disabled_slides,
                if f.warnings.is_empty() {
                    String::new()
                } else {
                    format!(", {} warning(s)", f.warnings.len())
                }
            );
        } else {
            let _ = writeln!(o, "  FAIL  {}", f.path);
            if let Some(e) = &f.error {
                let _ = writeln!(o, "        {e}");
            }
            if let Some(d) = &f.error_detail {
                let _ = writeln!(o, "        detail: {d}");
            }
        }
    }

    section(
        &mut o,
        "CANVAS SIZES (files)",
        s.canvas_sizes.iter().map(|(k, v)| format!("{k}  {v}")),
    );
    section(
        &mut o,
        "FONTS (files / runs) — every one must be installed on the show PC",
        s.fonts
            .iter()
            .map(|(k, v)| format!("{k}  {} / {}", v.files, v.runs)),
    );
    section(
        &mut o,
        "GROUP NAMES (count; colours)",
        s.group_names.iter().map(|(k, v)| {
            let colors: Vec<_> = v.colors.iter().map(|c| format!("[{c}]")).collect();
            format!("{k}  {}; {}", v.count, colors.join(" "))
        }),
    );

    let _ = writeln!(
        o,
        "\nELEMENTS AND ATTRIBUTES (count; * = not in spec; includes files that failed after XML parsing)"
    );
    for (name, e) in &s.elements {
        let _ = writeln!(
            o,
            "  <{name}>  {}{}",
            e.count,
            if e.known { "" } else { "  *" }
        );
        for (a, u) in &e.attributes {
            let _ = writeln!(
                o,
                "      @{a}  {}{}",
                u.count,
                if u.known { "" } else { "  *" }
            );
        }
    }

    section(
        &mut o,
        "UNHANDLED RTF CONTROL WORDS (count)",
        s.unhandled_rtf_words
            .iter()
            .map(|(k, v)| format!("\\{k}  {v}")),
    );
    section(
        &mut o,
        "FILES WITH MEDIA / CUES (not supported until M8)",
        r.files.iter().filter_map(|f| {
            let m = f.media.as_ref()?;
            Some(format!(
                "{}  slide cues {}, timeCues {}, mediaTracks {}",
                f.path, m.slide_cues, m.time_cues, m.media_tracks
            ))
        }),
    );
    let _ = writeln!(o, "\nDISABLED SLIDES: {}", s.disabled_slides);
    section(
        &mut o,
        "WARNINGS",
        r.files
            .iter()
            .flat_map(|f| f.warnings.iter().map(move |w| format!("{}: {w}", f.path))),
    );
    o
}

fn section(o: &mut String, title: &str, lines: impl Iterator<Item = String>) {
    let _ = writeln!(o, "\n{title}");
    let mut any = false;
    for l in lines {
        any = true;
        let _ = writeln!(o, "  {l}");
    }
    if !any {
        let _ = writeln!(o, "  (none)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    #[test]
    fn fixtures_report_text() {
        let r = build(&fixtures()).unwrap();
        insta::assert_snapshot!(render_text(&r, "fixtures"));
    }

    #[test]
    fn fixtures_report_counts() {
        let r = build(&fixtures()).unwrap();
        let s = &r.summary;
        assert_eq!((s.files_total, s.files_ok, s.files_failed), (11, 5, 6));
        assert_eq!(s.canvas_sizes.get("1024x768"), Some(&4));
        assert_eq!(s.canvas_sizes.get("1920x1080"), Some(&1));
        assert_eq!(s.fonts.get("Arial").map(|f| f.files), Some(3));
        assert_eq!(s.fonts.get("Georgia").map(|f| f.files), Some(1));
        assert_eq!(s.disabled_slides, 2);
        assert_eq!(
            s.files_with_media,
            ["announcements_synthetic.pro6", "unknowns.pro6"]
        );
        assert!(!s.elements["SyntheticArrangement"].known);
        assert!(!s.elements["RVPresentationDocument"].attributes["futureRootAttr"].known);
        assert!(s.elements["RVPresentationDocument"].attributes["width"].known);
    }

    #[test]
    fn json_serializes() {
        let r = build(&fixtures()).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(v["summary"]["files_total"], 11);
    }
}
