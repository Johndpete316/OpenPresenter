//! Media folders: a recursive scan (background thread) and the folder tree the
//! Video/Image panel shows. The tree mirrors the folders on disk; nothing is
//! copied, moved or written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const EXT: &[&str] = &[
    "png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "mp4", "mov", "m4v", "mkv", "avi",
    "wmv", "webm",
];
/// Guards against loops (junctions) and runaway trees.
const MAX_DEPTH: usize = 24;
const MAX_FILES: usize = 50_000;

#[derive(Debug, Default, Clone)]
pub struct MediaScan {
    /// Every image/video under the media folders, sorted.
    pub files: Vec<PathBuf>,
    /// One tree per media folder (only folders that contain media somewhere below).
    pub roots: Vec<MediaNode>,
    /// Sub-folders that couldn't be read (permissions, offline drive).
    pub unreadable: Vec<PathBuf>,
    /// Stopped at `MAX_FILES`.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaNode {
    pub path: PathBuf,
    pub name: String,
    /// Files directly in this folder.
    pub direct: usize,
    /// Files in this folder and everything below it.
    pub total: usize,
    pub children: Vec<MediaNode>,
}

fn is_media(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| EXT.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Folders never worth showing (hidden, Windows system folders).
fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("$RECYCLE.BIN")
        || name.eq_ignore_ascii_case("System Volume Information")
}

pub fn scan(dirs: &[PathBuf]) -> MediaScan {
    let started = std::time::Instant::now();
    let mut out = MediaScan::default();
    for root in dirs {
        if std::fs::read_dir(root).is_err() {
            out.unreadable.push(root.clone());
            continue;
        }
        walk(root, 0, &mut out);
    }
    out.files.sort();
    out.roots = dirs
        .iter()
        .filter(|d| !out.unreadable.contains(d))
        .map(|d| tree(d, &out.files))
        .collect();
    tracing::info!(
        folders = dirs.len(),
        files = out.files.len(),
        unreadable = out.unreadable.len(),
        truncated = out.truncated,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "media folders scanned"
    );
    for p in &out.unreadable {
        tracing::warn!(path = %p.display(), "media folder couldn't be read");
    }
    out
}

fn walk(dir: &Path, depth: usize, out: &mut MediaScan) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            out.unreadable.push(dir.to_path_buf());
            return;
        }
    };
    let mut subdirs = Vec::new();
    for e in entries.filter_map(Result::ok) {
        if out.files.len() >= MAX_FILES {
            out.truncated = true;
            return;
        }
        // `file_type` doesn't follow symlinks, so linked folders can't loop.
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_dir() {
            if !skip_dir(&e.file_name().to_string_lossy()) && depth < MAX_DEPTH {
                subdirs.push(path);
            }
        } else if ft.is_file() && is_media(&path) {
            out.files.push(path);
        }
    }
    for d in subdirs {
        walk(&d, depth + 1, out);
    }
}

/// The folder tree under `root` built from the (sorted) file list.
pub fn tree(root: &Path, files: &[PathBuf]) -> MediaNode {
    #[derive(Default)]
    struct Building {
        direct: usize,
        children: BTreeMap<String, Building>,
    }
    let mut top = Building::default();
    for f in files {
        let Ok(rel) = f.strip_prefix(root) else {
            continue;
        };
        let mut node = &mut top;
        let comps: Vec<_> = rel.components().collect();
        for c in &comps[..comps.len().saturating_sub(1)] {
            node = node
                .children
                .entry(c.as_os_str().to_string_lossy().into_owned())
                .or_default();
        }
        node.direct += 1;
    }
    fn finish(path: PathBuf, name: String, b: Building) -> MediaNode {
        let children: Vec<MediaNode> = b
            .children
            .into_iter()
            .map(|(n, c)| finish(path.join(&n), n, c))
            .collect();
        let total = b.direct + children.iter().map(|c| c.total).sum::<usize>();
        MediaNode {
            path,
            name,
            direct: b.direct,
            total,
            children,
        }
    }
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    finish(root.to_path_buf(), name, top)
}

/// Files under `folder` (or all files), grouped by the folder they're in: the folder's
/// own files first, then each sub-folder in order. Group keys are the folders.
pub fn grouped<'a>(
    files: &'a [PathBuf],
    folder: Option<&Path>,
) -> Vec<(PathBuf, Vec<&'a PathBuf>)> {
    let mut groups: BTreeMap<PathBuf, Vec<&PathBuf>> = BTreeMap::new();
    for f in files {
        if folder.is_some_and(|d| !f.starts_with(d)) {
            continue;
        }
        let parent = f.parent().map(Path::to_path_buf).unwrap_or_default();
        groups.entry(parent).or_default().push(f);
    }
    let mut out: Vec<(PathBuf, Vec<&PathBuf>)> = groups.into_iter().collect();
    // Path order already puts a folder before its sub-folders; keep the selected
    // folder's own files first.
    if let Some(d) = folder
        && let Some(i) = out.iter().position(|(p, _)| p == d)
    {
        let own = out.remove(i);
        out.insert(0, own);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn scan_follows_the_folder_structure() {
        let tmp = std::env::temp_dir().join(format!("pp6-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("Media");
        for f in [
            "loose.png",
            "Christmas/snow.mp4",
            "Christmas/Advent/candle.jpg",
            "Easter/sunrise.MOV",
            "Empty/readme.txt",
            ".hidden/skip.png",
        ] {
            touch(&root.join(f));
        }
        let s = scan(std::slice::from_ref(&root));
        let names: Vec<String> = s
            .files
            .iter()
            .map(|f| {
                f.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            names,
            [
                "Christmas/Advent/candle.jpg",
                "Christmas/snow.mp4",
                "Easter/sunrise.MOV",
                "loose.png"
            ]
        );
        let r = &s.roots[0];
        assert_eq!((r.name.as_str(), r.direct, r.total), ("Media", 1, 4));
        let kids: Vec<(&str, usize, usize)> = r
            .children
            .iter()
            .map(|c| (c.name.as_str(), c.direct, c.total))
            .collect();
        // Folders without media (Empty) and hidden ones don't appear.
        assert_eq!(kids, [("Christmas", 1, 2), ("Easter", 1, 1)]);
        assert_eq!(r.children[0].children[0].name, "Advent");

        let g = grouped(&s.files, Some(&root.join("Christmas")));
        let keys: Vec<_> = g.iter().map(|(p, v)| (p.clone(), v.len())).collect();
        assert_eq!(
            keys,
            [
                (root.join("Christmas"), 1),
                (root.join("Christmas").join("Advent"), 1)
            ]
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_folder_is_reported_not_fatal() {
        let s = scan(&[PathBuf::from("/definitely/not/here/pp6")]);
        assert!(s.files.is_empty() && s.roots.is_empty());
        assert_eq!(s.unreadable.len(), 1);
    }
}
