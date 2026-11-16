//! A doc comment's structure: its description, its summary and its tags.
//! The same grammar is read by the JetBrains plugin
//! (`NewspeakDocTags`), so a rule changed here changes there.

use crate::comment::{Comment, CommentKind, lines};
use crate::span::Span;

/// A `///` comment read as documentation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocComment {
    /// The lines before the first tag, an inline `{@summary …}` replaced by
    /// its text, then each unknown tag verbatim as a paragraph of its own:
    /// a misspelt tag stays visible rather than vanishing.
    pub description: String,
    /// The inline summary's text, else the description's first sentence;
    /// `None` for an empty description.
    pub summary: Option<String>,
    /// The tags in source order, unknown ones among them.
    pub tags: Vec<DocTag>,
}

/// One `@name …` tag of a doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocTag {
    pub kind: DocTagKind,
    /// `@name` in the source.
    pub name_span: Span,
    /// The first word after the name and its span in the source, for
    /// `Param` (the parameter's name) and `Signals` (the error's class);
    /// `None` for the other kinds and when the tag's line holds no word
    /// after the name.
    pub argument: Option<(String, Span)>,
    /// The tag's text after its name and argument, its lines joined with
    /// `\n`.
    pub text: String,
}

/// Which tag a [`DocTag`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocTagKind {
    Param,
    Answers,
    Signals,
    See,
    Factory,
    /// Any other name, without its `@`.
    Unknown(String),
}

impl DocTagKind {
    fn named(name: &str) -> DocTagKind {
        match name {
            "param" => DocTagKind::Param,
            "answers" => DocTagKind::Answers,
            "signals" => DocTagKind::Signals,
            "see" => DocTagKind::See,
            "factory" => DocTagKind::Factory,
            _ => DocTagKind::Unknown(name.to_string()),
        }
    }

    fn takes_argument(&self) -> bool {
        matches!(self, DocTagKind::Param | DocTagKind::Signals)
    }
}

/// A comment line without its indentation, `///` and one space, and the
/// source offset its text starts at.
struct DocLine<'source> {
    text: &'source str,
    start: usize,
}

impl DocComment {
    /// Reads `comment`, which `source` holds. The lines are read from the
    /// source rather than from [`Comment::text`] so that every span is a
    /// source offset whatever the line endings. A `Block` comment is no doc
    /// comment and answers an empty one.
    pub fn parse(comment: Comment, source: &str) -> DocComment {
        if comment.kind == CommentKind::Block {
            return DocComment::default();
        }
        let doc_lines = doc_lines(comment.span, source);
        let mut description_lines: Vec<&str> = Vec::new();
        let mut tag_lines: Vec<(DocTagHead<'_>, Vec<&str>)> = Vec::new();
        let mut in_fence = false;
        for line in &doc_lines {
            let trimmed = line.text.trim_start();
            let head = if in_fence { None } else { tag_head(line) };
            if trimmed.starts_with("```") {
                in_fence.toggle();
            }
            match (head, tag_lines.last_mut()) {
                (Some(head), _) => tag_lines.push((head, vec![line.text])),
                (None, Some((_, continuation))) => continuation.push(line.text),
                (None, None) => description_lines.push(line.text),
            }
        }
        let own_description = without_trailing_blank_lines(&description_lines).join("\n");
        let (mut description, inline_summary) = match inline_summary(&own_description) {
            Some((rendered, summary)) => (rendered, Some(summary)),
            None => (own_description, None),
        };
        let summary = inline_summary.or_else(|| first_sentence(&description));
        let mut tags = Vec::with_capacity(tag_lines.len());
        for (head, tag_source_lines) in tag_lines {
            let tag_source_lines = without_trailing_blank_lines(&tag_source_lines);
            if matches!(head.kind, DocTagKind::Unknown(_)) {
                if !description.is_empty() {
                    description.push_str("\n\n");
                }
                description.push_str(tag_source_lines.join("\n").trim_start());
            }
            tags.push(head.into_tag(tag_source_lines));
        }
        DocComment {
            description,
            summary,
            tags,
        }
    }
}

/// What a tag line says before its text: the tag, the span of `@name`,
/// and where on the line the text after the name starts.
struct DocTagHead<'source> {
    kind: DocTagKind,
    name_span: Span,
    /// The line's text after the name.
    rest: &'source str,
    /// The source offset `rest` starts at.
    rest_start: usize,
}

