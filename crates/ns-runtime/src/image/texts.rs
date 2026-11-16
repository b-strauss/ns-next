//! The image's text table: the text of every declaration the image
//! carries, each with its nested declarations cut out (`DeclarationText`),
//! written only when the image carries its program's source
//! (`encode::FLAG_HAS_SOURCE`). It follows the code table, and each mixin
//! row names its text by index (`mixins::encode`).
//!
//! A text is written as the parts it answers -- its range, its holes and
//! its text with the holes cut out -- so that an image carries no
//! declaration's text twice: a module's text is assembled from its
//! declarations' after the load (`Heap::assembled_text`), never stored
//! beside them.

use std::collections::HashSet;
use std::rc::Rc;

use ns_syntax::Span;

use crate::mixin::{DeclarationText, Mixin};

use super::decode::{DecodeError, Reader, bounded_count};
use super::encode::{Writer, u32_field};
use super::mixins::Row;

/// Each row's text index, by row id.
pub struct TextIds {
    ids: Vec<Option<u32>>,
}

impl TextIds {
    /// The index of the text of row `row`, which declares `mixin`. Every
    /// row of a module asked for source has one, since `encode` refused
    /// the image otherwise; an `Err` rather than a panic all the same, as
    /// every other refusal of the encoder.
    pub fn id_of(&self, row: usize, mixin: &Mixin) -> Result<u32, String> {
        self.ids.get(row).copied().flatten().ok_or_else(|| {
            format!(
                "image encoding cannot carry the source of `{}`, which was built without it",
                mixin.name
            )
        })
    }
}

/// Writes the table: one text per instance-side declaration among `rows`
/// (`mixins::plan`), in row order, and answers each row's index -- a
/// class side's its instance side's. The rows describe each declaration's
/// current version, so an edited program's image holds the text of each
/// declaration once, as it now is. This is the shape the decoder requires
/// (`mixins::row_texts`): a text belongs to one declaration, so no two
/// instance sides name one text even where two hold one allocation. A row
/// whose declaration holds no text -- one built without source -- is
/// refused.
pub(crate) fn encode(writer: &mut Writer, rows: &[Row]) -> Result<TextIds, String> {
    let mut texts: Vec<Rc<DeclarationText>> = Vec::new();
    let mut ids: Vec<Option<u32>> = vec![None; rows.len()];
    let class_sides: HashSet<u32> = rows.iter().filter_map(|row| row.class_side).collect();
    let nested: HashSet<u32> = rows
        .iter()
        .flat_map(|row| row.nested.iter().copied())
        .collect();
    // The rows are in preorder, module after module, so the module a row
    // belongs to is the last row that is no row's child.
    let mut module = None;
    for (index, row) in rows.iter().enumerate() {
        let row_id = u32_field("mixin table id", index);
        if class_sides.contains(&row_id) {
            continue;
        }
        if !nested.contains(&row_id) {
            module = Some(&row.mixin.name);
        }
        let Some(text) = &row.mixin.text else {
            return Err(format!(
                "image encoding cannot carry the source of module `{}`, which was built \
                 without it",
                module.unwrap_or(&row.mixin.name)
            ));
        };
        let id = u32_field("text table id", texts.len());
        texts.push(Rc::clone(text));
        ids[index] = Some(id);
        if let Some(class_side) = row.class_side {
            ids[class_side as usize] = Some(id);
        }
    }
    writer.write_u32(u32_field("text table entry count", texts.len()));
    for text in &texts {
        let range = text.range();
        writer.write_u32(range.start);
        writer.write_u32(range.end);
        writer.write_u32(u32_field("text's hole count", text.holes().len()));
        for hole in text.holes() {
            writer.write_u32(hole.start);
            writer.write_u32(hole.end);
        }
        writer.write_str(&text.cut_text());
    }
    Ok(TextIds { ids })
}

/// The smallest a text record can be: a range's two `u32`s, a hole count
/// and an empty text's length.
const MIN_TEXT_BYTES: usize = 4 * 4;

/// The smallest a hole can be: its two `u32`s.
const HOLE_BYTES: usize = 2 * 4;

/// Reads the table, each text checked against itself as it is read
/// (`DeclarationText::from_parts`): a text whose parts do not describe a
/// declaration is `MalformedText`, before any mixin holds it. Every count
/// is bounded by the bytes left before anything is reserved for it.
///
/// What a rejected load can be made to allocate here, against the table's
/// bytes (`docs/design/memory.md`, "The amplification bound of a rejected
/// load"): an empty text without holes, the smallest record at 16 bytes,
/// becomes a 72-byte `Rc<DeclarationText>` (56 bytes and the two counts)
/// and an 8-byte slot of the answer, and `mixins::row_texts` adds a 16-byte
/// owner entry per text while it checks the rows, **6x**; the slot and the
/// entry are dropped before the graph pass, so 4.5x is held through it. A
/// hole costs its 8 bytes and a text's bytes their length, 1x. The
/// answer's reservation is for the count the table claims, at most 8 bytes
/// for every 16 after the count, and a count past the records is refused
/// as soon as they run out, before the graph pass.
pub fn decode(reader: &mut Reader) -> Result<Vec<Rc<DeclarationText>>, DecodeError> {
    let count = bounded_count(reader.read_u32()? as usize, MIN_TEXT_BYTES, reader)?;
    let mut texts = Vec::with_capacity(count);
    for index in 0..count {
        let text = u32::try_from(index).unwrap_or(u32::MAX);
        let range = read_span(reader, text)?;
        let hole_count = bounded_count(reader.read_u32()? as usize, HOLE_BYTES, reader)?;
        let mut holes = Vec::with_capacity(hole_count);
        for _ in 0..hole_count {
            holes.push(read_span(reader, text)?);
        }
        let cut_text: Box<str> = Box::from(reader.read_text()?);
        let declaration_text = DeclarationText::from_parts(range, holes, cut_text)
            .ok_or(DecodeError::MalformedText { text })?;
        texts.push(Rc::new(declaration_text));
    }
    Ok(texts)
}

/// Two `u32`s as a span of text `text`; a span that ends before it
/// starts, which `Span` cannot hold, is `MalformedText`.
fn read_span(reader: &mut Reader, text: u32) -> Result<Span, DecodeError> {
    let start = reader.read_u32()?;
    let end = reader.read_u32()?;
    if start > end {
        return Err(DecodeError::MalformedText { text });
    }
    Ok(Span::new(start, end))
}
