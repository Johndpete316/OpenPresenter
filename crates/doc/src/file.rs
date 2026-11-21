//! Loading and saving: `.opres` JSON (versioned, with migrations) and `.pro6`.
//!
//! Save rules (docs/EDITOR-PLAN.md):
//! - `Pro6` mode → the `.pro6` file (previous version kept as `<name>.pro6.bak`).
//! - `Native` mode → `<name>.opres` next to the original, which is never touched.
//!   From then on, opening the `.pro6` opens the `.opres` (see [`preferred`]).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::compat::Mode;
use crate::convert::{ExportError, from_pro6, to_pro6};
use crate::model::Document;
use crate::{FORMAT_VERSION, NATIVE_EXT};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Pro6(#[from] pro6::ParseError),
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: not a readable presentation: {message}", path = .path.display())]
    Json { path: PathBuf, message: String },
    #[error("{path} was saved by a newer version (format {found}, this version reads up to {FORMAT_VERSION})", path = .path.display())]
    Newer { path: PathBuf, found: u64 },
}

impl LoadError {
    pub fn user_message(&self) -> String {
        match self {
            LoadError::Pro6(e) => e.user_message(),
            LoadError::Io { path, source } => {
                format!("'{}' couldn't be opened ({source}).", path.display())
            }
            LoadError::Json { path, message } => format!(
                "'{}' is damaged or isn't an OpenPresenter presentation ({message}). It wasn't opened.",
                path.display()
            ),
            LoadError::Newer { path, .. } => format!(
                "'{}' was saved by a newer version of OpenPresenter. Update this program to open it.",
                path.display()
            ),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error(transparent)]
    Pro6(#[from] pro6::WriteError),
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("backup of {path} failed: {source}", path = .path.display())]
    Backup {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl SaveError {
    pub fn user_message(&self) -> String {
        match self {
            SaveError::Export(e) => e.user_message(),
            SaveError::Pro6(e) => e.user_message(),
            SaveError::Io { path, source } => format!(
                "'{}' wasn't saved ({source}). Your edits are still open.",
                path.display()
            ),
            SaveError::Backup { path, source } => format!(
                "'{}' wasn't saved: the backup copy couldn't be made ({source}). Your edits are still open; nothing on disk changed.",
                path.display()
            ),
        }
    }
}

/// The file to open for `path`: a `.opres` saved next to a `.pro6` replaces it.
pub fn preferred(path: &Path) -> PathBuf {
    if is_pro6(path) {
        let native = path.with_extension(NATIVE_EXT);
        if native.is_file() {
            return native;
        }
    }
    path.to_path_buf()
}

pub fn is_pro6(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pro6"))
}

pub fn is_native(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(NATIVE_EXT))
}

/// Open a presentation, preferring a `.opres` next to a `.pro6` (see [`preferred`]).
pub fn open(path: &Path) -> Result<Document, LoadError> {
    load(&preferred(path))
}

/// Load exactly `path` (`.opres` by extension, a theme as its one-slide document,
/// otherwise `.pro6`).
pub fn load(path: &Path) -> Result<Document, LoadError> {
    let _span = tracing::info_span!("load", file = %path.display()).entered();
    if crate::theme::is_theme(path) {
        return crate::Theme::load(path)
            .map(|t| t.to_document())
            .map_err(|e| LoadError::Json {
                path: path.to_path_buf(),
                message: e.to_string(),
            });
    }
    if is_native(path) {
        let bytes = std::fs::read(path).map_err(|source| LoadError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        from_json(&bytes, path)
    } else {
        Ok(from_pro6(pro6::load(path)?))
    }
}

/// Migrations from version N to N+1, indexed by N - 1.
const MIGRATIONS: &[fn(&mut serde_json::Value)] = &[];

/// Parse `.opres` JSON, migrating older versions.
pub fn from_json(bytes: &[u8], path: &Path) -> Result<Document, LoadError> {
    let json = |message: String| LoadError::Json {
        path: path.to_path_buf(),
        message,
    };
    let mut v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| json(e.to_string()))?;
    let version = v
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| json("no format_version".into()))?;
    if version > u64::from(FORMAT_VERSION) {
        return Err(LoadError::Newer {
            path: path.to_path_buf(),
            found: version,
        });
    }
    for (i, m) in MIGRATIONS.iter().enumerate() {
        let from = i as u64 + 1;
        if version <= from {
            m(&mut v);
            tracing::info!(from, to = from + 1, "migrated .opres");
        }
    }
    if let Some(obj) = v.as_object_mut() {
        obj.insert("format_version".into(), FORMAT_VERSION.into());
    }
    let mut doc: Document = serde_json::from_value(v).map_err(|e| json(e.to_string()))?;
    doc.path = path.to_path_buf();
    if let Some(src) = doc.pro6.take() {
        let mut src = Arc::unwrap_or_clone(src);
        src.path = path.to_path_buf();
        doc.pro6 = Some(Arc::new(src));
    }
    Ok(doc)
}

pub fn to_json(doc: &Document) -> Vec<u8> {
    // Serializing plain data structures can't fail.
    serde_json::to_vec_pretty(doc).unwrap_or_default()
}

/// Write atomically: temp file in the same folder, flush to disk, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Copy `path` to `<name>.bak` (replacing an older backup) if it exists.
fn backup(path: &Path) -> Result<(), SaveError> {
    if path.exists() {
        let mut name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".bak");
        std::fs::copy(path, path.with_file_name(name)).map_err(|source| SaveError::Backup {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Where a document is saved under the save rules.
pub fn save_path(doc: &Document) -> PathBuf {
    if doc.theme.is_some() && crate::theme::is_theme(&doc.path) {
        return doc.path.clone();
    }
    match doc.mode {
        // A `.opres` is always saved as one (never `.pro6` XML into it).
        _ if is_native(&doc.path) => doc.path.clone(),
        Mode::Pro6 => doc.path.clone(),
        Mode::Native => doc.path.with_extension(NATIVE_EXT),
    }
}

/// Save under the save rules. Returns the document as saved (path updated; a
/// `.pro6` save is re-read from what was written, so later saves build on it).
pub fn save(doc: &Document) -> Result<Document, SaveError> {
    let target = save_path(doc);
    let _span = tracing::info_span!("save", file = %target.display(), mode = ?doc.mode).entered();
    if doc.theme.is_some() && crate::theme::is_theme(&target) {
        let theme = crate::Theme::from_document(doc).map_err(|e| SaveError::Io {
            path: target.clone(),
            source: std::io::Error::other(e.user_message()),
        })?;
        theme.save(&target).map_err(|e| SaveError::Io {
            path: target.clone(),
            source: std::io::Error::other(e.user_message()),
        })?;
        tracing::info!("saved theme");
        return Ok(doc.clone());
    }
    match doc.mode {
        Mode::Pro6 if !is_native(&target) => {
            let mut out = to_pro6(doc)?;
            out.path.clone_from(&target);
            backup(&target)?;
            pro6::save(&out, &target)?;
            tracing::info!("saved .pro6");
            Ok(from_pro6(out))
        }
        Mode::Pro6 | Mode::Native => {
            let mut saved = doc.clone();
            saved.path.clone_from(&target);
            if is_native(&doc.path) {
                backup(&target)?;
            }
            write_atomic(&target, &to_json(&saved)).map_err(|source| SaveError::Io {
                path: target.clone(),
                source,
            })?;
            tracing::info!("saved .opres");
            Ok(saved)
        }
    }
}

/// serde for `Document::pro6`: the original `.pro6` as its XML text.
pub(crate) mod pro6_source {
    use std::path::Path;
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        v: &Option<Arc<pro6::Document>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => s.serialize_some(&pro6::write_string(d)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Arc<pro6::Document>>, D::Error> {
        let Some(xml) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        pro6::parse_str(&xml, Path::new("embedded.pro6"))
            .map(|p| Some(Arc::new(p)))
            .map_err(|e| serde::de::Error::custom(e.user_message()))
    }
}
