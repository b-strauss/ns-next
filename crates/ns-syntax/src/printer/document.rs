//! A Wadler/Prettier document and its layout.

use std::collections::HashSet;

/// What the printer lowers a module to before it decides where lines break.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Document {
    /// Text without a line break.
    Text(String),
    /// Text that may hold line breaks (a multi-line string literal), written
    /// byte for byte; never trimmed.
    Verbatim(String),
    /// A space when its group is flat, a line break when it is broken.
    Line,
    /// Nothing when flat, a line break when broken.
    SoftLine,
    /// Always a line break; breaks every group around it.
    HardLine,
    /// A line break to column 0, whatever the indentation (a block comment's
    /// continuation line, which carries its own indentation).
    LiteralLine,
    /// `broken` when the enclosing group is broken, else `flat`.
    IfBreak {
        broken: String,
        flat: String,
    },
    /// Text written at the end of the current line, before its line break,
    /// that never counts toward the width (a trailing comment).
    LineSuffix(String),
    /// Breaks every group around it and writes nothing.
    BreakParent,
    /// Line breaks inside start `columns` further in.
    Indent(u32, Box<Document>),
    /// Flat if it fits on the line and holds nothing that forces a break,
    /// else broken, as a whole.
    Group(Box<Document>),
    /// The first alternative whose first line fits, else the last one.
    Choice(Vec<Document>),
    Concat(Vec<Document>),
}

pub(crate) fn text(content: impl Into<String>) -> Document {
    Document::Text(content.into())
}

pub(crate) fn concat(parts: Vec<Document>) -> Document {
    Document::Concat(parts)
}

pub(crate) fn group(content: Document) -> Document {
    Document::Group(Box::new(content))
}

pub(crate) fn indent(columns: u32, content: Document) -> Document {
    Document::Indent(columns, Box::new(content))
}

pub(crate) fn line() -> Document {
    Document::Line
}

pub(crate) fn soft_line() -> Document {
    Document::SoftLine
}

pub(crate) fn hard_line() -> Document {
    Document::HardLine
}

/// A document is as deep as the source it is lowered from (a chain of
/// messages nests a level per link), so the derived drop, which recurses
/// per level, could overflow the stack; its children are dropped from a
/// worklist instead.
impl Drop for Document {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_children(self, &mut pending);
        while let Some(mut child) = pending.pop() {
            take_children(&mut child, &mut pending);
        }
    }
}

/// Moves the children of `document` into `pending`, leaving it childless.
fn take_children(document: &mut Document, pending: &mut Vec<Document>) {
    match document {
        Document::Indent(_, content) | Document::Group(content) => {
            pending.push(std::mem::replace(
                &mut **content,
                Document::Concat(Vec::new()),
            ));
        }
        Document::Choice(parts) | Document::Concat(parts) => pending.append(parts),
        Document::Text(_)
        | Document::Verbatim(_)
        | Document::Line
        | Document::SoftLine
        | Document::HardLine
        | Document::LiteralLine
        | Document::IfBreak { .. }
        | Document::LineSuffix(_)
        | Document::BreakParent => {}
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Flat,
    Break,
}

/// A part of the document still to be written, with the indentation and the
/// mode of the group it stands in.
#[derive(Clone, Copy)]
struct Command<'document> {
    indentation: u32,
    mode: Mode,
    document: &'document Document,
}

/// The groups that must break whatever the width, by address: those holding
/// a `HardLine`, a `LiteralLine`, a `BreakParent` or a `Verbatim` with a
/// line break.
type BrokenGroups = HashSet<*const Document>;

