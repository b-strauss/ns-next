//! A `String`'s payload: its code points, in one of two widths
//! (`docs/design/core-library.md`, "Strings are code points in two widths").

/// A string's elements (§5.1.5), each a Unicode scalar value, stored one
/// byte each while every one fits Latin-1 and 32 bits each otherwise, so
/// that `size` and `at:` are one access.
///
/// Canonical: a string whose code points all fit Latin-1 is always narrow.
/// Equal strings therefore have equal representations, so `=` compares
/// them directly and `hash` agrees with it. Every constructor keeps that,
/// which is why the representation is private.
///
/// Every element is a scalar value -- at most U+10FFFF and never a
/// surrogate -- so the conversion to UTF-8 at every boundary (the
/// transcript, the image, an error text) is total.
#[derive(Clone, PartialEq, Eq)]
pub struct CodePoints(Representation);

#[derive(Clone, PartialEq, Eq)]
enum Representation {
    Narrow(Box<[u8]>),
    Wide(Box<[u32]>),
}

impl CodePoints {
    /// One code point; `None` for a surrogate or a value past U+10FFFF.
    pub fn from_code_point(code_point: u32) -> Option<CodePoints> {
        let character = char::from_u32(code_point)?;
        Some(match u8::try_from(u32::from(character)) {
            Ok(byte) => CodePoints(Representation::Narrow(Box::new([byte]))),
            Err(_) => CodePoints(Representation::Wide(Box::new([code_point]))),
        })
    }

    /// Latin-1 code points, one byte each; every byte is one, so this
    /// needs no check.
    pub(crate) fn from_bytes(bytes: Box<[u8]>) -> CodePoints {
        CodePoints(Representation::Narrow(bytes))
    }

    /// Code points 32 bits each, narrowed when every one fits Latin-1, so
    /// the result stays canonical. Each must be a scalar value: the caller
    /// read them out of a string that held them.
    pub(crate) fn from_wide(words: Box<[u32]>) -> CodePoints {
        debug_assert!(words.iter().all(|word| char::from_u32(*word).is_some()));
        if words.iter().all(|word| *word <= 0xFF) {
            CodePoints::from_words(&words)
        } else {
            CodePoints(Representation::Wide(words))
        }
    }

    /// The elements, one byte each, when the string is narrow.
    pub(crate) fn as_bytes(&self) -> Option<&[u8]> {
        match &self.0 {
            Representation::Narrow(bytes) => Some(bytes),
            Representation::Wide(_) => None,
        }
    }

    /// The elements, 32 bits each, when the string is wide.
    pub(crate) fn as_wide(&self) -> Option<&[u32]> {
        match &self.0 {
            Representation::Narrow(_) => None,
            Representation::Wide(words) => Some(words),
        }
    }

    pub fn size(&self) -> usize {
        match &self.0 {
            Representation::Narrow(bytes) => bytes.len(),
            Representation::Wide(words) => words.len(),
        }
    }

    /// The code point at the 0-based `index`; `None` past the end.
    pub fn code_point_at(&self, index: usize) -> Option<u32> {
        match &self.0 {
            Representation::Narrow(bytes) => bytes.get(index).map(|byte| u32::from(*byte)),
            Representation::Wide(words) => words.get(index).copied(),
        }
    }

    /// The elements in the 0-based, half-open `start..end`; `None` when the
    /// range is not inside the string. A slice of a wide string whose
    /// elements all fit Latin-1 is narrowed, which keeps it canonical.
    pub fn slice(&self, start: usize, end: usize) -> Option<CodePoints> {
        match &self.0 {
            Representation::Narrow(bytes) => Some(CodePoints(Representation::Narrow(
                bytes.get(start..end)?.into(),
            ))),
            Representation::Wide(words) => Some(CodePoints::from_words(words.get(start..end)?)),
        }
    }

