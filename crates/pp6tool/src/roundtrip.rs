//! `pp6tool roundtrip <dir>`: load → save → compare, for every `.pro6` file.
//!
//! Each file is parsed, written back through the model writer, and compared with
//! the original: byte-identical, semantically equal (formatting-only changes), or
//! different (listed). Files that don't parse in the first place are skipped.

use std::fmt::Write as _;
use std::path::Path;

use pro6::Document;
use pro6::diff::{Diff, semantic_diff};

pub struct FileResult {
    pub rel: String,
    pub outcome: Outcome,
}

pub enum Outcome {
    Identical,
    /// Bytes differ but PP6 would read the same thing.
    SemanticallyEqual,
    Different(Vec<Diff>),
    /// The rewritten file no longer parses: a writer bug.
    RewriteUnreadable(String),
    /// The original doesn't parse; nothing to round-trip.
    Skipped(String),
    /// `--out` copy couldn't be written.
    SaveFailed(String),
}

impl Outcome {
    /// Whether this outcome means the writer lost or changed data.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Outcome::Different(_) | Outcome::RewriteUnreadable(_) | Outcome::SaveFailed(_)
        )
    }
}

/// Round-trip every file under `dir`. With `out`, also save each rewritten file
/// under `out` at the same relative path (for opening in PP6).
pub fn run(dir: &Path, out: Option<&Path>) -> anyhow::Result<Vec<FileResult>> {
    let mut results = Vec::new();
    for f in crate::walk::pro6_files(dir)? {
        let _span = tracing::info_span!("roundtrip", file = %f.rel).entered();
        let outcome = round_trip(&f.path, out.map(|o| o.join(&f.rel)).as_deref());
        results.push(FileResult {
            rel: f.rel,
            outcome,
        });
    }
    Ok(results)
}

fn round_trip(path: &Path, save_to: Option<&Path>) -> Outcome {
    let parsed = pro6::read_utf8(path).and_then(|src| {
        let raw = pro6::parse_raw(&src, path)?;
        let doc = Document::from_raw(raw.clone(), path)?;
        Ok((src, raw, doc))
    });
    let (src, original, doc) = match parsed {
        Ok(v) => v,
        Err(e) => return Outcome::Skipped(e.user_message()),
    };

    let written = pro6::write_string(&doc);
    if let Some(target) = save_to {
        let saved = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|e| format!("couldn't create {}: {e}", target.display()))
            .and_then(|()| pro6::save(&doc, target).map_err(|e| e.user_message()));
        if let Err(msg) = saved {
            return Outcome::SaveFailed(msg);
        }
    }
    if written == src {
        return Outcome::Identical;
    }
    let rewritten = match pro6::parse_raw(&written, path) {
        Ok(r) => r,
        Err(e) => return Outcome::RewriteUnreadable(e.to_string()),
    };
    let diffs = semantic_diff(&original.root, &rewritten.root);
    if let Err(e) = Document::from_raw(rewritten, path) {
        return Outcome::RewriteUnreadable(e.to_string());
    }
    if diffs.is_empty() {
        Outcome::SemanticallyEqual
    } else {
        Outcome::Different(diffs)
    }
}

pub fn render_text(results: &[FileResult], title: &str) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "pp6tool roundtrip: {title}");
    let mut counts = [0usize; 6];
    for r in results {
        let (i, label) = match &r.outcome {
            Outcome::Identical => (0, "IDENTICAL"),
            Outcome::SemanticallyEqual => (1, "EQUAL    "),
            Outcome::Different(_) => (2, "DIFF     "),
            Outcome::RewriteUnreadable(_) => (3, "BROKEN   "),
            Outcome::SaveFailed(_) => (4, "SAVEFAIL "),
            Outcome::Skipped(_) => (5, "SKIP     "),
        };
        counts[i] += 1;
        let _ = writeln!(o, "  {label}  {}", r.rel);
        match &r.outcome {
            Outcome::Different(diffs) => {
                const SHOWN: usize = 20;
                for d in diffs.iter().take(SHOWN) {
                    let _ = writeln!(o, "      {d}");
                }
                if diffs.len() > SHOWN {
                    let _ = writeln!(o, "      … {} more", diffs.len() - SHOWN);
                }
            }
            Outcome::RewriteUnreadable(m) => {
                let _ = writeln!(o, "      rewritten file doesn't parse: {m}");
            }
            Outcome::SaveFailed(m) | Outcome::Skipped(m) => {
                let _ = writeln!(o, "      {m}");
            }
            Outcome::Identical | Outcome::SemanticallyEqual => {}
        }
    }
    let _ = writeln!(
        o,
        "\n{} identical, {} semantically equal, {} different, {} broken, {} save failed, {} skipped (input doesn't parse)",
        counts[0], counts[1], counts[2], counts[3], counts[4], counts[5]
    );
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    #[test]
    fn fixtures_round_trip_with_zero_diff() {
        let results = run(&fixtures(), None).unwrap();
        assert!(results.iter().all(|r| !r.outcome.is_failure()));
        let identical: Vec<_> = results
            .iter()
            .filter(|r| matches!(r.outcome, Outcome::Identical))
            .map(|r| r.rel.as_str())
            .collect();
        // unbalanced_rtf.pro6 is valid XML/model (RTF is parsed lazily), so it round-trips too.
        assert_eq!(
            identical,
            [
                "invalid/unbalanced_rtf.pro6",
                "render_1024x768.pro6",
                "song_1024x768.pro6",
                "two_fonts_1920x1080.pro6",
                "unknowns.pro6"
            ]
        );
        insta::assert_snapshot!(render_text(&results, "fixtures"));
    }

    #[test]
    fn out_dir_receives_identical_copies() {
        let out = std::env::temp_dir().join(format!("pp6tool-rt-{}", std::process::id()));
        let results = run(&fixtures(), Some(&out)).unwrap();
        assert!(results.iter().all(|r| !r.outcome.is_failure()));
        let a = std::fs::read(fixtures().join("song_1024x768.pro6")).unwrap();
        let b = std::fs::read(out.join("song_1024x768.pro6")).unwrap();
        assert_eq!(a, b);
        assert!(out.join("invalid/unbalanced_rtf.pro6").exists());
        assert!(
            !out.join("invalid/bad_rect.pro6").exists(),
            "skipped inputs are not written"
        );
        std::fs::remove_dir_all(&out).unwrap();
    }
}
