//! `pp6tool rescale --to WxH <files|dirs>`: change presentations' canvas resolution.
//!
//! Dry run by default (report only). `--out DIR` writes rescaled copies; `--in-place`
//! backs each original up to `<name>.pro6.bak` (never overwriting an existing backup)
//! before replacing it. Every written file is read back and checked.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use pro6::rescale::{Mode, RescaleReport, rescale};
use pro6::{CanvasSize, WinPayloads};
use render::{Fonts, render_slide};

pub struct Options {
    pub to: (u32, u32),
    pub inputs: Vec<PathBuf>,
    pub stretch: bool,
    pub out: Option<PathBuf>,
    pub in_place: bool,
    pub preview: Option<PathBuf>,
    pub keep_win_payloads: bool,
    pub font_dirs: Vec<PathBuf>,
}

pub struct FileResult {
    pub rel: String,
    pub report: Option<RescaleReport>,
    pub written: Option<PathBuf>,
    pub backup: Option<PathBuf>,
    pub error: Option<String>,
}

/// Files to process, each with the path to report and to mirror under `--out`.
fn inputs(paths: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            for f in crate::walk::pro6_files(p)? {
                out.push((f.path, f.rel));
            }
        } else {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push((p.clone(), name));
        }
    }
    Ok(out)
}

pub fn run(o: &Options) -> anyhow::Result<Vec<FileResult>> {
    anyhow::ensure!(
        !(o.out.is_some() && o.in_place),
        "use either --out or --in-place, not both"
    );
    let to = CanvasSize {
        width: o.to.0,
        height: o.to.1,
    };
    let mode = if o.stretch { Mode::Stretch } else { Mode::Fit };
    let win = if o.keep_win_payloads {
        WinPayloads::Keep
    } else {
        WinPayloads::Drop
    };
    let mut fonts = o.preview.as_ref().map(|_| {
        let mut f = Fonts::system();
        for d in &o.font_dirs {
            f.add_dir(d);
        }
        f
    });
    let mut results = Vec::new();
    for (path, rel) in inputs(&o.inputs)? {
        let mut r = FileResult {
            rel: rel.clone(),
            report: None,
            written: None,
            backup: None,
            error: None,
        };
        let original = match pro6::load(&path) {
            Ok(d) => d,
            Err(e) => {
                r.error = Some(e.user_message());
                results.push(r);
                continue;
            }
        };
        let mut doc = original.clone();
        let report = rescale(&mut doc, to, mode, win);
        if let (Some(dir), Some(f)) = (&o.preview, fonts.as_mut())
            && let Err(e) = preview(f, &original, &doc, &dir.join(rel.trim_end_matches(".pro6")))
        {
            r.error = Some(format!("preview failed: {e}"));
        }
        let target = match (&o.out, o.in_place) {
            (Some(dir), _) => Some(dir.join(&rel)),
            (None, true) if !report.unchanged => Some(path.clone()),
            _ => None,
        };
        if let Some(target) = target {
            match write(&doc, &path, &target, o.in_place, to) {
                Ok(backup) => {
                    r.written = Some(target);
                    r.backup = backup;
                }
                Err(e) => r.error = Some(format!("{e:#}")),
            }
        }
        r.report = Some(report);
        results.push(r);
    }
    Ok(results)
}

/// Write `doc` to `target` (backing up `source` first when in place), then verify.
fn write(
    doc: &pro6::Document,
    source: &Path,
    target: &Path,
    in_place: bool,
    to: CanvasSize,
) -> anyhow::Result<Option<PathBuf>> {
    let mut backup = None;
    if in_place {
        let mut n = 0;
        let b = loop {
            let mut name = source
                .file_name()
                .map(|s| s.to_os_string())
                .unwrap_or_default();
            name.push(if n == 0 {
                ".bak".to_owned()
            } else {
                format!(".bak{n}")
            });
            let candidate = source.with_file_name(name);
            if !candidate.exists() {
                break candidate;
            }
            n += 1;
        };
        std::fs::copy(source, &b)
            .with_context(|| format!("couldn't back up {} to {}", source.display(), b.display()))?;
        backup = Some(b);
    } else if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("couldn't create {}", parent.display()))?;
    }
    pro6::save(doc, target).map_err(|e| anyhow::anyhow!(e.user_message()))?;
    // Verify: re-read, check the canvas, and check a second write is identical.
    let back = pro6::load(target)
        .map_err(|e| anyhow::anyhow!("written file doesn't read back: {}", e.user_message()))?;
    anyhow::ensure!(
        back.canvas == to,
        "written file has canvas {:?}, expected {:?}",
        back.canvas,
        to
    );
    anyhow::ensure!(
        pro6::write_string(&back) == std::fs::read_to_string(target)?,
        "written file doesn't round-trip cleanly"
    );
    Ok(backup)
}

fn preview(
    fonts: &mut Fonts,
    before: &pro6::Document,
    after: &pro6::Document,
    dir: &Path,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (label, doc) in [("before", before), ("after", after)] {
        for (i, slide) in doc.slides().enumerate() {
            let path = dir.join(format!("{label}-{:03}.png", i + 1));
            match render_slide(fonts, doc, slide, doc.canvas.width, doc.canvas.height) {
                Ok(frame) => frame.composite().save(&path)?,
                Err(e) => tracing::warn!(slide = i + 1, error = %e, "preview slide not rendered"),
            }
        }
    }
    Ok(())
}

