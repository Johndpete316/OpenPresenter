//! Finding `.pro6` files.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// A `.pro6` file found under a scanned directory.
pub struct Found {
    pub path: PathBuf,
    /// Path relative to the scanned directory, `/`-separated (stable across OSes).
    pub rel: String,
}

/// All `.pro6` files under `dir`, recursively, sorted by path.
pub fn pro6_files(dir: &Path) -> anyhow::Result<Vec<Found>> {
    let mut paths = Vec::new();
    collect(dir, &mut paths).with_context(|| format!("couldn't scan {}", dir.display()))?;
    paths.sort();
    Ok(paths
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            Found { path, rel }
        })
        .collect())
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
