//! Native playlists: a TOML file of presentation paths + optional arrangement IDs.
//! (No PP6 playlist import, by design.)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "item")]
    pub items: Vec<PlaylistItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistItem {
    /// Absolute, or relative to the playlist file's folder.
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<String>,
}

/// A playlist and the file it's kept in (`None`: not saved, e.g. in tests).
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistFile {
    pub file: Option<PathBuf>,
    pub playlist: Playlist,
}

/// One item of one playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemRef {
    pub playlist: usize,
    pub item: usize,
}

/// Every `*.toml` playlist in `dir`, sorted by name. Unreadable files are reported, not
/// fatal. A missing folder is just "no playlists yet".
pub fn load_dir(dir: &Path) -> (Vec<PlaylistFile>, Vec<PlaylistError>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (out, errors);
    };
    for e in entries.filter_map(Result::ok) {
        let path = e.path();
        if path
            .extension()
            .is_none_or(|x| !x.eq_ignore_ascii_case("toml"))
        {
            continue;
        }
        match Playlist::load(&path) {
            Ok(mut p) => {
                if p.name.trim().is_empty() {
                    p.name = path
                        .file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }
                out.push(PlaylistFile {
                    file: Some(path),
                    playlist: p,
                });
            }
            Err(e) => errors.push(e),
        }
    }
    out.sort_by_key(|p| p.playlist.name.to_lowercase());
    (out, errors)
}

/// A file name in `dir` for a new playlist called `name` that doesn't exist yet.
pub fn new_file_name(dir: &Path, name: &str) -> PathBuf {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_owned();
    let stem = if stem.is_empty() {
        "Playlist".to_owned()
    } else {
        stem
    };
    let mut path = dir.join(format!("{stem}.toml"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} {n}.toml"));
        n += 1;
    }
    path
}

#[derive(Debug, thiserror::Error)]
pub enum PlaylistError {
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: not a valid playlist: {message}", path = .path.display())]
    Format { path: PathBuf, message: String },
}

impl PlaylistError {
    pub fn user_message(&self) -> String {
        let name = |p: &Path| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        match self {
            PlaylistError::Io { path, source } => {
                format!(
                    "Playlist file '{}' couldn't be read or written ({source}). Other playlists are unaffected.",
                    name(path)
                )
            }
            PlaylistError::Format { path, .. } => {
                format!(
                    "'{}' isn't a playlist this program can read. It was skipped; other playlists are unaffected.",
                    name(path)
                )
            }
        }
    }
}

impl Playlist {
    pub fn load(path: &Path) -> Result<Playlist, PlaylistError> {
        let text = std::fs::read_to_string(path).map_err(|source| PlaylistError::Io {
            path: path.into(),
            source,
        })?;
        let mut p: Playlist = toml::from_str(&text).map_err(|e| PlaylistError::Format {
            path: path.into(),
            message: e.to_string(),
        })?;
        let base = path.parent().unwrap_or(Path::new(""));
        for item in &mut p.items {
            if item.path.is_relative() {
                item.path = base.join(&item.path);
            }
        }
        Ok(p)
    }

    /// Atomic save (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), PlaylistError> {
        if let Some(dir) = path.parent()
            && let Err(source) = std::fs::create_dir_all(dir)
        {
            return Err(PlaylistError::Io {
                path: path.into(),
                source,
            });
        }
        let text = toml::to_string_pretty(self).map_err(|e| PlaylistError::Format {
            path: path.into(),
            message: e.to_string(),
        })?;
        crate::write_atomic(path, text.as_bytes()).map_err(|source| PlaylistError::Io {
            path: path.into(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_relative_paths() {
        let dir = std::env::temp_dir().join(format!("pp6core-pl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("sunday.toml"),
            "name = \"Sunday\"\n[[item]]\npath = \"songs/Amazing Grace.pro6\"\narrangement = \"AA\"\n[[item]]\npath = \"/abs/Notices.pro6\"\n",
        )
        .unwrap();
        let p = Playlist::load(&dir.join("sunday.toml")).unwrap();
        assert_eq!(p.name, "Sunday");
        assert_eq!(p.items[0].path, dir.join("songs/Amazing Grace.pro6"));
        assert_eq!(p.items[0].arrangement.as_deref(), Some("AA"));
        assert_eq!(p.items[1].path, PathBuf::from("/abs/Notices.pro6"));
        p.save(&dir.join("copy.toml")).unwrap();
        assert_eq!(Playlist::load(&dir.join("copy.toml")).unwrap(), p);
        let err = Playlist::load(&dir.join("missing.toml")).unwrap_err();
        assert!(err.user_message().contains("unaffected"));
        std::fs::write(dir.join("bad.toml"), "item = 5").unwrap();
        assert!(matches!(
            Playlist::load(&dir.join("bad.toml")),
            Err(PlaylistError::Format { .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
