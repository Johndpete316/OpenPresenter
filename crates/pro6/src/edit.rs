//! Editing operations on the typed model.

use std::path::Path;

use crate::error::ParseError;
use crate::extra::{Extra, Leaf};
use crate::model::{Payload, TextElement};
use crate::raw::RawElement;
use crate::rtf::{self, Paragraph, RtfDoc, Run};

/// What to do with the Windows-only `WinFlowData` / `WinFontData` payloads when
/// text changes. Whether PP6 on Windows shows stale text from them instead of
/// `RTFData` is unverified (docs/PLAN.md Unknowns), so callers must choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinPayloads {
    /// Leave them as they were (they will describe the old text).
    Keep,
    /// Remove them. The spec says slides render without them.
    Drop,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("existing slide text can't be read: {0}")]
    ExistingRtf(#[from] ParseError),
    #[error("text element {uuid} has no styled text to take the font and size from")]
    NoStyle { uuid: String },
    #[error("{what} doesn't exist")]
    NotFound { what: String },
    #[error("the presentation has no slide to copy the layout from")]
    NoTemplate,
    #[error("slide {uuid} has no text box")]
    NoTextElement { uuid: String },
}

impl EditError {
    pub fn user_message(&self) -> String {
        match self {
            EditError::ExistingRtf(e) => format!(
                "This text box's current text is damaged, so it can't be edited safely ({}). Nothing was changed.",
                e.kind()
            ),
            EditError::NoStyle { .. } => {
                "This text box has no font or size set, so new text can't be styled to \
                 match. Nothing was changed."
                    .to_owned()
            }
            EditError::NotFound { what } => {
                format!(
                    "That {what} no longer exists (the presentation changed). Nothing was changed."
                )
            }
            EditError::NoTemplate => {
                "This presentation has no slides to copy the font and layout from, so a \
                 new slide can't be made here. Nothing was changed."
                    .to_owned()
            }
            EditError::NoTextElement { .. } => {
                "This slide has no text box to put text in. Nothing was changed.".to_owned()
            }
        }
    }
}

impl TextElement {
    /// Replace the element's text, regenerating `RTFData` and `PlainText`.
    ///
    /// The new text takes the font, size, colour and bold/italic/underline of the
    /// first existing run and the first paragraph's alignment; the font and colour
    /// tables are kept. Each `\n` starts a new paragraph. `PlainText` keeps the
    /// newline style it had (`\r\n`, `\r` or `\n`). UUIDs are not touched.
    /// `file` is used only for error context.
    pub fn set_text(&mut self, text: &str, win: WinPayloads, file: &Path) -> Result<(), EditError> {
        let _span = tracing::info_span!("set_text", uuid = %self.uuid).entered();
        let fail = |e: EditError| {
            tracing::warn!(file = %file.display(), xml_path = %self.xml_path, error = %e, "text edit refused");
            e
        };
        let old = match self.rtf_doc(file) {
            Ok(Some(d)) => d,
            Ok(None) => {
                return Err(fail(EditError::NoStyle {
                    uuid: self.uuid.clone(),
                }));
            }
            Err(e) => return Err(fail(EditError::ExistingRtf(e))),
        };
        let Some(style) = old.runs().next().cloned() else {
            return Err(fail(EditError::NoStyle {
                uuid: self.uuid.clone(),
            }));
        };
        let align = old.paragraphs.first().map(|p| p.align).unwrap_or_default();

        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let paragraphs = text
            .split('\n')
            .map(|line| Paragraph {
                align,
                runs: if line.is_empty() {
                    Vec::new()
                } else {
                    vec![Run {
                        text: line.to_owned(),
                        ..style.clone()
                    }]
                },
            })
            .collect();
        let new = RtfDoc {
            fonts: old.fonts,
            colors: old.colors,
            paragraphs,
            code_page: old.code_page,
            unhandled_words: Default::default(),
        };
        set_payload(&mut self.rtf, "RTFData", rtf::write(&new).into_bytes());

        let newline = match &self.plain_text {
            Some(p) if p.value.bytes.windows(2).any(|w| w == b"\r\n") => "\r\n",
            Some(p) if p.value.bytes.contains(&b'\r') => "\r",
            _ => "\n",
        };
        set_payload(
            &mut self.plain_text,
            "PlainText",
            text.replace('\n', newline).into_bytes(),
        );

        if win == WinPayloads::Drop {
            self.win_flow = None;
            self.win_font = None;
        }
        tracing::info!(chars = text.chars().count(), win_payloads = ?win, "text edited");
        Ok(())
    }
}

fn set_payload(slot: &mut Option<Leaf<Payload>>, ivar: &str, bytes: Vec<u8>) {
    match slot {
        Some(leaf) => leaf.value.bytes = bytes,
        None => {
            *slot = Some(Leaf {
                value: Payload { bytes },
                extra: Extra {
                    element: RawElement::new("NSString", &[("rvXMLIvarName", ivar)]),
                    typed_slots: Vec::new(),
                },
            });
        }
    }
}
