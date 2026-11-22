//! User settings (TOML). Missing fields take defaults, so older files keep working.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Command;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folders scanned for presentations (`.pro6` and `.opres`).
    pub library_dirs: Vec<PathBuf>,
    /// Where new presentations (`.opres`) are saved; `None` = the first library folder.
    /// Also scanned as part of the library. Keeps OpenPresenter's own files apart from
    /// the `.pro6` library while both are in use.
    pub new_presentations_dir: Option<PathBuf>,
    /// Folders listed in the media bin.
    pub media_dirs: Vec<PathBuf>,
    /// Extra font folders (in addition to installed fonts).
    pub font_dirs: Vec<PathBuf>,
    pub output: OutputSettings,
    pub keys: KeyBindings,
    /// mpv audio device name (`None` = system default). Used from M5.
    pub audio_device: Option<String>,
    pub web: WebSettings,
    pub editor: EditorSettings,
    pub view: ViewSettings,
    pub integrations: IntegrationSettings,
}

/// Optional connections to outside services (Settings → Integrations).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationSettings {
    pub esv: EsvSettings,
}

/// ESV Bible API (api.esv.org): scripture presentations from a reference. The API
/// token isn't kept here but in the system's credential store.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EsvSettings {
    pub enabled: bool,
    /// Theme (UUID) for scripture slides; `None` = the plain look.
    pub theme: Option<String>,
}

/// How the operator window looks (remembered between runs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    /// Slide tile width in the presentation grid, in points (0 = the default size).
    pub tile_width: f32,
}

/// Slide editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// Fit Text never shrinks the main text size below this (canvas pixels).
    pub fit_min_size: f64,
    /// Colours for groups made by Paste Lyrics, by name ("Verse" also matches
    /// "Verse 2").
    pub group_colors: Vec<GroupColor>,
    /// Font of the plain look for new presentations (scripture too).
    pub default_font: String,
    /// Text size of the plain look, canvas pixels; 0 = sized for the canvas.
    pub default_font_size: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupColor {
    pub name: String,
    pub color: [u8; 3],
}

impl Default for EditorSettings {
    fn default() -> Self {
        let c = |name: &str, color: [u8; 3]| GroupColor {
            name: name.into(),
            color,
        };
        EditorSettings {
            fit_min_size: 24.0,
            group_colors: vec![
                c("Verse", [40, 110, 230]),
                c("Pre-Chorus", [240, 140, 30]),
                c("Chorus", [220, 40, 50]),
                c("Bridge", [150, 60, 200]),
                c("Tag", [20, 160, 140]),
                c("Intro", [120, 120, 120]),
                c("Outro", [120, 120, 120]),
                c("Ending", [120, 120, 120]),
            ],
            default_font: "Arial".into(),
            default_font_size: 0.0,
        }
    }
}

/// Phone remote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSettings {
    pub enabled: bool,
    pub port: u16,
    /// Listen address; `None` = this PC's LAN address.
    pub bind: Option<String>,
    /// PIN digits, 4–6.
    pub pin_length: u8,
}