    /// Canonical without a scan: a wide operand holds a code point past
    /// U+00FF, and so does the result.
    pub fn concatenate(&self, other: &CodePoints) -> CodePoints {
        match (&self.0, &other.0) {
            (Representation::Narrow(first), Representation::Narrow(second)) => {
                let mut bytes = Vec::with_capacity(first.len() + second.len());
                bytes.extend_from_slice(first);
                bytes.extend_from_slice(second);
                CodePoints(Representation::Narrow(bytes.into_boxed_slice()))
            }
            _ => {
                let mut words = Vec::with_capacity(self.size() + other.size());
                words.extend(self.elements());
                words.extend(other.elements());
                CodePoints(Representation::Wide(words.into_boxed_slice()))
            }
        }
    }

    pub fn elements(&self) -> impl Iterator<Item = u32> + '_ {
        let (narrow, wide): (&[u8], &[u32]) = match &self.0 {
            Representation::Narrow(bytes) => (bytes, &[]),
            Representation::Wide(words) => (&[], words),
        };
        narrow
            .iter()
            .map(|byte| u32::from(*byte))
            .chain(wide.iter().copied())
    }

    /// Every element is a scalar value by construction, so the replacement
    /// character is never produced; it stands in for a panic.
    pub fn characters(&self) -> impl Iterator<Item = char> + '_ {
        self.elements()
            .map(|code_point| char::from_u32(code_point).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// The hash of the UTF-8 text, `hash::content_hash` of a `&str`, so that
    /// a String hashes as it did when it was one, and as the class names
    /// `builtins::value` hashes through `text::string_hash` do.
    pub fn content_hash(&self) -> i64 {
        match &self.0 {
            // ASCII is its own UTF-8.
            Representation::Narrow(bytes) if bytes.is_ascii() => {
                crate::hash::content_hash(std::str::from_utf8(bytes).unwrap_or_default())
            }
            _ => crate::hash::content_hash_of_characters(self.characters()),
        }
    }

    /// The text as UTF-8. ASCII is its own UTF-8, so a narrow ASCII string
    /// hands its bytes over in one copy; any other string is encoded
    /// character by character.
    pub fn to_utf8(&self) -> String {
        match &self.0 {
            Representation::Narrow(bytes) if bytes.is_ascii() => {
                String::from_utf8(bytes.to_vec()).unwrap_or_default()
            }
            _ => self.characters().collect(),
        }
    }

    fn from_words(words: &[u32]) -> CodePoints {
        if words.iter().all(|word| *word <= 0xFF) {
            let bytes: Vec<u8> = words
                .iter()
                .filter_map(|word| u8::try_from(*word).ok())
                .collect();
            CodePoints(Representation::Narrow(bytes.into_boxed_slice()))
        } else {
            CodePoints(Representation::Wide(words.into()))
        }
    }
}

/// Counts the code points first and allocates exactly, so that a decoded
/// string never holds more than 4 heap bytes per wire byte
/// (`docs/design/memory.md`, the decode amplification).
impl From<&str> for CodePoints {
    fn from(text: &str) -> CodePoints {
        if text.is_ascii() {
            return CodePoints(Representation::Narrow(text.as_bytes().into()));
        }
        let count = text.chars().count();
        if text.chars().all(|character| u32::from(character) <= 0xFF) {
            let mut bytes = Vec::with_capacity(count);
            bytes.extend(
                text.chars()
                    .filter_map(|character| u8::try_from(u32::from(character)).ok()),
            );
            CodePoints(Representation::Narrow(bytes.into_boxed_slice()))
        } else {
            let mut words = Vec::with_capacity(count);
            words.extend(text.chars().map(u32::from));
            CodePoints(Representation::Wide(words.into_boxed_slice()))
        }
    }
}

/// An ASCII `String` hands its buffer over.
impl From<String> for CodePoints {
    fn from(text: String) -> CodePoints {
        if text.is_ascii() {
            return CodePoints(Representation::Narrow(text.into_bytes().into_boxed_slice()));
        }
        CodePoints::from(text.as_str())
    }
}

