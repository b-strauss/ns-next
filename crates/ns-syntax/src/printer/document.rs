//! A Wadler/Prettier document and its layout.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// What the printer lowers a module to before it decides where lines break.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Document {
    /// Text without a line break.
    Text(String),
    /// Text that may hold line breaks (a multi-line string literal), written
    /// byte for byte; never trimmed. A `\r` is a line break whether or not a
    /// `\n` follows it.
    Verbatim(String),
    /// Lines written one below the other (a block comment's): the first
    /// where the layout stands, each further one shifted right by as many
    /// columns as the first stands right of `column`, or left by as many as
    /// it stands left of it, never further left than column 0.
    Shifted {
        lines: Vec<String>,
        column: u32,
    },
    /// Writes nothing, but the next text starts a line of its own: a line
    /// break before it unless its line holds nothing yet, which is then
    /// indented as that text is (a comment that must stand alone on its
    /// line, or that ends its line).
    FreshLine,
    /// Text after something on its line (a comment trailing a token):
    /// counted toward the width only when more text follows it on the
    /// line, since at the end of a line it stands where nothing else would.
    Trailing(String),
    /// Marks where a comment leading the text after it starts.
    LeadingStart,
    /// Ends a comment leading the text after it: a space, or a `FreshLine`
    /// when the comment started its line, where it stays alone.
    LeadingEnd,
    /// A space when its group is flat, a line break when it is broken.
    Line,
    /// Nothing when flat, a line break when broken.
    SoftLine,
    /// Always a line break; breaks every group around it.
    HardLine,
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
    /// Writes nothing; stands right after a block's `]`. When its group is
    /// broken, the `]` ends a block opened at a line end, and a message sent
    /// to what ends there may follow it on its line (`Follow`).
    BlockEnd,
    /// A receiver, then the messages sent to it, `rest`, a `Group`. When the
    /// receiver ends on a broken block's `]` or a multi-line string (only `)`
    /// after it), or did not break, the group is decided where the receiver ends; when the
    /// receiver broke otherwise, the outer level breaks first: the group is
    /// broken.
    Follow {
        receiver: Box<Document>,
        rest: Box<Document>,
    },
    /// Line breaks inside start `columns` further in.
    Indent(u32, Box<Document>),
    /// Flat if it fits on the line and holds nothing that forces a break,
    /// else broken, as a whole. Inside a flat group it is flat too, unless
    /// it holds something that forces a break, or a line break has been
    /// written since a group or `Choice` was last decided: then it is
    /// measured again from where it starts.
    Group(Box<Document>),
    /// The first alternative whose first line fits, laid out flat (its
    /// groups that hold a forced break still break), else the last one,
    /// broken. Inside a flat group it is the first alternative, unless a
    /// line break has been written since a group or `Choice` was last
    /// decided: then its alternatives are measured again.
    Choice(Vec<Document>),
    Concat(Vec<Document>),
    /// A document that stands in several alternatives of `Choice`s, held
    /// once: written and measured as its content. Each alternative of a
    /// message with block arguments holds the same arguments, so without
    /// sharing, nested messages would multiply them exponentially.
    Shared(Rc<Document>),
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
        Document::Follow { receiver, rest } => {
            pending.push(std::mem::replace(
                &mut **receiver,
                Document::Concat(Vec::new()),
            ));
            pending.push(std::mem::replace(&mut **rest, Document::Concat(Vec::new())));
        }
        // Only the last holder takes the content apart, leaving an empty
        // `Concat` behind; the others drop their count.
        Document::Shared(content) => {
            if let Some(content) = Rc::get_mut(content) {
                pending.push(std::mem::replace(content, Document::Concat(Vec::new())));
            }
        }
        Document::Text(_)
        | Document::Verbatim(_)
        | Document::Shifted { .. }
        | Document::FreshLine
        | Document::Trailing(_)
        | Document::LeadingStart
        | Document::LeadingEnd
        | Document::Line
        | Document::SoftLine
        | Document::HardLine
        | Document::IfBreak { .. }
        | Document::LineSuffix(_)
        | Document::BreakParent
        | Document::BlockEnd => {}
    }
}

