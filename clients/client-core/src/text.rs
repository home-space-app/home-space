//! Decoding text files for the text viewer and encoding the edited text back.

use crate::viewers::Editing;

const BOM: &[u8] = b"\xEF\xBB\xBF";
const CRLF: &str = "\r\n";
const LF: &str = "\n";

/// How lines end in a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => LF,
            Self::CrLf => CRLF,
        }
    }
}

/// A text file ready to show, with what is needed to write it back the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDoc {
    /// The content with `\n` line breaks.
    pub text: String,
    pub line_ending: LineEnding,
    /// Whether the file started with a UTF-8 byte order mark.
    pub bom: bool,
    /// Content that is not valid UTF-8 is shown with replacement characters and cannot be
    /// edited, since saving it would change the bytes that could not be decoded.
    pub editing: Editing,
}

impl TextDoc {
    /// Decodes the content of a text file.
    pub fn decode(bytes: &[u8]) -> Self {
        let (bom, body) = match bytes.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        let (raw, editing) = match std::str::from_utf8(body) {
            Ok(text) => (text.to_owned(), Editing::Editable),
            Err(_) => (
                String::from_utf8_lossy(body).into_owned(),
                Editing::ReadOnly,
            ),
        };
        Self {
            line_ending: line_ending(&raw),
            text: raw.replace(CRLF, LF),
            bom,
            editing,
        }
    }

    /// Whether the text can be edited and saved.
    pub fn is_editable(&self) -> bool {
        self.editing == Editing::Editable
    }

    /// The bytes to store for `edited`, with this file's byte order mark and line endings.
    pub fn encode(&self, edited: &str) -> Vec<u8> {
        let normalized = edited.replace(CRLF, LF);
        let body = match self.line_ending {
            LineEnding::Lf => normalized,
            LineEnding::CrLf => normalized.replace(LF, self.line_ending.as_str()),
        };
        let bom = if self.bom { BOM } else { &[] };
        [bom, body.as_bytes()].concat()
    }

    /// This document after `edited` was saved.
    pub fn with_text(&self, edited: &str) -> Self {
        Self {
            text: edited.replace(CRLF, LF),
            ..self.clone()
        }
    }
}

/// CRLF only when every line break in `text` is one; mixed files are saved with LF.
fn line_ending(text: &str) -> LineEnding {
    let breaks = text.matches(LF).count();
    let crlf = text.matches(CRLF).count();
    if breaks > 0 && breaks == crlf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_text_is_editable() {
        let doc = TextDoc::decode("héllo\n".as_bytes());
        assert_eq!(doc.text, "héllo\n");
        assert!(doc.is_editable());
    }

    #[test]
    fn invalid_utf8_is_read_only() {
        let doc = TextDoc::decode(b"a\xFFb");
        assert_eq!(doc.text, "a\u{FFFD}b");
        assert!(!doc.is_editable());
    }

    #[test]
    fn the_byte_order_mark_is_hidden_and_restored() {
        let doc = TextDoc::decode(b"\xEF\xBB\xBFhi");
        assert_eq!(doc.text, "hi");
        assert_eq!(doc.encode("ho"), b"\xEF\xBB\xBFho");
    }

    #[test]
    fn crlf_files_are_shown_with_lf_and_saved_with_crlf() {
        let doc = TextDoc::decode(b"a\r\nb\r\n");
        assert_eq!(doc.text, "a\nb\n");
        assert_eq!(doc.encode("a\nb\nc\n"), b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn mixed_line_endings_are_saved_with_lf() {
        let doc = TextDoc::decode(b"a\r\nb\nc");
        assert_eq!(doc.line_ending, LineEnding::Lf);
        assert_eq!(doc.encode(&doc.text), b"a\nb\nc");
    }

    #[test]
    fn files_without_line_breaks_are_saved_with_lf() {
        let doc = TextDoc::decode(b"one line");
        assert_eq!(doc.encode("two\nlines"), b"two\nlines");
    }

    #[test]
    fn crlf_typed_into_an_lf_file_is_saved_as_lf() {
        let doc = TextDoc::decode(b"a\n");
        assert_eq!(doc.encode("a\r\nb"), b"a\nb");
    }

    #[test]
    fn with_text_keeps_the_encoding() {
        let doc = TextDoc::decode(b"\xEF\xBB\xBFa\r\n").with_text("b\n");
        assert_eq!(doc.encode(&doc.text), b"\xEF\xBB\xBFb\r\n");
    }
}