impl Default for WebSettings {
    fn default() -> Self {
        WebSettings {
            enabled: true,
            port: 8686,
            bind: None,
            pin_length: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    pub enabled: bool,
    /// Display name as reported by the OS (preferred, survives re-plugging).
    pub monitor_name: Option<String>,
    /// Fallback display index when the name isn't found.
    pub monitor_index: Option<usize>,
    /// Switch the output display to this mode while the output is open
    /// (`None` = leave the display as Windows has it).
    pub resolution: Option<Resolution>,
    /// How background images and videos fill the output.
    pub media_scaling: MediaScaling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaScaling {
    /// Cover the whole output, cropping edges that don't fit (no black bars).
    #[default]
    Fill,
    /// Show the whole picture, black bars where the shapes differ.
    Fit,
    /// Stretch to the output's shape.
    Stretch,
}

impl Default for OutputSettings {
    fn default() -> Self {
        OutputSettings {
            enabled: true,
            monitor_name: None,
            monitor_index: None,
            resolution: None,
            media_scaling: MediaScaling::Fill,
        }
    }
}

/// Key names follow winit's logical key names (`PageDown`, `ArrowRight`, `F1`,
/// `Space`) or a single character (`.`, `b`). Matching is case-insensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    pub next: Vec<String>,
    pub prev: Vec<String>,
    pub clear_text: Vec<String>,
    pub clear_background: Vec<String>,
    pub clear_all: Vec<String>,
    /// Toggles Freeze / Blank.
    pub freeze: Vec<String>,
    pub blank: Vec<String>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let v = |keys: &[&str]| keys.iter().map(|k| (*k).to_owned()).collect();
        KeyBindings {
            // Clickers send PageDown/PageUp (some send arrows); their "blank" button sends `.` or `b`.
            next: v(&["PageDown", "ArrowRight", "ArrowDown", "Space"]),
            prev: v(&["PageUp", "ArrowLeft", "ArrowUp"]),
            clear_text: v(&["F3"]),
            clear_background: v(&["F2"]),
            clear_all: v(&["F1", ".", "b", "Clear"]),
            freeze: v(&["F4"]),
            blank: v(&["F5"]),
        }
    }
}

impl KeyBindings {
    /// The command bound to a key name, if any.
    pub fn command_for(&self, key: &str) -> Option<Command> {
        let hit = |list: &[String]| list.iter().any(|k| k.eq_ignore_ascii_case(key));
        if hit(&self.next) {
            Some(Command::Next)
        } else if hit(&self.prev) {
            Some(Command::Prev)
        } else if hit(&self.clear_all) {
            Some(Command::ClearAll)
        } else if hit(&self.clear_text) {
            Some(Command::ClearText)
        } else if hit(&self.clear_background) {
            Some(Command::ClearBackground)
        } else if hit(&self.freeze) {
            Some(Command::FreezeToggle)
        } else if hit(&self.blank) {
            Some(Command::BlankToggle)
        } else {
            None
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {message}", path = .path.display())]
    Format { path: PathBuf, message: String },
}

impl SettingsError {
    pub fn user_message(&self) -> String {
        match self {
            SettingsError::Io { source, .. } => {
                format!("Settings couldn't be read or saved ({source}). Defaults are used for now.")
            }
            SettingsError::Format { message, .. } => {
                format!(
                    "The settings file has a mistake ({message}). Defaults are used until it's fixed."
                )
            }
        }
    }
}

impl Settings {
    /// Every folder the library reads: the library folders, then the new-presentations
    /// folder if it isn't one of them (or inside one).
    pub fn library_folders(&self) -> Vec<PathBuf> {
        let mut v = self.library_dirs.clone();
        if let Some(d) = &self.new_presentations_dir
            && !v.iter().any(|l| d.starts_with(l))
        {
            v.push(d.clone());
        }
        v
    }

    /// The folder new presentations are saved in.
    pub fn new_presentations_folder(&self) -> Option<PathBuf> {
        self.new_presentations_dir
            .clone()
            .or_else(|| self.library_dirs.first().cloned())
    }

    /// Load settings; a missing file gives defaults.
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| SettingsError::Format {
                path: path.into(),
                message: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(source) => Err(SettingsError::Io {
                path: path.into(),
                source,
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let text = toml::to_string_pretty(self).map_err(|e| SettingsError::Format {
            path: path.into(),
            message: e.to_string(),
        })?;
        crate::write_atomic(path, text.as_bytes()).map_err(|source| SettingsError::Io {
            path: path.into(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let k = KeyBindings::default();
        assert_eq!(k.command_for("pagedown"), Some(Command::Next));
        assert_eq!(k.command_for("PageUp"), Some(Command::Prev));
        assert_eq!(k.command_for("F1"), Some(Command::ClearAll));
        assert_eq!(k.command_for("B"), Some(Command::ClearAll));
        assert_eq!(k.command_for("F3"), Some(Command::ClearText));
        assert_eq!(k.command_for("x"), None);
        let s: Settings =
            toml::from_str("library_dirs = [\"/lib\"]\n[keys]\nnext = [\"Enter\"]\n").unwrap();
        assert_eq!(s.library_dirs, [PathBuf::from("/lib")]);
        assert_eq!(s.keys.command_for("Enter"), Some(Command::Next));
        assert_eq!(
            s.keys.command_for("PageDown"),
            None,
            "a configured list replaces the default"
        );
        assert_eq!(s.keys.prev, KeyBindings::default().prev);
        assert!(s.output.enabled);
        assert_eq!(s.output.media_scaling, MediaScaling::Fill);
        let o: Settings = toml::from_str(
            "[output]\nmedia_scaling = \"fit\"\nresolution = { width = 1920, height = 1200 }\n",
        )
        .unwrap();
        assert_eq!(o.output.media_scaling, MediaScaling::Fit);
        assert_eq!(
            o.output.resolution,
            Some(Resolution {
                width: 1920,
                height: 1200
            })
        );
        assert!(toml::to_string_pretty(&o).is_ok());
        let mut f = Settings {
            library_dirs: vec![PathBuf::from("/lib")],
            ..Settings::default()
        };
        assert_eq!(f.new_presentations_folder(), Some(PathBuf::from("/lib")));
        assert_eq!(f.library_folders(), [PathBuf::from("/lib")]);
        f.new_presentations_dir = Some(PathBuf::from("/lib/OpenPresenter"));
        assert_eq!(
            f.library_folders(),
            [PathBuf::from("/lib")],
            "already scanned"
        );
        f.new_presentations_dir = Some(PathBuf::from("/opres"));
        assert_eq!(
            f.library_folders(),
            [PathBuf::from("/lib"), PathBuf::from("/opres")]
        );
        assert_eq!(f.new_presentations_folder(), Some(PathBuf::from("/opres")));
        let missing = Settings::load(Path::new("/definitely/not/here.toml")).unwrap();
        assert_eq!(missing, Settings::default());
    }
}