/// Lays `document` out within `width` columns, each line break written as
/// `line_ending`, trailing spaces removed from every line.
pub(crate) fn layout(document: &Document, width: u32, line_ending: &str) -> String {
    let broken_groups = broken_groups(document);
    let width = usize::try_from(width).unwrap_or(usize::MAX);
    let mut writer = Writer {
        output: String::new(),
        column: 0,
        protected_end: 0,
        line_suffixes: Vec::new(),
        line_ending,
    };
    let mut commands = vec![Command {
        indentation: 0,
        mode: Mode::Break,
        document,
    }];
    while let Some(command) = commands.pop() {
        let Command {
            indentation,
            mode,
            document,
        } = command;
        match document {
            Document::Text(content) => writer.write(content),
            Document::Verbatim(content) => writer.write_verbatim(content),
            Document::Line => match mode {
                Mode::Flat => writer.write(" "),
                Mode::Break => writer.break_line(indentation),
            },
            Document::SoftLine => {
                if mode == Mode::Break {
                    writer.break_line(indentation);
                }
            }
            Document::HardLine => writer.break_line(indentation),
            Document::LiteralLine => writer.break_line(0),
            Document::IfBreak { broken, flat } => writer.write(match mode {
                Mode::Flat => flat,
                Mode::Break => broken,
            }),
            Document::LineSuffix(content) => writer.line_suffixes.push(content),
            Document::BreakParent => {}
            Document::Indent(columns, content) => commands.push(Command {
                indentation: indentation.saturating_add(*columns),
                mode,
                document: content,
            }),
            Document::Group(content) => {
                let flat = Command {
                    indentation,
                    mode: Mode::Flat,
                    document: content,
                };
                let group_mode = if broken_groups.contains(&std::ptr::from_ref(document)) {
                    Mode::Break
                } else if mode == Mode::Flat
                    || fits(
                        flat,
                        &commands,
                        width.saturating_sub(writer.column),
                        &broken_groups,
                    )
                {
                    Mode::Flat
                } else {
                    Mode::Break
                };
                commands.push(Command {
                    mode: group_mode,
                    ..flat
                });
            }
            Document::Choice(alternatives) => {
                let Some((last, others)) = alternatives.split_last() else {
                    continue;
                };
                let chosen = match mode {
                    Mode::Flat => Command {
                        indentation,
                        mode: Mode::Flat,
                        document: alternatives.first().unwrap_or(last),
                    },
                    Mode::Break => others
                        .iter()
                        .map(|alternative| Command {
                            indentation,
                            mode: Mode::Flat,
                            document: alternative,
                        })
                        .find(|candidate| {
                            fits(
                                *candidate,
                                &commands,
                                width.saturating_sub(writer.column),
                                &broken_groups,
                            )
                        })
                        .unwrap_or(Command {
                            indentation,
                            mode: Mode::Break,
                            document: last,
                        }),
                };
                commands.push(chosen);
            }
            Document::Concat(parts) => {
                commands.extend(parts.iter().rev().map(|part| Command {
                    indentation,
                    mode,
                    document: part,
                }));
            }
        }
    }
    writer.flush_line_suffixes();
    writer.trim_trailing_spaces();
    writer.output
}

/// The output so far and what the next line break needs to know.
struct Writer<'text> {
    output: String,
    /// In `char`s, the unit the width is counted in.
    column: usize,
    /// Where the last `Verbatim` ended: trimming stops there, since the
    /// spaces before it are a string literal's own.
    protected_end: usize,
    line_suffixes: Vec<&'text str>,
    line_ending: &'text str,
}

impl Writer<'_> {
    fn write(&mut self, content: &str) {
        self.output.push_str(content);
        self.column += content.chars().count();
    }

    fn write_verbatim(&mut self, content: &str) {
        self.output.push_str(content);
        self.protected_end = self.output.len();
        self.column = match content.rfind('\n') {
            Some(last_break) => content[last_break + 1..].chars().count(),
            None => self.column + content.chars().count(),
        };
    }

    fn break_line(&mut self, indentation: u32) {
        self.flush_line_suffixes();
        self.trim_trailing_spaces();
        self.output.push_str(self.line_ending);
        let indentation = usize::try_from(indentation).unwrap_or(usize::MAX);
        self.output.extend(std::iter::repeat_n(' ', indentation));
        self.column = indentation;
    }

    fn flush_line_suffixes(&mut self) {
        for suffix in self.line_suffixes.drain(..) {
            self.output.push_str(suffix);
        }
    }

    fn trim_trailing_spaces(&mut self) {
        let kept = self.output[self.protected_end..]
            .trim_end_matches([' ', '\t'])
            .len();
        self.output.truncate(self.protected_end + kept);
    }
}

