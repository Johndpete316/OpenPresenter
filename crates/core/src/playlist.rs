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
                    "Playlist '{}' couldn't be read or written ({source}). The current playlist is unchanged.",
                    name(path)
                )
            }
            PlaylistError::Format { path, .. } => {
                format!(
                    "'{}' isn't a playlist this program can read. The current playlist is unchanged.",
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
        assert!(err.user_message().contains("unchanged"));
        std::fs::write(dir.join("bad.toml"), "item = 5").unwrap();
        assert!(matches!(
            Playlist::load(&dir.join("bad.toml")),
            Err(PlaylistError::Format { .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
