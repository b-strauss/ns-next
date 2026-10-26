//! Byte-offset source spans.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Span {
        debug_assert!(start <= end, "span start after end");
        Span { start, end }
    }

    /// A span of byte offsets into a source. The frontend refuses a source
    /// past `u32::MAX` bytes before lexing it (`source_too_long`), so every
    /// offset it takes fits; one that does not is a bug of the caller.
    pub fn from_offsets(start: usize, end: usize) -> Span {
        let narrow = |offset: usize| {
            u32::try_from(offset).expect(
                "internal: a source past u32::MAX bytes is refused before it is lexed (source_too_long)",
            )
        };
        Span::new(narrow(start), narrow(end))
    }

    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    pub fn text(self, source: &str) -> &str {
        &source[self.start as usize..self.end as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_up_to_u32_max_make_a_span() {
        assert_eq!(
            Span::from_offsets(0, u32::MAX as usize),
            Span::new(0, u32::MAX)
        );
    }

    #[test]
    #[should_panic(expected = "source_too_long")]
    fn an_offset_past_u32_max_panics() {
        Span::from_offsets(0, u32::MAX as usize + 1);
    }

    #[test]
    fn merge_covers_both() {
        let first = Span::new(2, 5);
        let second = Span::new(10, 12);
        assert_eq!(first.merge(second), Span::new(2, 12));
    }

    #[test]
    fn text_slices_source() {
        let source = "hello world";
        assert_eq!(Span::new(6, 11).text(source), "world");
    }

    #[test]
    fn contains_is_half_open() {
        let span = Span::new(3, 6);
        assert!(span.contains(3));
        assert!(span.contains(5));
        assert!(!span.contains(6));
    }
}