/// Where the last line break in `content` ends, `\r\n`, `\r` and `\n` each
/// one break.
fn after_last_line_break(content: &str) -> Option<usize> {
    content.rfind(['\n', '\r']).map(|index| index + 1)
}

/// `content`'s first line, up to its first line break of any kind.
fn first_line(content: &str) -> Option<&str> {
    content
        .find(['\n', '\r'])
        .map(|line_break| &content[..line_break])
}

/// `line` moved `shift` columns: spaces put in front, or up to that many
/// leading whitespace characters taken away. An empty line stays empty.
/// A continuation line's own leading whitespace, tabs included, is the
/// comment's text, never expanded: the rule against tabs governs only
/// the indentation the printer writes itself, and a tab counts as one
/// column here as in `AttachedComment::column`.
fn shifted(line: &str, shift: i64) -> String {
    if line.is_empty() {
        return String::new();
    }
    if shift >= 0 {
        let added = usize::try_from(shift).unwrap_or(usize::MAX);
        let mut moved = " ".repeat(added);
        moved.push_str(line);
        return moved;
    }
    let mut removable = usize::try_from(shift.unsigned_abs()).unwrap_or(usize::MAX);
    let mut rest = line;
    while removable > 0 {
        let mut characters = rest.chars();
        match characters.next() {
            Some(character) if character.is_whitespace() => {
                rest = characters.as_str();
                removable -= 1;
            }
            _ => break,
        }
    }
    rest.to_string()
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
    /// Set on the command that decides a `Follow`'s `rest` once its receiver
    /// is written: the count of line breaks written before the receiver.
    follows: Option<usize>,
}

/// The groups that must break whatever the width, by address: those holding
/// a `HardLine`, a `BreakParent`, a `FreshLine`, or a
/// `Verbatim` or `Shifted` with a line break.
type BrokenGroups = HashSet<*const Document>;

