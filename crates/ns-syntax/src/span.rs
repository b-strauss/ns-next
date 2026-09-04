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

    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    pub fn text(self, src: &str) -> &str {
        &src[self.start as usize..self.end as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_covers_both() {
        let a = Span::new(2, 5);
        let b = Span::new(10, 12);
        assert_eq!(a.merge(b), Span::new(2, 12));
    }

    #[test]
    fn text_slices_source() {
        let src = "hello world";
        assert_eq!(Span::new(6, 11).text(src), "world");
    }

    #[test]
    fn contains_is_half_open() {
        let s = Span::new(3, 6);
        assert!(s.contains(3));
        assert!(s.contains(5));
        assert!(!s.contains(6));
    }
}