pub fn render_text(results: &[FileResult], dry_run: bool) -> String {
    let mut o = String::new();
    if dry_run {
        let _ = writeln!(
            o,
            "DRY RUN: nothing written (use --out DIR or --in-place)\n"
        );
    }
    for r in results {
        let _ = writeln!(o, "{}", r.rel);
        if let Some(rep) = &r.report {
            let (Some(f), Some(t)) = (rep.from, rep.to) else {
                continue;
            };
            if rep.unchanged {
                let _ = writeln!(o, "  already {}x{}; unchanged", t.width, t.height);
            } else {
                let _ = writeln!(
                    o,
                    "  canvas {}x{} → {}x{}  scale x {:.4} y {:.4} text {:.4}  offset ({:.1}, {:.1})",
                    f.width,
                    f.height,
                    t.width,
                    t.height,
                    rep.scale_x,
                    rep.scale_y,
                    rep.scale_text,
                    rep.offset_x,
                    rep.offset_y
                );
                let sizes: Vec<String> = rep
                    .font_sizes
                    .iter()
                    .map(|(a, b)| format!("{}→{}pt", *a as f64 / 2.0, *b as f64 / 2.0))
                    .collect();
                let _ = writeln!(
                    o,
                    "  {} text element(s); font sizes {}",
                    rep.text_elements,
                    if sizes.is_empty() {
                        "unchanged".into()
                    } else {
                        sizes.join(", ")
                    }
                );
                if !rep.other_rtf_sizes.is_empty() {
                    let other: Vec<String> = rep
                        .other_rtf_sizes
                        .iter()
                        .map(|(w, n)| format!("\\{w}×{n}"))
                        .collect();
                    let _ = writeln!(o, "  other RTF sizes scaled: {}", other.join(" "));
                }
                let _ = writeln!(
                    o,
                    "  shadows {}, strokes {}, Win payloads dropped {}",
                    rep.shadows, rep.strokes, rep.win_payloads_dropped
                );
            }
            for w in &rep.warnings {
                let _ = writeln!(o, "  warning: {w}");
            }
        }
        if let Some(p) = &r.written {
            let _ = writeln!(o, "  written: {}", p.display());
        }
        if let Some(b) = &r.backup {
            let _ = writeln!(o, "  original backed up to: {}", b.display());
        }
        if let Some(e) = &r.error {
            let _ = writeln!(o, "  ERROR: {e}");
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn opts(inputs: Vec<PathBuf>) -> Options {
        Options {
            to: (1024, 768),
            inputs,
            stretch: false,
            out: None,
            in_place: false,
            preview: None,
            keep_win_payloads: false,
            font_dirs: vec![],
        }
    }

    #[test]
    fn dry_run_writes_nothing_and_reports() {
        let results = run(&opts(vec![fixtures().join("two_fonts_1920x1080.pro6")])).unwrap();
        assert!(results[0].written.is_none());
        let text = render_text(&results, true);
        assert!(text.contains("DRY RUN"));
        assert!(text.contains("canvas 1920x1080 → 1024x768"), "{text}");
        assert!(text.contains("offset (0.0, 96.0)"), "{text}");
    }

    #[test]
    fn out_dir_and_in_place_with_backup() {
        let tmp = std::env::temp_dir().join(format!("pp6tool-rescale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // --out mirrors the input folder.
        let results = run(&Options {
            out: Some(tmp.join("out")),
            ..opts(vec![fixtures()])
        })
        .unwrap();
        assert!(tmp.join("out/two_fonts_1920x1080.pro6").exists());
        assert!(
            results
                .iter()
                .filter(|r| r.rel.starts_with("invalid/") && r.rel != "invalid/unbalanced_rtf.pro6")
                .all(|r| r.error.is_some())
        );
        // --in-place backs up first, and never overwrites a backup.
        let f = tmp.join("song.pro6");
        std::fs::copy(fixtures().join("two_fonts_1920x1080.pro6"), &f).unwrap();
        let original = std::fs::read(&f).unwrap();
        let r = run(&Options {
            in_place: true,
            ..opts(vec![f.clone()])
        })
        .unwrap();
        assert_eq!(
            r[0].backup.as_deref(),
            Some(tmp.join("song.pro6.bak").as_path())
        );
        assert_eq!(std::fs::read(tmp.join("song.pro6.bak")).unwrap(), original);
        assert_eq!(
            pro6::load(&f).unwrap().canvas,
            CanvasSize {
                width: 1024,
                height: 768
            }
        );
        // Already at size: nothing written, no new backup.
        let r = run(&Options {
            in_place: true,
            ..opts(vec![f.clone()])
        })
        .unwrap();
        assert!(r[0].report.as_ref().unwrap().unchanged && r[0].backup.is_none());
        std::fs::copy(fixtures().join("two_fonts_1920x1080.pro6"), &f).unwrap();
        let r = run(&Options {
            in_place: true,
            ..opts(vec![f.clone()])
        })
        .unwrap();
        assert_eq!(
            r[0].backup.as_deref(),
            Some(tmp.join("song.pro6.bak1").as_path())
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