/// Lays `document` out within `width` columns, each line break written as
/// `line_ending`, trailing spaces removed from every line (never a
/// `Verbatim`'s own). A group is measured only up to its first line break;
/// after a line break inside flat content, the next `Group` or `Choice` is
/// measured again rather than taken flat, so what follows a hugged block's
/// `]` is held to the width when it is grouped. A `Line` of a group already
/// taken flat stays a space.
pub(crate) fn layout(document: &Document, width: u32, line_ending: &str) -> String {
    let broken_groups = broken_groups(document);
    let width = usize::try_from(width).unwrap_or(usize::MAX);
    let mut writer = Writer {
        output: String::new(),
        column: 0,
        protected_end: 0,
        line_start: 0,
        line_has_content: false,
        fresh_line: false,
        leading_starts_line: false,
        remeasure: false,
        line_breaks: 0,
        block_end: None,
        line_suffixes: Vec::new(),
        line_ending,
    };
    let mut commands = vec![Command {
        indentation: 0,
        mode: Mode::Break,
        document,
        follows: None,
    }];
    while let Some(command) = commands.pop() {
        let Command {
            indentation,
            mode,
            document,
            follows,
        } = command;
        if let (Some(breaks_before), Document::Follow { rest, .. }) = (follows, document) {
            let broke = writer.line_breaks != breaks_before;
            let decided = if broke && !writer.ends_after_block() {
                // The receiver broke by the rules of its own messages: the
                // messages sent to it are the outer level, broken first.
                let content = match &**rest {
                    Document::Group(content) => &**content,
                    other => other,
                };
                Command {
                    indentation,
                    mode: Mode::Break,
                    document: content,
                    follows: None,
                }
            } else {
                Command {
                    indentation,
                    mode,
                    document: rest,
                    follows: None,
                }
            };
            commands.push(decided);
            continue;
        }
        match document {
            Document::Text(content) => writer.write(content, indentation),
            Document::Verbatim(content) => writer.write_verbatim(content, indentation),
            Document::Shifted { lines, column } => {
                writer.write_shifted(lines, *column, indentation)
            }
            Document::FreshLine => writer.fresh_line = true,
            Document::Trailing(content) => writer.write(content, indentation),
            Document::LeadingStart => {
                writer.leading_starts_line = writer.fresh_line || !writer.line_has_content;
            }
            Document::LeadingEnd => {
                if writer.leading_starts_line {
                    writer.fresh_line = true;
                } else {
                    writer.write(" ", indentation);
                }
            }
            Document::Line => match mode {
                Mode::Flat => writer.write(" ", indentation),
                Mode::Break => writer.break_line(indentation),
            },
            Document::SoftLine => {
                if mode == Mode::Break {
                    writer.break_line(indentation);
                }
            }
            Document::HardLine => writer.break_line(indentation),
            Document::IfBreak { broken, flat } => writer.write(
                match mode {
                    Mode::Flat => flat,
                    Mode::Break => broken,
                },
                indentation,
            ),
            Document::LineSuffix(content) => writer.line_suffixes.push(content),
            Document::BreakParent => {}
            Document::BlockEnd => {
                if mode == Mode::Break {
                    writer.block_end = Some(writer.output.len());
                }
            }
            Document::Follow { receiver, .. } => {
                commands.push(Command {
                    indentation,
                    mode,
                    document,
                    follows: Some(writer.line_breaks),
                });
                commands.push(Command {
                    indentation,
                    mode,
                    document: receiver,
                    follows: None,
                });
            }
            Document::Indent(columns, content) => commands.push(Command {
                indentation: indentation.saturating_add(*columns),
                mode,
                document: content,
                follows: None,
            }),
            Document::Shared(content) => commands.push(Command {
                indentation,
                mode,
                document: content,
                follows: None,
            }),
            Document::Group(content) => {
                let flat = Command {
                    indentation,
                    mode: Mode::Flat,
                    document: content,
                    follows: None,
                };
                let marked = broken_groups.contains(&std::ptr::from_ref(document));
                let group_mode = if mode == Mode::Flat && !writer.remeasure {
                    if marked {
                        Mode::Break
                    } else {
                        Mode::Flat
                    }
                } else {
                    writer.remeasure = false;
                    if !marked && fits(flat, &commands, writer.start(width), &broken_groups) {
                        Mode::Flat
                    } else {
                        Mode::Break
                    }
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
                let chosen = if mode == Mode::Flat && !writer.remeasure {
                    Command {
                        indentation,
                        mode: Mode::Flat,
                        document: alternatives.first().unwrap_or(last),
                        follows: None,
                    }
                } else {
                    writer.remeasure = false;
                    others
                        .iter()
                        .map(|alternative| Command {
                            indentation,
                            mode: Mode::Flat,
                            document: alternative,
                            follows: None,
                        })
                        .find(|candidate| {
                            fits(*candidate, &commands, writer.start(width), &broken_groups)
                        })
                        .unwrap_or(Command {
                            indentation,
                            mode: Mode::Break,
                            document: last,
                            follows: None,
                        })
                };
                commands.push(chosen);
            }
            Document::Concat(parts) => {
                commands.extend(parts.iter().rev().map(|part| Command {
                    indentation,
                    mode,
                    document: part,
                    follows: None,
                }));
            }
        }
    }
    writer.trim_trailing_spaces();
    writer.flush_line_suffixes();
    writer.trim_trailing_spaces();
    writer.output
}

/// The output so far and what the next line break needs to know.
struct Writer<'text> {
    output: String,
    /// In `char`s, the unit the width is counted in.
    column: usize,
    /// Where the last non-empty `Verbatim` ended: trimming stops there, since
    /// spaces at its end are a string literal's own. An empty one holds
    /// nothing to protect and leaves the spaces before it trimmable.
    protected_end: usize,
    /// Where the current line starts in `output`, after its indentation's
    /// line break.
    line_start: usize,
    /// Whether anything but indentation stands on the current line.
    line_has_content: bool,
    /// A `FreshLine` waits for the next text.
    fresh_line: bool,
    /// The leading comment being written started its line.
    leading_starts_line: bool,
    /// Set by every line break written. A group or `Choice` was taken flat
    /// on a measurement that ended at the first line break; once one is
    /// written, what follows on the new line was never measured, so the next
    /// group or `Choice` met in flat content is decided as in broken content.
    remeasure: bool,
    /// How many line breaks have been written.
    line_breaks: usize,
    /// Where the `]` of the last broken block, or the last multi-line string,
    /// ended in `output`.
    block_end: Option<usize>,
    line_suffixes: Vec<&'text str>,
    line_ending: &'text str,
}