impl DocTagHead<'_> {
    /// The tag of this head and of `lines`, the tag's lines, its own line
    /// first.
    fn into_tag(self, lines: &[&str]) -> DocTag {
        let mut first_line = self.rest.trim_start();
        let mut argument = None;
        if self.kind.takes_argument() && !first_line.is_empty() {
            let word_start = self.rest_start + (self.rest.len() - first_line.len());
            let word_length = first_line
                .find(char::is_whitespace)
                .unwrap_or(first_line.len());
            let word = &first_line[..word_length];
            argument = Some((
                word.to_string(),
                Span::from_offsets(word_start, word_start + word_length),
            ));
            first_line = first_line[word_length..].trim_start();
        }
        let mut text = first_line.to_string();
        for line in lines.iter().skip(1) {
            text.push('\n');
            text.push_str(line);
        }
        DocTag {
            kind: self.kind,
            name_span: self.name_span,
            argument,
            text: text.trim_start().to_string(),
        }
    }
}

/// The comment's lines as [`Comment::text`] strips them, each with the
/// source offset of its text.
fn doc_lines(span: Span, source: &str) -> Vec<DocLine<'_>> {
    let raw = span.text(source);
    let mut line_start = span.start as usize;
    let mut doc_lines = Vec::new();
    for line in lines(raw) {
        let indented = line.trim_start();
        let mut text_start = line_start + (line.len() - indented.len());
        let mut text = indented;
        if let Some(after_marker) = text.strip_prefix("///") {
            text = after_marker;
            text_start += "///".len();
        }
        if let Some(after_space) = text.strip_prefix(' ') {
            text = after_space;
            text_start += 1;
        }
        doc_lines.push(DocLine {
            text: text.trim_end(),
            start: text_start,
        });
        let line_end = line_start + line.len();
        line_start = line_end + line_break_width(&source[line_end..]);
    }
    doc_lines
}

/// The width of the line break `rest` starts with: `\r\n` is one break.
fn line_break_width(rest: &str) -> usize {
    if rest.starts_with("\r\n") { 2 } else { 1 }
}

/// The head of `line` if it is a tag line: its trimmed text starts with
/// `@`, one or more ASCII letters, then whitespace or the line's end.
fn tag_head<'source>(line: &DocLine<'source>) -> Option<DocTagHead<'source>> {
    let trimmed = line.text.trim_start();
    let after_at = trimmed.strip_prefix('@')?;
    let name_length = after_at.bytes().take_while(u8::is_ascii_alphabetic).count();
    if name_length == 0 {
        return None;
    }
    let rest = &after_at[name_length..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name_start = line.start + (line.text.len() - trimmed.len());
    let name_end = name_start + 1 + name_length;
    Some(DocTagHead {
        kind: DocTagKind::named(&after_at[..name_length]),
        name_span: Span::from_offsets(name_start, name_end),
        rest,
        rest_start: name_end,
    })
}

fn without_trailing_blank_lines<'line>(lines: &'line [&'line str]) -> &'line [&'line str] {
    let kept = lines.len()
        - lines
            .iter()
            .rev()
            .take_while(|line| line.trim().is_empty())
            .count();
    &lines[..kept]
}

/// Tracks whether a walk over text stands inside a backtick code span: a
/// run of backticks opens one, and only a run of the same length closes
/// it, as Markdown reads them.
#[derive(Default)]
struct CodeSpans {
    opening_run: Option<usize>,
}

impl CodeSpans {
    /// Steps over the backtick run at the start of `rest`, if any, and
    /// answers its length in bytes (0 when `rest` starts with no backtick).
    fn step(&mut self, rest: &str) -> usize {
        let run = rest.bytes().take_while(|&byte| byte == b'`').count();
        if run > 0 {
            match self.opening_run {
                None => self.opening_run = Some(run),
                Some(opening) if opening == run => self.opening_run = None,
                Some(_) => {}
            }
        }
        run
    }

    fn inside(&self) -> bool {
        self.opening_run.is_some()
    }
}

const SUMMARY_OPENING: &str = "{@summary ";

/// The first `{@summary …}` of `description` outside code, if any: the
/// description with the construct replaced by its text, and the text with
/// its whitespace collapsed. The construct ends at the `}` matching its
/// `{`, braces inside backtick code spans not counted, so a summary may
/// show code that holds a brace.
fn inline_summary(description: &str) -> Option<(String, String)> {
    let mut code = CodeSpans::default();
    let mut offset = 0;
    let opening = loop {
        let rest = description.get(offset..)?;
        let first = rest.chars().next()?;
        let run = code.step(rest);
        if run > 0 {
            offset += run;
            continue;
        }
        if !code.inside() && rest.starts_with(SUMMARY_OPENING) {
            break offset;
        }
        offset += first.len_utf8();
    };
    let text_start = opening + SUMMARY_OPENING.len();
    let mut code = CodeSpans::default();
    let mut depth = 1usize;
    let mut offset = text_start;
    let closing = loop {
        let rest = description.get(offset..)?;
        let first = rest.chars().next()?;
        let run = code.step(rest);
        if run > 0 {
            offset += run;
            continue;
        }
        if !code.inside() {
            match first {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break offset;
                    }
                }
                _ => {}
            }
        }
        offset += first.len_utf8();
    };
    let text = &description[text_start..closing];
    let mut rendered = String::with_capacity(description.len());
    rendered.push_str(&description[..opening]);
    rendered.push_str(text);
    rendered.push_str(&description[closing + 1..]);
    Some((rendered, collapsed(text)))
}

