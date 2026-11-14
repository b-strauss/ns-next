//! The open documents' text, and the conversion from the byte offsets every
//! `Span` holds to the positions LSP speaks.

use lsp_types::Position;
use ns_tooling::lines::{self, LineColumn};

pub use ns_tooling::lines::utf16_length;

/// The tooling crate's `LineIndex` answering in the positions LSP speaks.
pub struct LineIndex(lines::LineIndex);

impl LineIndex {
    /// The index of `text`'s line starts (`ns_tooling::lines::LineIndex::new`).
    pub fn new(text: &str) -> LineIndex {
        LineIndex(lines::LineIndex::new(text))
    }

    /// The LSP position of byte `offset` in `text`, clamped as
    /// `ns_tooling::lines::LineIndex::position` clamps an offset past the end
    /// or inside a character.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let LineColumn { line, column } = self.0.position(text, offset);
        Position::new(line, column)
    }

    /// The byte offset of LSP `position` in `text`, `None` for a line the
    /// text does not have; a column past its line's end or inside a
    /// surrogate pair is clamped as `ns_tooling::lines::LineIndex::offset`
    /// clamps it.
    pub fn offset(&self, text: &str, position: Position) -> Option<u32> {
        let line_column = LineColumn {
            line: position.line,
            column: position.character,
        };
        self.0.offset(text, line_column)
    }
}
