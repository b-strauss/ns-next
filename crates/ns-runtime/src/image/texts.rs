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

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::mixin::{mixin_key, DeclarationText, Mixin};

use super::decode::{bounded_count, DecodeError, Reader};
use super::encode::{u32_field, Writer};

/// Each row's text index, by the row's `mixin_key`.
pub struct TextIds {
    ids: HashMap<usize, u32>,
}

impl TextIds {
    /// The index of the text of `mixin`'s row. Every row of a module asked
    /// for source has one, since `encode` refused the image otherwise; an
    /// `Err` rather than a panic all the same, as every other refusal of
    /// the encoder.
    pub fn id_of(&self, mixin: &Rc<Mixin>) -> Result<u32, String> {
        self.ids.get(&mixin_key(mixin)).copied().ok_or_else(|| {
            format!(
                "image encoding cannot carry the source of `{}`, which was built without it",
                mixin.name
            )
        })
    }
}

/// Writes the table: one text per instance-side declaration under
/// `module_roots`, in the order the mixin table's rows reach them, and
/// answers each row's index -- a class side's its instance side's. This is
/// the shape the decoder requires (`mixins::row_texts`): a text belongs to
/// one declaration, so no two instance sides name one text even where two
/// hold one allocation. A module one of whose declarations holds no text
/// -- one built without source -- is refused.
pub(crate) fn encode(writer: &mut Writer, module_roots: &[Rc<Mixin>]) -> Result<TextIds, String> {
    let mut texts: Vec<Rc<DeclarationText>> = Vec::new();
    let mut ids: HashMap<usize, u32> = HashMap::new();
    for root in module_roots {
        let mut tree = Vec::new();
        crate::mixin::preorder(root, &mut tree);
        for mixin in &tree {
            // A class side, registered with its instance side, which
            // `preorder` reaches first.
            if ids.contains_key(&mixin_key(mixin)) {
                continue;
            }
            let Some(text) = &mixin.text else {
                return Err(format!(
                    "image encoding cannot carry the source of module `{}`, which was built                      without it",
                    root.name
                ));
            };
            let id = u32_field("text table id", texts.len());
            texts.push(Rc::clone(text));
            ids.insert(mixin_key(mixin), id);
            if let Some(class_side) = &mixin.class_side {
                ids.insert(mixin_key(class_side), id);
            }
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