/// The first sentence of `description`'s first paragraph: up to and
/// including the first `.` outside backtick code spans that whitespace
/// follows or that ends the paragraph -- so `a.b` in code or prose does
/// not end it -- else the whole paragraph; whitespace collapsed. `None`
/// for a description with no text.
fn first_sentence(description: &str) -> Option<String> {
    let paragraph: Vec<&str> = lines(description)
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty())
        .collect();
    let paragraph = paragraph.join("\n");
    if paragraph.is_empty() {
        return None;
    }
    let mut code = CodeSpans::default();
    let mut offset = 0;
    while let Some(first) = paragraph[offset..].chars().next() {
        let run = code.step(&paragraph[offset..]);
        if run > 0 {
            offset += run;
            continue;
        }
        offset += first.len_utf8();
        if first == '.'
            && !code.inside()
            && paragraph[offset..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            return Some(collapsed(&paragraph[..offset]));
        }
    }
    Some(collapsed(&paragraph))
}

/// `text` with each run of whitespace one space, none at either end.
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn parsed(source: &str) -> DocComment {
        let comments = crate::lexer::Lexer::new(source)
            .run_keeping_comments()
            .comments;
        let Some(&comment) = comments.first() else {
            panic!("the source holds a comment: {source:?}");
        };
        DocComment::parse(comment, source)
    }

    fn tag(
        kind: DocTagKind,
        argument: Option<&str>,
        text: &str,
    ) -> (DocTagKind, Option<String>, String) {
        (kind, argument.map(str::to_string), text.to_string())
    }

    fn tags(doc: &DocComment) -> Vec<(DocTagKind, Option<String>, String)> {
        doc.tags
            .iter()
            .map(|tag| {
                (
                    tag.kind.clone(),
                    tag.argument.as_ref().map(|(name, _)| name.clone()),
                    tag.text.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_comment_without_tags_is_all_description() {
        let source = indoc! {"
            /// Answers the size
            /// of the receiver.
            size = (^0)
        "};
        let comments = crate::lexer::Lexer::new(source)
            .run_keeping_comments()
            .comments;
        let doc = parsed(source);
        assert_eq!(doc.description, comments[0].text(source));
        assert!(doc.tags.is_empty());
    }

    #[test]
    fn a_factory_tag_has_no_argument() {
        let doc = parsed(indoc! {"
            /// A point.
            /// @factory makes one.
        "});
        assert_eq!(doc.description, "A point.");
        assert_eq!(tags(&doc), [tag(DocTagKind::Factory, None, "makes one.")]);
    }

    #[test]
    fn tags_split_the_description_off() {
        let doc = parsed(indoc! {"
            /// Evaluates.
            ///
            /// @param stop a number.
            /// @param action a block,
            /// sent value:.
            /// @answers nil.
            /// @signals ArgumentError when stop is no number.
            /// @see `to:by:do:`
        "});
        assert_eq!(doc.description, "Evaluates.");
        assert_eq!(
            tags(&doc),
            [
                tag(DocTagKind::Param, Some("stop"), "a number."),
                tag(DocTagKind::Param, Some("action"), "a block,\nsent value:."),
                tag(DocTagKind::Answers, None, "nil."),
                tag(
                    DocTagKind::Signals,
                    Some("ArgumentError"),
                    "when stop is no number."
                ),
                tag(DocTagKind::See, None, "`to:by:do:`"),
            ]
        );
    }

    #[test]
    fn an_unknown_tag_is_kept_in_order() {
        let doc = parsed(indoc! {"
            /// Answers.
            /// @param value a number.
            /// @retrun nil.
            /// @see `other`
        "});
        assert_eq!(
            tags(&doc),
            [
                tag(DocTagKind::Param, Some("value"), "a number."),
                tag(DocTagKind::Unknown("retrun".to_string()), None, "nil."),
                tag(DocTagKind::See, None, "`other`"),
            ]
        );
        assert_eq!(doc.description, "Answers.\n\n@retrun nil.");
    }

    #[test]
    fn a_param_without_a_name_has_no_argument() {
        let doc = parsed(indoc! {"
            /// Answers.
            /// @param
            /// @answers nil.
        "});
        assert_eq!(
            tags(&doc),
            [
                tag(DocTagKind::Param, None, ""),
                tag(DocTagKind::Answers, None, "nil."),
            ]
        );
    }

    #[test]
    fn an_at_sign_inside_a_line_is_text() {
        let doc = parsed(indoc! {"
            /// Answers.
            /// mail me @home
        "});
        assert_eq!(doc.description, "Answers.\nmail me @home");
        assert!(doc.tags.is_empty());
    }

    #[test]
    fn a_tag_line_inside_a_code_fence_is_text() {
        let doc = parsed(indoc! {"
            /// Documents a method:
            /// ```
            /// @param x a number.
            /// ```
            /// @answers nil.
        "});
        assert_eq!(
            doc.description,
            "Documents a method:\n```\n@param x a number.\n```"
        );
        assert_eq!(tags(&doc), [tag(DocTagKind::Answers, None, "nil.")]);
    }

    #[test]
    fn the_summary_is_the_first_sentence() {
        let doc = parsed(indoc! {"
            /// Answers the size. Never nil.
            /// @answers an integer.
        "});
        assert_eq!(doc.summary.as_deref(), Some("Answers the size."));
        let doc = parsed(indoc! {"
            /// Answers   the size
            /// of the receiver
            ///
            /// More. Even more.
        "});
        assert_eq!(
            doc.summary.as_deref(),
            Some("Answers the size of the receiver")
        );
        let doc = parsed(indoc! {"
            /// @answers nil.
            /// @see `other`
        "});
        assert_eq!(doc.summary, None);
    }

    #[test]
    fn a_first_sentence_does_not_end_inside_code() {
        let doc = parsed(indoc! {"
            /// Sends `a.b` first. Then more.
            /// @answers nil.
        "});
        assert_eq!(doc.summary.as_deref(), Some("Sends `a.b` first."));
    }

    #[test]
    fn an_inline_summary_wins_and_reads_as_its_text() {
        let doc = parsed(indoc! {"
            /// {@summary The size.} Answers the size of the receiver.
            /// @answers an integer.
        "});
        assert_eq!(doc.summary.as_deref(), Some("The size."));
        assert_eq!(
            doc.description,
            "The size. Answers the size of the receiver."
        );
    }

    #[test]
    fn a_summary_ends_at_its_closing_brace_outside_code() {
        let doc = parsed(indoc! {"
            /// {@summary Uses `{x}` here.} More.
            /// @answers nil.
        "});
        assert_eq!(doc.summary.as_deref(), Some("Uses `{x}` here."));
        assert_eq!(doc.description, "Uses `{x}` here. More.");
    }

    #[test]
    fn spans_hold_on_crlf_lines() {
        let source = indoc! {"
            /// Evaluates.
            ///
            /// @param stop a number.
            ///   @signals ArgumentError when stop is no number.
        "}
        .replace('\n', "\r\n");
        let doc = parsed(&source);
        assert_eq!(doc.description, "Evaluates.");
        let [param, signals] = doc.tags.as_slice() else {
            panic!("two tags: {:?}", doc.tags);
        };
        assert_eq!(param.name_span.text(&source), "@param");
        let Some((name, name_span)) = &param.argument else {
            panic!("the param names its parameter");
        };
        assert_eq!(name, "stop");
        assert_eq!(name_span.text(&source), "stop");
        assert_eq!(param.text, "a number.");
        assert_eq!(signals.name_span.text(&source), "@signals");
        let Some((class, class_span)) = &signals.argument else {
            panic!("the signals names its class");
        };
        assert_eq!(class, "ArgumentError");
        assert_eq!(class_span.text(&source), "ArgumentError");
    }

    #[test]
    fn a_block_comment_is_an_empty_doc_comment() {
        let doc = parsed("(* Answers. @param x *)");
        assert_eq!(doc, DocComment::default());
    }
}