/// Whether `next`, followed by the `rest` of the stack in its own modes,
/// fits in `remaining` columns up to the first line break.
fn fits(
    next: Command<'_>,
    rest: &[Command<'_>],
    remaining: usize,
    broken_groups: &BrokenGroups,
) -> bool {
    let mut remaining = remaining;
    let mut pending = vec![(next.mode, next.document)];
    let mut rest_index = rest.len();
    loop {
        let (mode, document) = match pending.pop() {
            Some(part) => part,
            None => {
                let Some(below) = rest_index.checked_sub(1) else {
                    return true;
                };
                rest_index = below;
                (rest[below].mode, rest[below].document)
            }
        };
        let measured = match document {
            Document::Text(content) => content,
            Document::Verbatim(content) => match content.split_once('\n') {
                Some((first_line, _)) => {
                    return first_line.trim_end_matches('\r').chars().count() <= remaining;
                }
                None => content,
            },
            Document::Line => match mode {
                Mode::Flat => " ",
                Mode::Break => return true,
            },
            Document::SoftLine => match mode {
                Mode::Flat => "",
                Mode::Break => return true,
            },
            Document::HardLine | Document::LiteralLine => return true,
            Document::IfBreak { broken, flat } => match mode {
                Mode::Flat => flat,
                Mode::Break => broken,
            },
            Document::LineSuffix(_) | Document::BreakParent => "",
            Document::Indent(_, content) => {
                pending.push((mode, content));
                ""
            }
            Document::Group(content) => {
                let group_mode = if broken_groups.contains(&std::ptr::from_ref(document)) {
                    Mode::Break
                } else {
                    mode
                };
                pending.push((group_mode, content));
                ""
            }
            // What the layout would write: the first alternative when flat;
            // when broken, the last, the one every other falls back to.
            Document::Choice(alternatives) => {
                let alternative = match mode {
                    Mode::Flat => alternatives.first(),
                    Mode::Break => alternatives.last(),
                };
                pending.extend(alternative.map(|alternative| (mode, alternative)));
                ""
            }
            Document::Concat(parts) => {
                pending.extend(parts.iter().rev().map(|part| (mode, part)));
                ""
            }
        };
        let width = measured.chars().count();
        if width > remaining {
            return false;
        }
        remaining -= width;
    }
}

/// Marks every group that holds something forcing a line break, and every
/// group around it, up to the nearest `Choice`: an alternative's line break
/// is the alternative's, and choosing among alternatives is how the
/// enclosing line avoids breaking.
fn broken_groups(document: &Document) -> BrokenGroups {
    enum Visit<'document> {
        Enter(&'document Document),
        /// Leaves the innermost group or `Choice` of `enclosing`.
        Leave,
    }
    let mut broken = BrokenGroups::new();
    // The groups around the visited part, innermost last; `None` for a
    // `Choice`, where breaking stops.
    let mut enclosing: Vec<Option<*const Document>> = Vec::new();
    let mut visits = vec![Visit::Enter(document)];
    while let Some(visit) = visits.pop() {
        let document = match visit {
            Visit::Enter(document) => document,
            Visit::Leave => {
                enclosing.pop();
                continue;
            }
        };
        let forces_break = match document {
            Document::HardLine | Document::LiteralLine | Document::BreakParent => true,
            Document::Verbatim(content) => content.contains('\n'),
            Document::Indent(_, content) => {
                visits.push(Visit::Enter(content));
                false
            }
            Document::Group(content) => {
                enclosing.push(Some(std::ptr::from_ref(document)));
                visits.push(Visit::Leave);
                visits.push(Visit::Enter(content));
                false
            }
            Document::Choice(alternatives) => {
                enclosing.push(None);
                visits.push(Visit::Leave);
                visits.extend(alternatives.iter().map(Visit::Enter));
                false
            }
            Document::Concat(parts) => {
                visits.extend(parts.iter().map(Visit::Enter));
                false
            }
            Document::Text(_)
            | Document::Line
            | Document::SoftLine
            | Document::IfBreak { .. }
            | Document::LineSuffix(_) => false,
        };
        if forces_break {
            // A group already marked had every group around it marked then.
            for group in enclosing.iter().rev() {
                match group {
                    Some(group) if broken.insert(*group) => {}
                    _ => break,
                }
            }
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(parts: &[&str]) -> Document {
        let mut items = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                items.push(Document::Line);
            }
            items.push(text(*part));
        }
        group(concat(items))
    }

    #[test]
    fn a_group_that_fits_stays_flat() {
        assert_eq!(layout(&words(&["a", "b", "c"]), 10, "\n"), "a b c");
    }

    #[test]
    fn a_group_that_does_not_fit_breaks_as_a_whole() {
        assert_eq!(
            layout(&words(&["aaaa", "bbbb", "cccc"]), 10, "\n"),
            "aaaa\nbbbb\ncccc"
        );
    }

    #[test]
    fn the_outer_group_breaks_before_the_inner_one() {
        let document = group(concat(vec![
            text("receiver"),
            indent(
                4,
                concat(vec![
                    Document::Line,
                    words(&["at:", "1"]),
                    Document::Line,
                    words(&["put:", "2"]),
                ]),
            ),
        ]));
        assert_eq!(
            layout(&document, 14, "\n"),
            "receiver\n    at: 1\n    put: 2"
        );
    }

    #[test]
    fn a_hard_line_breaks_every_group_around_it() {
        let document = group(concat(vec![
            text("a"),
            Document::Line,
            text("b"),
            Document::HardLine,
            text("c"),
        ]));
        assert_eq!(layout(&document, 100, "\n"), "a\nb\nc");
    }

    #[test]
    fn if_break_follows_its_group() {
        let flat = group(concat(vec![
            text("{a"),
            Document::IfBreak {
                broken: ".".into(),
                flat: String::new(),
            },
            text("}"),
        ]));
        assert_eq!(layout(&flat, 100, "\n"), "{a}");
        let broken = group(concat(vec![
            text("{a"),
            Document::IfBreak {
                broken: ".".into(),
                flat: String::new(),
            },
            Document::HardLine,
            text("}"),
        ]));
        assert_eq!(layout(&broken, 100, "\n"), "{a.\n}");
    }

    #[test]
    fn a_line_suffix_does_not_count_and_lands_before_the_break() {
        let document = concat(vec![
            group(concat(vec![
                text("aaaaaaaa"),
                Document::LineSuffix(" /// note".into()),
                Document::Line,
                text("b"),
            ])),
            Document::HardLine,
            text("c"),
        ]);
        // the suffix is not measured: "aaaaaaaa b" fits in 10
        assert_eq!(layout(&document, 10, "\n"), "aaaaaaaa b /// note\nc");
    }

    #[test]
    fn break_parent_breaks_its_group() {
        let document = group(concat(vec![
            text("a"),
            Document::LineSuffix(" /// x".into()),
            Document::BreakParent,
            Document::Line,
            text("b"),
        ]));
        assert_eq!(layout(&document, 100, "\n"), "a /// x\nb");
    }

    #[test]
    fn choice_takes_the_first_alternative_whose_first_line_fits() {
        let long = group(concat(vec![
            text("x"),
            Document::Line,
            text("yyyyyyyyyyyy"),
        ]));
        let hugged = concat(vec![
            text("x ["),
            indent(2, concat(vec![Document::HardLine, text("body")])),
            Document::HardLine,
            text("]"),
        ]);
        let document = Document::Choice(vec![long, hugged, text("fallback")]);
        assert_eq!(layout(&document, 8, "\n"), "x [\n  body\n]");
    }

    #[test]
    fn indentation_is_two_spaces_per_level_and_blank_lines_carry_none() {
        let document = concat(vec![
            text("("),
            indent(
                2,
                concat(vec![
                    Document::HardLine,
                    text("a"),
                    Document::HardLine,
                    Document::HardLine,
                    text("b"),
                ]),
            ),
            Document::HardLine,
            text(")"),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "(\n  a\n\n  b\n)");
    }

    #[test]
    fn line_endings_are_the_callers_and_verbatim_text_is_untouched() {
        let document = concat(vec![
            text("a"),
            Document::HardLine,
            Document::Verbatim("'x  \ny'".into()),
            Document::HardLine,
            text("b"),
        ]);
        assert_eq!(layout(&document, 100, "\r\n"), "a\r\n'x  \ny'\r\nb");
    }

    #[test]
    fn a_literal_line_returns_to_column_zero() {
        let document = indent(
            4,
            concat(vec![text("(* a"), Document::LiteralLine, text("   b *)")]),
        );
        assert_eq!(layout(&document, 100, "\n"), "(* a\n   b *)");
    }

    /// A literal line is a line break whatever the width, so its group's
    /// lines break too, as around a `HardLine`.
    #[test]
    fn a_literal_line_breaks_every_group_around_it() {
        let document = group(concat(vec![
            text("a"),
            line(),
            text("(* b"),
            Document::LiteralLine,
            text("c *)"),
        ]));
        assert_eq!(layout(&document, 100, "\n"), "a\n(* b\nc *)");
    }

    #[test]
    fn width_counts_chars_not_bytes() {
        assert_eq!(layout(&words(&["äöü", "äöü"]), 7, "\n"), "äöü äöü");
    }

    #[test]
    fn a_soft_line_is_nothing_when_flat_and_a_break_when_broken() {
        let document = |content: &str| {
            group(concat(vec![
                text("("),
                indent(2, concat(vec![soft_line(), text(content)])),
                soft_line(),
                text(")"),
            ]))
        };
        assert_eq!(layout(&document("abc"), 10, "\n"), "(abc)");
        assert_eq!(
            layout(&document("abcdefghij"), 10, "\n"),
            "(\n  abcdefghij\n)"
        );
    }

    /// The column after a multi-line `Verbatim` is its last line's length,
    /// and spaces written after it are trimmed again.
    #[test]
    fn a_verbatim_line_break_restarts_the_column() {
        let document = concat(vec![
            Document::Verbatim("'aaaaaaaaaaaa\nb'".into()),
            group(concat(vec![text(" c"), line(), text("d"), text(" ")])),
            hard_line(),
            text("e"),
        ]);
        assert_eq!(layout(&document, 8, "\n"), "'aaaaaaaaaaaa\nb' c d\ne");
    }

    /// A `HardLine` inside an alternative breaks the groups inside that
    /// alternative only: the group around the `Choice` stays flat.
    #[test]
    fn breaking_does_not_propagate_out_of_a_choice() {
        let alternative = concat(vec![
            group(concat(vec![text("a"), line(), text("b"), hard_line()])),
            text("c"),
        ]);
        let document = group(concat(vec![
            text("x"),
            line(),
            Document::Choice(vec![alternative]),
            line(),
            text("y"),
        ]));
        assert_eq!(layout(&document, 100, "\n"), "x a\nb\nc y");
    }

    /// A `Choice` alternative laid out flat still breaks its marked groups,
    /// and an alternative whose marked group's first line does not fit is
    /// passed over.
    #[test]
    fn a_choice_measures_a_marked_group_to_its_first_line_break() {
        let alternative = |head: &str| {
            group(concat(vec![
                text(head),
                Document::BreakParent,
                line(),
                text("tail"),
            ]))
        };
        let document = Document::Choice(vec![
            alternative("too long a head"),
            alternative("head"),
            text("last"),
        ]);
        assert_eq!(layout(&document, 8, "\n"), "head\ntail");
    }

    #[test]
    fn when_no_alternative_fits_the_last_one_is_laid_out_broken() {
        let document = Document::Choice(vec![
            text("much too long"),
            group(concat(vec![text("also"), line(), text("long")])),
        ]);
        assert_eq!(layout(&document, 5, "\n"), "also\nlong");
    }

    #[test]
    fn line_suffixes_are_flushed_at_the_end_and_trimmed() {
        let document = concat(vec![text("a"), Document::LineSuffix(" /// end  ".into())]);
        assert_eq!(layout(&document, 100, "\n"), "a /// end");
    }

    /// A `Verbatim` with a line break forces its group's broken form, and
    /// the measurement of what follows stops at its first line break.
    #[test]
    fn a_multi_line_verbatim_breaks_its_group() {
        let document = group(concat(vec![
            text("a"),
            line(),
            Document::Verbatim("'b\nc'".into()),
        ]));
        assert_eq!(layout(&document, 100, "\n"), "a\n'b\nc'");
    }

    /// Layout, the breaking pass and dropping all walk the document with an
    /// explicit stack: 100,000 levels of nesting fit a 256 KiB thread stack.
    #[test]
    fn a_deeply_nested_document_lays_out_and_drops_on_a_small_stack() {
        const LEVELS: usize = 100_000;
        const STACK_BYTES: usize = 256 * 1024;
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(|| {
                // Every group holds the innermost hard line, so every one is
                // marked broken.
                let mut broken = concat(vec![text("end"), hard_line(), text("done")]);
                for _ in 0..LEVELS {
                    broken = group(concat(vec![text("x"), line(), indent(0, broken)]));
                }
                let mut expected = "x\n".repeat(LEVELS);
                expected.push_str("end\ndone");
                assert_eq!(layout(&broken, 100, "\n"), expected);
                drop(broken);

                // Nothing here takes width, so the outermost group's
                // measurement walks every level before it answers "fits".
                let mut flat = text("deep");
                for _ in 0..LEVELS {
                    flat = group(indent(2, concat(vec![soft_line(), flat])));
                }
                assert_eq!(layout(&flat, 100, "\n"), "deep");
                drop(flat);
            })
            .expect("spawn the layout thread")
            .join()
            .expect("the document lays out and drops on a small stack");
    }
}