impl std::fmt::Display for CodePoints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_utf8())
    }
}

/// The text `{:?}` of a Rust `String` gives, so `Heap::describe` reads as
/// it did.
impl std::fmt::Debug for CodePoints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.to_string(), formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::CodePoints;

    const TEXTS: [&str; 10] = [
        "",
        "a",
        "abcdefgh",
        "abcdefghi",
        "café",
        "naïve façade",
        "ÿ",
        "€uro",
        "😀 x",
        "Ã©",
    ];

    #[test]
    fn the_hash_is_the_hash_of_the_utf8_text() {
        for text in TEXTS {
            assert_eq!(
                CodePoints::from(text).content_hash(),
                crate::hash::content_hash(text),
                "{text}"
            );
        }
    }

    #[test]
    fn the_text_survives_the_round_trip() {
        for text in TEXTS {
            assert_eq!(CodePoints::from(text).to_string(), text);
            assert_eq!(CodePoints::from(text).to_utf8(), text, "{text}");
            assert_eq!(CodePoints::from(text.to_string()).to_string(), text);
            assert_eq!(
                CodePoints::from(text).size(),
                text.chars().count(),
                "{text}"
            );
        }
    }

    #[test]
    fn latin1_text_is_narrow_whatever_built_it() {
        let wide = CodePoints::from("€abc");
        assert_eq!(wide.slice(1, 4), Some(CodePoints::from("abc")));
        assert_eq!(
            CodePoints::from("caf").concatenate(&CodePoints::from("é")),
            CodePoints::from("café")
        );
        assert_eq!(
            CodePoints::from_code_point(0xE9),
            Some(CodePoints::from("é"))
        );
        assert_ne!(CodePoints::from("€"), CodePoints::from("e"));
    }

    #[test]
    fn units_read_back_into_the_canonical_width() {
        for text in TEXTS {
            let code_points = CodePoints::from(text);
            let rebuilt = match (code_points.as_bytes(), code_points.as_wide()) {
                (Some(bytes), None) => CodePoints::from_bytes(bytes.into()),
                (None, Some(words)) => CodePoints::from_wide(words.into()),
                _ => unreachable!("a string is narrow or wide"),
            };
            assert_eq!(rebuilt, code_points, "{text}");
        }
        assert_eq!(
            CodePoints::from_wide(Box::new([0x63, 0xE9])),
            CodePoints::from("cé")
        );
    }

    #[test]
    fn a_concatenation_with_a_wide_operand_is_wide_and_equal_to_its_literal() {
        let joined = CodePoints::from("café").concatenate(&CodePoints::from("€"));
        assert_eq!(joined, CodePoints::from("café€"));
        assert_eq!(
            joined.content_hash(),
            CodePoints::from("café€").content_hash()
        );
        assert_eq!(joined.code_point_at(4), Some(0x20AC));
    }

    #[test]
    fn only_scalar_values_are_code_points() {
        assert_eq!(CodePoints::from_code_point(0xD800), None);
        assert_eq!(CodePoints::from_code_point(0xDFFF), None);
        assert_eq!(CodePoints::from_code_point(0x11_0000), None);
        assert_eq!(
            CodePoints::from_code_point(0x10_FFFF).map(|code_points| code_points.size()),
            Some(1)
        );
        assert_eq!(
            CodePoints::from_code_point(0).map(|code_points| code_points.size()),
            Some(1)
        );
    }

    #[test]
    fn a_slice_outside_the_string_is_none() {
        let text = CodePoints::from("abc");
        assert_eq!(text.slice(1, 4), None);
        assert_eq!(text.slice(2, 1), None);
        assert_eq!(text.slice(3, 3), Some(CodePoints::from("")));
        assert_eq!(text.code_point_at(3), None);
    }
}
