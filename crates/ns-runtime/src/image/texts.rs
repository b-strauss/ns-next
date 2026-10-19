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

use crate::mixin::{DeclarationText, Mixin};

use super::decode::{bounded_count, DecodeError, Reader};
use super::encode::{u32_field, Writer};

/// A text's key: its allocation, so that the texts the rows share -- a
/// class side's is its instance side's -- are written once.
fn text_key(text: &Rc<DeclarationText>) -> usize {
    Rc::as_ptr(text) as usize
}

/// Each text's index in the table, by `text_key`.
pub struct TextIds {
    ids: HashMap<usize, u32>,
}

impl TextIds {
    /// The index of `mixin`'s text. Every mixin of a module asked for
    /// source has one in the table, since `encode` refused the image
    /// otherwise; an `Err` rather than a panic all the same, as every
    /// other refusal of the encoder.
    pub fn id_of(&self, mixin: &Mixin) -> Result<u32, String> {
        mixin
            .text
            .as_ref()
            .and_then(|text| self.ids.get(&text_key(text)).copied())
            .ok_or_else(|| {
                format!(
                    "image encoding cannot carry the source of `{}`, which was built without it",
                    mixin.name
                )
            })
    }
}

/// Writes the table: every distinct text the declarations under
/// `module_roots` hold, in the order the mixin table's rows reach them.
/// A module one of whose declarations holds no text -- one built without
/// source -- is refused.
pub(crate) fn encode(writer: &mut Writer, module_roots: &[Rc<Mixin>]) -> Result<TextIds, String> {
    let mut texts: Vec<Rc<DeclarationText>> = Vec::new();
    let mut ids: HashMap<usize, u32> = HashMap::new();
    for root in module_roots {
        let mut tree = Vec::new();
        crate::mixin::preorder(root, &mut tree);
        for mixin in &tree {
            let Some(text) = &mixin.text else {
                return Err(format!(
                    "image encoding cannot carry the source of module `{}`, which was built \
                     without it",
                    root.name
                ));
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = ids.entry(text_key(text)) {
                entry.insert(u32_field("text table id", texts.len()));
                texts.push(Rc::clone(text));
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