impl Writer<'_> {
    /// Whether the output ends on a broken block's `]` or a multi-line
    /// string, closing parentheses aside.
    fn ends_after_block(&self) -> bool {
        self.block_end
            .and_then(|end| self.output.get(end..))
            .is_some_and(|after| after.chars().all(|character| character == ')'))
    }

    /// Where a measurement made now starts.
    fn start(&self, width: usize) -> Start {
        Start {
            width,
            column: self.column,
            fresh_line: self.fresh_line,
        }
    }

    /// `content` as it is written: a waiting `FreshLine` drops it while it
    /// is blank, and starts its line before it otherwise, without the
    /// spaces that would have led it on the old line.
    fn begin<'content>(
        &mut self,
        content: &'content str,
        indentation: u32,
    ) -> Option<&'content str> {
        if !self.fresh_line {
            return Some(content);
        }
        let content = content.trim_start_matches(' ');
        if content.is_empty() {
            return None;
        }
        self.fresh_line = false;
        if self.line_has_content {
            self.break_line(indentation);
        } else {
            // The line holds only indentation: it is indented anew rather
            // than followed by a blank line.
            self.output
                .truncate(self.line_start.max(self.protected_end));
            let indentation = usize::try_from(indentation).unwrap_or(usize::MAX);
            self.output.extend(std::iter::repeat_n(' ', indentation));
            self.column = indentation;
        }
        Some(content)
    }

    fn write(&mut self, content: &str, indentation: u32) {
        let Some(content) = self.begin(content, indentation) else {
            return;
        };
        self.output.push_str(content);
        self.column += content.chars().count();
        if !content.trim().is_empty() {
            self.line_has_content = true;
        }
    }

    fn write_verbatim(&mut self, content: &str, indentation: u32) {
        let Some(content) = self.begin(content, indentation) else {
            return;
        };
        self.output.push_str(content);
        if !content.is_empty() {
            self.protected_end = self.output.len();
            self.line_has_content = true;
        }
        self.column = match after_last_line_break(content) {
            Some(last_line) => {
                self.remeasure = true;
                self.line_breaks += 1;
                // A message may follow a multi-line string on its last line,
                // as it follows a broken block's `]`.
                self.block_end = Some(self.output.len());
                content[last_line..].chars().count()
            }
            None => self.column + content.chars().count(),
        };
    }

    fn write_shifted(&mut self, lines: &[String], column: u32, indentation: u32) {
        let Some((first, rest)) = lines.split_first() else {
            return;
        };
        let Some(first) = self.begin(first, indentation) else {
            return;
        };
        let shift = i64::try_from(self.column).unwrap_or(i64::MAX) - i64::from(column);
        self.write(first, indentation);
        for line in rest {
            self.trim_trailing_spaces();
            self.output.push_str(self.line_ending);
            self.line_start = self.output.len();
            let moved = shifted(line, shift);
            self.column = moved.chars().count();
            self.output.push_str(&moved);
            self.remeasure = true;
            self.line_breaks += 1;
        }
    }

    fn break_line(&mut self, indentation: u32) {
        self.remeasure = true;
        self.line_breaks += 1;
        self.fresh_line = false;
        // Each suffix brings its one space: the spaces before it are the
        // line's own, and go.
        self.trim_trailing_spaces();
        self.flush_line_suffixes();
        self.trim_trailing_spaces();
        self.output.push_str(self.line_ending);
        self.line_start = self.output.len();
        self.line_has_content = false;
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

/// Where a measurement starts: the width, the column the writer stands at,
/// and whether a `FreshLine` waits, which puts the first text measured at
/// the start of a line of its own.
#[derive(Clone, Copy)]
struct Start {
    width: usize,
    column: usize,
    fresh_line: bool,
}

/// Whether `next`, followed by the `rest` of the stack in its own modes,
/// fits in the line from `start` up to the first line break. Text in `next`
/// after a `FreshLine` on a line that already holds text does not fit: it
/// would stand on a line no layout asked for, after a comment that must end
/// its line.
fn fits(
    next: Command<'_>,
    rest: &[Command<'_>],
    start: Start,
    broken_groups: &BrokenGroups,
) -> bool {
    let mut remaining = start.width.saturating_sub(start.column);
    // With a `FreshLine` waiting, the first text is still to be placed at
    // the start of a line, at its own indentation.
    let mut placed = !start.fresh_line;
    let mut fresh_line = false;
    let mut pending = vec![(next.indentation, next.mode, next.document)];
    let mut rest_index = rest.len();
    let mut in_rest = false;
    // The width of trailing text not yet known to be followed on its line.
    let mut deferred = 0;
    loop {
        let (indentation, mode, document) = match pending.pop() {
            Some(part) => part,
            None => {
                // What is being decided ends its line with a comment: what
                // follows it is another line's.
                if fresh_line {
                    return true;
                }
                let Some(below) = rest_index.checked_sub(1) else {
                    return true;
                };
                rest_index = below;
                in_rest = true;
                let command = rest[below];
                // A `Follow` waiting for its receiver: what is left of it
                // is its messages.
                let document = match (command.follows, command.document) {
                    (Some(_), Document::Follow { rest, .. }) => &**rest,
                    (_, document) => document,
                };
                (command.indentation, command.mode, document)
            }
        };
        let measured = match document {
            Document::Text(content) | Document::Verbatim(content) => content.as_str(),
            Document::Shifted { lines, .. } => lines.first().map_or("", String::as_str),
            Document::Trailing(content) if placed && !fresh_line => {
                deferred += content.chars().count();
                continue;
            }
            Document::Trailing(content) => content.as_str(),
            Document::LeadingStart => "",
            Document::LeadingEnd => " ",
            Document::FreshLine => {
                // Past what is being decided, a comment that must end its
                // line ends the measured line like a line break.
                if in_rest && placed {
                    return true;
                }
                fresh_line = placed;
                ""
            }
            Document::Line => match mode {
                Mode::Flat => " ",
                Mode::Break => return true,
            },
            Document::SoftLine => match mode {
                Mode::Flat => "",
                Mode::Break => return true,
            },
            Document::HardLine => return true,
            Document::IfBreak { broken, flat } => match mode {
                Mode::Flat => flat,
                Mode::Break => broken,
            },
            Document::LineSuffix(_) | Document::BreakParent | Document::BlockEnd => "",
            Document::Follow { receiver, rest } => {
                pending.push((indentation, mode, &**rest));
                pending.push((indentation, mode, &**receiver));
                ""
            }
            Document::Indent(columns, content) => {
                pending.push((indentation.saturating_add(*columns), mode, content));
                ""
            }
            Document::Shared(content) => {
                pending.push((indentation, mode, &**content));
                ""
            }
            Document::Group(content) => {
                let group_mode = if broken_groups.contains(&std::ptr::from_ref(document)) {
                    Mode::Break
                } else {
                    mode
                };
                pending.push((indentation, group_mode, content));
                ""
            }
            // What the layout would write: the first alternative when flat;
            // when broken, the last, the one every other falls back to.
            Document::Choice(alternatives) => {
                let alternative = match mode {
                    Mode::Flat => alternatives.first(),
                    Mode::Break => alternatives.last(),
                };
                pending.extend(alternative.map(|alternative| (indentation, mode, alternative)));
                ""
            }
            Document::Concat(parts) => {
                pending.extend(parts.iter().rev().map(|part| (indentation, mode, part)));
                ""
            }
        };
        let mut measured = measured;
        if !placed || fresh_line {
            measured = measured.trim_start_matches(' ');
            if measured.is_empty() {
                continue;
            }
            if fresh_line {
                return false;
            }
            placed = true;
            let indentation = usize::try_from(indentation).unwrap_or(usize::MAX);
            remaining = start.width.saturating_sub(indentation);
        }
        let (line, ends) = match first_line(measured) {
            Some(line) => (line, true),
            None => (measured, false),
        };
        let mut width = line.chars().count();
        if !line.trim().is_empty() {
            width += deferred;
            deferred = 0;
        }
        if width > remaining {
            return false;
        }
        if ends {
            return true;
        }
        remaining -= width;
    }
}

/// Marks every group that holds something forcing a line break, and every
/// group around it, up to the nearest `Choice`: an alternative's line break
/// is the alternative's, and choosing among alternatives is how the
/// enclosing line avoids breaking. A `Shared` document is walked once; where
/// it stands again, whether a forced break leaves it is remembered rather
/// than found again, which keeps the pass linear in the distinct documents.
fn broken_groups(document: &Document) -> BrokenGroups {
    enum Visit<'document> {
        Enter(&'document Document),
        /// Leaves the innermost entry of `enclosing`.
        Leave,
    }
    /// What stands around the visited part.
    enum Enclosing {
        Group(*const Document),
        /// Where breaking stops.
        Choice,
        /// A shared document walked for the first time, and whether a
        /// forced break inside it has reached its edge.
        Shared {
            content: *const Document,
            breaks_out: bool,
        },
    }
    /// Marks the groups around a forced break, innermost first.
    fn mark(enclosing: &mut [Enclosing], broken: &mut BrokenGroups) {
        for around in enclosing.iter_mut().rev() {
            match around {
                // A group already marked had every group around it marked
                // then, on the same path: only a `Shared` content is reached
                // by two paths, and it is walked only once.
                Enclosing::Group(group) => {
                    if !broken.insert(*group) {
                        break;
                    }
                }
                Enclosing::Choice => break,
                Enclosing::Shared { breaks_out, .. } => *breaks_out = true,
            }
        }
    }
    let mut broken = BrokenGroups::new();
    let mut shared_breaks_out: HashMap<*const Document, bool> = HashMap::new();
    let mut enclosing: Vec<Enclosing> = Vec::new();
    let mut visits = vec![Visit::Enter(document)];
    while let Some(visit) = visits.pop() {
        let document = match visit {
            Visit::Enter(document) => document,
            Visit::Leave => {
                if let Some(Enclosing::Shared {
                    content,
                    breaks_out,
                }) = enclosing.pop()
                {
                    shared_breaks_out.insert(content, breaks_out);
                }
                continue;
            }
        };
        let forces_break = match document {
            Document::HardLine | Document::BreakParent | Document::FreshLine => true,
            Document::Verbatim(content) => content.contains(['\n', '\r']),
            Document::Shifted { lines, .. } => lines.len() > 1,
            Document::Indent(_, content) => {
                visits.push(Visit::Enter(content));
                false
            }
            Document::Group(content) => {
                enclosing.push(Enclosing::Group(std::ptr::from_ref(document)));
                visits.push(Visit::Leave);
                visits.push(Visit::Enter(content));
                false
            }
            Document::Choice(alternatives) => {
                enclosing.push(Enclosing::Choice);
                visits.push(Visit::Leave);
                visits.extend(alternatives.iter().map(Visit::Enter));
                false
            }
            Document::Shared(content) => {
                let content_address = Rc::as_ptr(content);
                match shared_breaks_out.get(&content_address) {
                    Some(breaks_out) => *breaks_out,
                    None => {
                        enclosing.push(Enclosing::Shared {
                            content: content_address,
                            breaks_out: false,
                        });
                        visits.push(Visit::Leave);
                        visits.push(Visit::Enter(content));
                        false
                    }
                }
            }
            Document::Concat(parts) => {
                visits.extend(parts.iter().map(Visit::Enter));
                false
            }
            Document::Follow { receiver, rest } => {
                visits.push(Visit::Enter(receiver));
                visits.push(Visit::Enter(rest));
                false
            }
            Document::Text(_)
            | Document::BlockEnd
            | Document::Line
            | Document::SoftLine
            | Document::IfBreak { .. }
            | Document::LineSuffix(_)
            | Document::Trailing(_)
            | Document::LeadingStart
            | Document::LeadingEnd => false,
        };
        if forces_break {
            mark(&mut enclosing, &mut broken);
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

    /// A block comment written further left than it stood moves its
    /// continuation lines left by as much, none past column 0; written
    /// further right, right by as much. An empty line stays empty.
    #[test]
    fn shifted_lines_move_with_the_first() {
        let comment = |column: u32| Document::Shifted {
            lines: vec![
                "(* one".to_string(),
                "     two".to_string(),
                String::new(),
                "  three *)".to_string(),
            ],
            column,
        };
        let left = indent(2, concat(vec![hard_line(), comment(4)]));
        assert_eq!(layout(&left, 100, "\n"), "\n  (* one\n   two\n\nthree *)");
        let right = indent(6, concat(vec![hard_line(), comment(2)]));
        assert_eq!(
            layout(&right, 100, "\n"),
            "\n      (* one\n         two\n\n      three *)"
        );
    }

    /// Lines written one below the other break every group around them.
    #[test]
    fn shifted_lines_break_every_group_around_them() {
        let document = group(concat(vec![
            text("a"),
            line(),
            Document::Shifted {
                lines: vec!["(* b".to_string(), "c *)".to_string()],
                column: 0,
            },
        ]));
        assert_eq!(layout(&document, 100, "\n"), "a\n(* b\nc *)");
    }

    /// A fresh line starts the next text on a line of its own: a line
    /// break when its line holds text, only the text's indentation when it
    /// is blank, and spaces in between are dropped.
    #[test]
    fn a_fresh_line_starts_the_next_text_on_a_line_of_its_own() {
        let document = concat(vec![
            text("a"),
            Document::FreshLine,
            text(" b"),
            Document::FreshLine,
            indent(2, concat(vec![hard_line(), Document::FreshLine, text("c")])),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "a\nb\n  c");
    }

    /// A fresh line is broken by the next line break rather than doubled.
    #[test]
    fn a_line_break_after_a_fresh_line_is_the_only_one() {
        let document = concat(vec![
            text("(* note *)"),
            Document::FreshLine,
            hard_line(),
            text("next"),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "(* note *)\nnext");
    }

    /// Text after a fresh line on a line holding text never fits, so a
    /// `Choice` takes an alternative that breaks there itself; a measurement
    /// that starts with a fresh line waiting measures from the next line.
    #[test]
    fn text_after_a_fresh_line_does_not_fit() {
        let trailing = || {
            concat(vec![
                text("key"),
                Document::LineSuffix(" /// note".into()),
                Document::FreshLine,
            ])
        };
        let document = Document::Choice(vec![
            concat(vec![trailing(), text(" next: [")]),
            concat(vec![
                trailing(),
                indent(4, concat(vec![hard_line(), text("next: [")])),
            ]),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "key /// note\n    next: [");
        let waiting = concat(vec![
            trailing(),
            group(indent(4, concat(vec![line(), text("x".repeat(90))]))),
        ]);
        assert_eq!(
            layout(&waiting, 100, "\n"),
            format!("key /// note\n    {}", "x".repeat(90))
        );
    }

    /// A `\r` alone is a line break: the column restarts after it, and the
    /// measurement of what follows stops at it.
    #[test]
    fn a_carriage_return_alone_breaks_a_verbatim() {
        let string = format!("'{}\r{}'", "a".repeat(8), "b");
        let document = concat(vec![
            Document::Verbatim(string.clone()),
            group(concat(vec![line(), text("c")])),
        ]);
        assert_eq!(layout(&document, 8, "\n"), format!("{string} c"));
        let leading = group(concat(vec![
            text("x"),
            line(),
            Document::Verbatim(string.clone()),
        ]));
        assert_eq!(layout(&leading, 100, "\n"), format!("x\n{string}"));
    }

    /// Spaces before a `\r` inside a string are the string's own.
    #[test]
    fn trimming_never_cuts_into_a_verbatim_ending_after_a_carriage_return() {
        let document = concat(vec![
            Document::Verbatim("'a\rb  ".into()),
            hard_line(),
            text("c"),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "'a\rb  \nc");
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

    /// `[`, a body on its own lines, `]`: a block laid out multi-line.
    fn multi_line_block() -> Document {
        group(concat(vec![
            text("["),
            indent(2, concat(vec![hard_line(), text("a.")])),
            hard_line(),
            text("]"),
        ]))
    }

    /// The group around the `Choice` was measured only up to the `[`; the
    /// group after the `]` is measured again rather than taken flat.
    #[test]
    fn a_group_after_a_line_break_in_flat_content_is_measured_again() {
        let tail = group(indent(4, concat(vec![line(), text("y".repeat(30))])));
        let document = group(concat(vec![
            text("x"),
            line(),
            Document::Choice(vec![concat(vec![text("foo: "), multi_line_block()])]),
            tail,
        ]));
        assert_eq!(
            layout(&document, 20, "\n"),
            format!("x foo: [\n  a.\n]\n    {}", "y".repeat(30))
        );
    }

    #[test]
    fn an_argument_after_a_hugged_block_is_measured_again() {
        let argument = group(indent(
            4,
            concat(vec![
                text("aaaaaa"),
                line(),
                text("+ bbbbbb"),
                line(),
                text("+ cccccc"),
            ]),
        ));
        let hugged = concat(vec![
            text("x at: "),
            multi_line_block(),
            text(" put: "),
            argument,
        ]);
        let document = Document::Choice(vec![hugged, text("fallback")]);
        assert_eq!(
            layout(&document, 20, "\n"),
            "x at: [\n  a.\n] put: aaaaaa\n    + bbbbbb\n    + cccccc"
        );
    }

    #[test]
    fn a_choice_after_a_line_break_in_flat_content_is_decided_again() {
        let document = group(concat(vec![
            text("x"),
            line(),
            Document::Choice(vec![concat(vec![text("foo: "), multi_line_block()])]),
            Document::Choice(vec![text(format!(" {}", "y".repeat(30))), text(" short")]),
        ]));
        assert_eq!(layout(&document, 20, "\n"), "x foo: [\n  a.\n] short");
    }

    #[test]
    fn trimming_never_cuts_into_a_verbatim() {
        let document = concat(vec![
            Document::Verbatim("'a\nb  ".into()),
            hard_line(),
            Document::Verbatim("'c  ".into()),
            hard_line(),
            text("d"),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "'a\nb  \n'c  \nd");
    }

    #[test]
    fn an_empty_verbatim_does_not_keep_the_spaces_before_it() {
        let document = concat(vec![
            text("a "),
            Document::Verbatim(String::new()),
            hard_line(),
            text("b"),
        ]);
        assert_eq!(layout(&document, 100, "\n"), "a\nb");
    }

    #[test]
    fn when_no_alternative_fits_the_last_one_is_laid_out_broken() {
        let document = Document::Choice(vec![
            text("much too long"),
            group(concat(vec![text("also"), line(), text("long")])),
        ]);
        assert_eq!(layout(&document, 5, "\n"), "also\nlong");
    }

    /// A shared document is written as its content wherever it stands, and
    /// its forced break breaks the groups around each place, the second
    /// found from what the first walk remembered.
    #[test]
    fn a_shared_document_breaks_the_groups_around_every_place_it_stands() {
        let body = Rc::new(concat(vec![text("a"), hard_line(), text("b")]));
        let around = |body: &Rc<Document>| {
            group(concat(vec![
                text("x"),
                line(),
                Document::Shared(Rc::clone(body)),
            ]))
        };
        let document = concat(vec![around(&body), hard_line(), around(&body)]);
        assert_eq!(layout(&document, 100, "\n"), "x\na\nb\nx\na\nb");
    }

    /// A `Choice` whose alternatives share their content takes the first
    /// whose first line fits, as with content of their own.
    #[test]
    fn alternatives_sharing_their_content_are_chosen_as_without_sharing() {
        let argument = Rc::new(text("argument"));
        let document = |width: u32| {
            let choice = Document::Choice(vec![
                concat(vec![
                    text("message: "),
                    Document::Shared(Rc::clone(&argument)),
                ]),
                concat(vec![
                    text("message:"),
                    indent(
                        4,
                        concat(vec![hard_line(), Document::Shared(Rc::clone(&argument))]),
                    ),
                ]),
            ]);
            layout(&choice, width, "\n")
        };
        assert_eq!(document(20), "message: argument");
        assert_eq!(document(10), "message:\n    argument");
    }

    /// Sharing at every level of a deep nesting doubles the paths per level
    /// but not the work: the marking pass walks each shared content once.
    #[test]
    fn nested_sharing_is_walked_once() {
        const LEVELS: usize = 200;
        let mut inner = Rc::new(concat(vec![text("core"), hard_line()]));
        for _ in 0..LEVELS {
            let both = Document::Choice(vec![
                group(Document::Shared(Rc::clone(&inner))),
                group(Document::Shared(Rc::clone(&inner))),
            ]);
            inner = Rc::new(both);
        }
        let document = group(Document::Shared(inner));
        assert_eq!(layout(&document, 100, "\n"), "core\n");
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
