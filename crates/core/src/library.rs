//! Library index: one entry per `.pro6` file (path, title, mtime, size, search
//! tokens from title and slide text). Cached on disk so the library shows
//! instantly; revalidated in the background, re-parsing only changed files.
//! The full document is parsed for display only when a presentation is opened.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: PathBuf,
    /// File name without extension.
    pub title: String,
    pub modified_ms: u64,
    pub size: u64,
    /// Lowercase words from the title.
    pub title_tokens: Vec<String>,
    /// Lowercase words from the slide text.
    pub text_tokens: Vec<String>,
    /// Operator message if the file couldn't be parsed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryIndex {
    pub version: u32,
    pub entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RevalidateStats {
    pub reused: usize,
    pub parsed: usize,
    pub failed: usize,
    pub removed: usize,
}

pub fn tokenize(text: &str) -> Vec<String> {
    let set: BTreeSet<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    set.into_iter().collect()
}

impl LibraryIndex {
    /// Cached index, or `None` if missing/unreadable/from another version.
    pub fn load_cache(path: &Path) -> Option<LibraryIndex> {
        let bytes = std::fs::read(path).ok()?;
        let idx: LibraryIndex = serde_json::from_slice(&bytes)
            .map_err(|e| tracing::warn!(path = %path.display(), error = %e, "library cache unreadable; rebuilding"))
            .ok()?;
        (idx.version == CACHE_VERSION).then_some(idx)
    }

    pub fn save_cache(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        crate::write_atomic(path, &bytes)
    }

    /// Rescan `dirs`: keep entries whose mtime and size are unchanged, parse the rest.
    pub fn revalidate(&self, dirs: &[PathBuf]) -> (LibraryIndex, RevalidateStats) {
        let started = Instant::now();
        let old: HashMap<&Path, &IndexEntry> =
            self.entries.iter().map(|e| (e.path.as_path(), e)).collect();
        let mut files = Vec::new();
        for d in dirs {
            if let Err(e) = collect(d, &mut files) {
                tracing::warn!(dir = %d.display(), error = %e, "library folder couldn't be scanned");
            }
        }
        files.sort();
        files.dedup();
        let mut stats = RevalidateStats::default();
        let mut entries = Vec::with_capacity(files.len());
        for path in files {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            let size = meta.len();
            if let Some(e) = old
                .get(path.as_path())
                .filter(|e| e.modified_ms == modified_ms && e.size == size)
            {
                stats.reused += 1;
                entries.push((*e).clone());
                continue;
            }
            let entry = index_file(&path, modified_ms, size);
            if entry.error.is_some() {
                stats.failed += 1;
            } else {
                stats.parsed += 1;
            }
            entries.push(entry);
        }
        stats.removed = self
            .entries
            .iter()
            .filter(|e| !entries.iter().any(|n| n.path == e.path))
            .count();
        tracing::info!(
            files = entries.len(),
            reused = stats.reused,
            parsed = stats.parsed,
            failed = stats.failed,
            removed = stats.removed,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "library revalidated"
        );
        (
            LibraryIndex {
                version: CACHE_VERSION,
                entries,
            },
            stats,
        )
    }

    /// Entries matching every word of `query` (as a word prefix), title matches first.
    /// An empty query lists everything by title.
    pub fn search(&self, query: &str) -> Vec<&IndexEntry> {
        let q = tokenize(query);
        let prefix = |tokens: &[String], w: &str| tokens.iter().any(|t| t.starts_with(w));
        let mut hits: Vec<(u8, &IndexEntry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                if q.iter().all(|w| prefix(&e.title_tokens, w)) {
                    Some((0, e))
                } else if q
                    .iter()
                    .all(|w| prefix(&e.title_tokens, w) || prefix(&e.text_tokens, w))
                {
                    Some((1, e))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))
        });
        hits.into_iter().map(|(_, e)| e).collect()
    }
}

fn index_file(path: &Path, modified_ms: u64, size: u64) -> IndexEntry {
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut entry = IndexEntry {
        path: path.to_path_buf(),
        title_tokens: tokenize(&title),
        title,
        modified_ms,
        size,
        text_tokens: Vec::new(),
        error: None,
    };
    match pro6::load(path) {
        Ok(doc) => {
            let mut text = String::new();
            for el in doc.text_elements() {
                match el.rtf_doc(path) {
                    Ok(Some(rtf)) => text.push_str(&rtf.plain_text()),
                    _ => {
                        if let Some(p) = &el.plain_text {
                            text.push_str(&p.value.text());
                        }
                    }
                }
                text.push('\n');
            }
            entry.text_tokens = tokenize(&text);
        }
        Err(e) => entry.error = Some(e.user_message()),
    }
    entry
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pro6"))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// Revalidate on a background thread, save the cache, then call `done`.
pub fn spawn_revalidate(
    index: LibraryIndex,
    dirs: Vec<PathBuf>,
    cache_path: Option<PathBuf>,
    done: impl FnOnce(LibraryIndex, RevalidateStats) + Send + 'static,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("library-index".into())
        .spawn(move || {
            let (new, stats) = index.revalidate(&dirs);
            if let Some(p) = &cache_path
                && (stats.parsed + stats.failed + stats.removed > 0 || !p.exists())
                && let Err(e) = new.save_cache(p)
            {
                tracing::warn!(path = %p.display(), error = %e, "couldn't save library cache");
            }
            done(new, stats);
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    #[test]
    fn revalidate_reuses_unchanged_and_searches_lyrics() {
        let (idx, s) = LibraryIndex::default().revalidate(&[fixtures()]);
        assert_eq!(s.parsed + s.failed, idx.entries.len());
        assert!(
            s.failed >= 5,
            "invalid fixtures are indexed with an error: {s:?}"
        );
        let (again, s2) = idx.revalidate(&[fixtures()]);
        assert_eq!((s2.reused, s2.parsed, s2.failed), (idx.entries.len(), 0, 0));
        assert_eq!(again, idx);

        let titles = |q: &str| {
            idx.search(q)
                .iter()
                .map(|e| e.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("song"), ["song_1024x768"]);
        assert_eq!(titles("wretch"), ["song_1024x768"], "lyric search");
        assert_eq!(titles("amaz gra"), ["song_1024x768"], "word prefixes");
        assert!(titles("zzz").is_empty());
        assert_eq!(idx.search("").len(), idx.entries.len());
    }

    #[test]
    fn cache_round_trip() {
        let (idx, _) = LibraryIndex::default().revalidate(&[fixtures()]);
        let path = std::env::temp_dir().join(format!("pp6core-lib-{}.json", std::process::id()));
        idx.save_cache(&path).unwrap();
        assert_eq!(LibraryIndex::load_cache(&path), Some(idx));
        std::fs::remove_file(&path).unwrap();
    }
}
