//! `pp6tool render <file> --out <dir>`: one PNG per slide, for visual diff against PP6.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context as _;
use render::{Fonts, render_slide};

pub struct Options {
    pub file: PathBuf,
    pub out: PathBuf,
    /// Output size; `None` = the document's canvas size.
    pub size: Option<(u32, u32)>,
    pub font_dirs: Vec<PathBuf>,
    /// Use only `font_dirs`, not system fonts.
    pub fonts_only: bool,
}

pub struct SlideResult {
    pub index: usize,
    pub group: String,
    pub png: Option<PathBuf>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub render_ms: f64,
}

pub fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or("expected WIDTHxHEIGHT, e.g. 1920x1080")?;
    let w: u32 = w.trim().parse().map_err(|_| format!("bad width {w:?}"))?;
    let h: u32 = h.trim().parse().map_err(|_| format!("bad height {h:?}"))?;
    if w == 0 || h == 0 {
        return Err("width and height must be positive".into());
    }
    Ok((w, h))
}

pub fn run(opts: &Options) -> anyhow::Result<Vec<SlideResult>> {
    // Exactly the given file (a .pro6 or .opres), as the app would show it.
    let doc = doc::load(&opts.file)
        .map_err(|e| anyhow::anyhow!("{}\n  detail: {e}", e.user_message()))?;
    let started = Instant::now();
    let mut fonts = if opts.fonts_only {
        let dirs: Vec<&Path> = opts.font_dirs.iter().map(PathBuf::as_path).collect();
        Fonts::from_dirs(&dirs)
    } else {
        let mut f = Fonts::system();
        for d in &opts.font_dirs {
            f.add_dir(d);
        }
        f
    };
    tracing::info!(
        faces = fonts.face_count(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "fonts ready"
    );
    let (w, h) = opts.size.unwrap_or((doc.canvas.width, doc.canvas.height));
    std::fs::create_dir_all(&opts.out)
        .with_context(|| format!("couldn't create {}", opts.out.display()))?;

    let mut results = Vec::new();
    let mut index = 0;
    for group in doc.groups() {
        for slide in group.slides() {
            index += 1;
            let mut r = SlideResult {
                index,
                group: group.name.clone(),
                png: None,
                error: None,
                warnings: Vec::new(),
                render_ms: 0.0,
            };
            match render_slide(&mut fonts, &doc, slide, w, h) {
                Ok(frame) => {
                    r.render_ms = frame.render_time.as_secs_f64() * 1000.0;
                    r.warnings = frame.warnings.iter().map(|w| w.message.clone()).collect();
                    let path = opts
                        .out
                        .join(format!("{index:03}-{}.png", slug(&group.name)));
                    frame
                        .composite()
                        .save(&path)
                        .with_context(|| format!("couldn't write {}", path.display()))?;
                    r.png = Some(path);
                }
                Err(e) => r.error = Some(e.user_message()),
            }
            results.push(r);
        }
    }
    Ok(results)
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "unnamed".into() } else { s }
}

pub fn render_text(results: &[SlideResult]) -> String {
    let mut o = String::new();
    for r in results {
        let group = if r.group.is_empty() {
            "(unnamed)"
        } else {
            &r.group
        };
        match (&r.png, &r.error) {
            (Some(p), _) => {
                let _ = writeln!(
                    o,
                    "  OK    {:>3}  {group}  {}  ({:.1} ms)",
                    r.index,
                    p.display(),
                    r.render_ms
                );
            }
            (None, Some(e)) => {
                let _ = writeln!(o, "  FAIL  {:>3}  {group}  {e}", r.index);
            }
            (None, None) => {}
        }
        for w in &r.warnings {
            let _ = writeln!(o, "        warning: {w}");
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fixture_with_bundled_fonts() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let out = std::env::temp_dir().join(format!("pp6tool-render-{}", std::process::id()));
        let results = run(&Options {
            file: fixtures.join("render_1024x768.pro6"),
            out: out.clone(),
            size: Some((1920, 1080)),
            font_dirs: vec![fixtures.join("fonts")],
            fonts_only: true,
        })
        .unwrap();
        assert_eq!(results.len(), 4);
        assert!(results[..3].iter().all(|r| r.png.is_some()));
        assert!(results[3].error.as_deref().unwrap().contains("Futura PT"));
        let img = image::open(out.join("001-render.png")).unwrap();
        assert_eq!((img.width(), img.height()), (1920, 1080));
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn size_and_slug() {
        assert_eq!(parse_size("1920x1080"), Ok((1920, 1080)));
        assert!(parse_size("0x5").is_err() && parse_size("big").is_err());
        assert_eq!(slug("Verse 1"), "verse-1");
        assert_eq!(slug(""), "unnamed");
    }
}
