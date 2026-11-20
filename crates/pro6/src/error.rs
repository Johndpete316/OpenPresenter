//! Typed parse errors. Every error carries the file, the XML element path and,
//! where known, the UUID of the nearest enclosing slide/group/element.

use std::fmt;
use std::path::{Path, PathBuf};

/// A failure to read a `.pro6` file. Boxed so `Result<_, ParseError>` stays small.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ParseError(Box<ParseErrorDetail>);

#[derive(Debug, thiserror::Error)]
#[error("{file}: {kind} (at {xml_path}{uuid})", file = .file.display(), uuid = UuidSuffix(.uuid.as_deref()))]
pub struct ParseErrorDetail {
    pub file: PathBuf,
    /// Element path such as `RVPresentationDocument/array[groups]/RVSlideGrouping[1]`.
    /// Indices are 0-based among element siblings.
    pub xml_path: String,
    /// UUID of the nearest enclosing node that has one.
    pub uuid: Option<String>,
    #[source]
    pub kind: ParseErrorKind,
}

struct UuidSuffix<'a>(Option<&'a str>);
impl fmt::Display for UuidSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(u) => write!(f, ", uuid {u}"),
            None => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseErrorKind {
    #[error("could not read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("file is not valid UTF-8 (byte offset {offset})")]
    Encoding { offset: usize },
    #[error("malformed XML at line {line}, column {column}: {message}")]
    Xml {
        message: String,
        line: usize,
        column: usize,
    },
    #[error("root element is <{found}>, expected <RVPresentationDocument>")]
    NotPro6Root { found: String },
    #[error("required attribute `{attr}` is missing")]
    MissingAttr { attr: &'static str },
    #[error("attribute `{attr}` has invalid value {value:?} (expected {expected})")]
    BadAttr {
        attr: String,
        value: String,
        expected: &'static str,
    },
    #[error("required child <{element}> is missing")]
    MissingChild { element: String },
    #[error("`{field}` payload is not valid base64: {message}")]
    Base64 { field: String, message: String },
    #[error("element position {value:?} is not a valid RVRect3D `{{x y z w h}}`")]
    BadRect { value: String },
    #[error("RTF text is malformed: {message}")]
    Rtf { message: String },
}

impl ParseError {
    pub(crate) fn new(ctx: &Ctx<'_>, kind: ParseErrorKind) -> Self {
        let d = ParseErrorDetail {
            file: ctx.file.to_path_buf(),
            xml_path: ctx.path.clone(),
            uuid: ctx.uuid.clone(),
            kind,
        };
        tracing::warn!(
            file = %d.file.display(),
            xml_path = %d.xml_path,
            uuid = d.uuid.as_deref().unwrap_or(""),
            error = %d.kind,
            "pro6 parse error"
        );
        ParseError(Box::new(d))
    }

    pub fn file(&self) -> &Path {
        &self.0.file
    }
    pub fn xml_path(&self) -> &str {
        &self.0.xml_path
    }
    pub fn uuid(&self) -> Option<&str> {
        self.0.uuid.as_deref()
    }
    pub fn kind(&self) -> &ParseErrorKind {
        &self.0.kind
    }

    /// One plain sentence for the operator, plus what happens next.
    pub fn user_message(&self) -> String {
        let name = self
            .0
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.0.file.display().to_string());
        let what = match &self.0.kind {
            ParseErrorKind::Io(e) => match e.kind() {
                std::io::ErrorKind::NotFound => "the file no longer exists".to_owned(),
                std::io::ErrorKind::PermissionDenied => {
                    "this PC isn't allowed to read it (permission denied)".to_owned()
                }
                _ => format!("it couldn't be read from disk ({e})"),
            },
            ParseErrorKind::Encoding { .. } => {
                "it contains characters that aren't valid text (not UTF-8)".to_owned()
            }
            ParseErrorKind::Xml { line, .. } => {
                format!("the file is damaged or incomplete near line {line}")
            }
            ParseErrorKind::NotPro6Root { .. } => {
                "it isn't a ProPresenter 6 presentation".to_owned()
            }
            ParseErrorKind::MissingAttr {
                attr: a @ ("width" | "height"),
            } => {
                format!("it doesn't say what size its slides are (no canvas `{a}`)")
            }
            ParseErrorKind::MissingAttr { .. }
            | ParseErrorKind::BadAttr { .. }
            | ParseErrorKind::MissingChild { .. }
            | ParseErrorKind::BadRect { .. } => {
                format!("{} has invalid layout data", self.location_phrase())
            }
            ParseErrorKind::Base64 { .. } | ParseErrorKind::Rtf { .. } => {
                format!("the text of {} is corrupted", self.location_phrase())
            }
        };
        format!(
            "'{name}' couldn't be opened: {what}. The file was skipped; other files are unaffected."
        )
    }

    /// "slide 3 in group 2" style description derived from the XML path.
    fn location_phrase(&self) -> String {
        let mut group = None;
        let mut slide = None;
        for seg in self.0.xml_path.split('/') {
            if let Some(i) = index_of(seg, "RVSlideGrouping") {
                group = Some(i + 1);
            } else if let Some(i) = index_of(seg, "RVDisplaySlide") {
                slide = Some(i + 1);
            }
        }
        match (slide, group) {
            (Some(s), Some(g)) => format!("slide {s} in group {g}"),
            (None, Some(g)) => format!("slide group {g}"),
            _ => "the presentation".to_owned(),
        }
    }
}

fn index_of(seg: &str, name: &str) -> Option<usize> {
    seg.strip_prefix(name)?
        .strip_prefix('[')?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// Parse context: which file, where in the tree, and nearest UUID.
#[derive(Clone)]
pub(crate) struct Ctx<'a> {
    pub file: &'a Path,
    pub path: String,
    pub uuid: Option<String>,
}

impl<'a> Ctx<'a> {
    pub fn root(file: &'a Path) -> Self {
        Ctx {
            file,
            path: String::new(),
            uuid: None,
        }
    }

    /// Descend into a child path segment.
    pub fn child(&self, segment: impl fmt::Display) -> Self {
        let path = if self.path.is_empty() {
            segment.to_string()
        } else {
            format!("{}/{segment}", self.path)
        };
        Ctx {
            file: self.file,
            path,
            uuid: self.uuid.clone(),
        }
    }

    pub fn with_uuid(mut self, uuid: Option<&str>) -> Self {
        if let Some(u) = uuid.filter(|u| !u.is_empty()) {
            self.uuid = Some(u.to_owned());
        }
        self
    }

    pub fn err(&self, kind: ParseErrorKind) -> ParseError {
        ParseError::new(self, kind)
    }
}
