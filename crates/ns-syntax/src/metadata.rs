//! §4.3's metadata comments, `(*:tag: payload*)`, each with the target
//! the specification associates it with.
//!
//! A metadata comment is a comment lexically: the lexer, the attachment
//! (`attach.rs`) and the printer treat it as any other `(* ... *)`. The
//! attachment places a comment by layout -- the line it stands on, the
//! closer after it -- while §4.3 associates metadata by adjacency alone:
//! a metadata comment stands *immediately before* `x` when nothing but
//! whitespace and comments lies between it and `x`, and *immediately
//! after* `x` likewise. So the targets are read here, from the tokens
//! next to each comment, and the attachment is left as it is. The tree
//! does not record every token -- a statement's `.`, a method's `=` --
//! and either may stand between a comment and a candidate, so the source
//! is read for the gaps.

use std::collections::HashMap;

use crate::ast::{
    AttachedComment, ClassBody, ClassDeclaration, CodeBody, Expression, ExpressionKind, Member,
    MessagePattern, MessageSend, MethodDeclaration, Module, Parameter, SlotDeclaration, Statement,
    StatementKind, SuperclassClause,
};
use crate::comment::CommentKind;
use crate::span::Span;

/// One metadata comment: its tag, its payload and where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// The text between the comment's first two colons, which opens the
    /// comment after `(*` and any whitespace.
    pub tag: String,
    /// The text after the second colon up to `*)`, trimmed.
    pub payload: String,
    /// The comment, `(*` through `*)`.
    pub span: Span,
}

/// What a metadata comment is associated with (§4.3), each by the span of
/// the target's own syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataTarget {
    /// The module: the comment stands immediately before the root class's
    /// declaration, which is the module declaration.
    Module,
    /// A class, by its declaration's span: the comment stands immediately
    /// after the class's header, before the `(` of its members.
    Class(Span),
    /// A class's factory, by its pattern's span: the comment stands
    /// immediately after the pattern.
    Factory(Span),
    /// A method, by its pattern's span: the comment stands immediately
    /// after the pattern.
    Method(Span),
    /// A parameter of a method, a factory or a block, by its name's span:
    /// the comment stands immediately before the parameter (a block
    /// parameter's `:` or its name).
    Parameter(Span),
    /// A slot, a lazy slot or a temporary, by its name's span: the comment
    /// stands immediately before the declaration's first token.
    Slot(Span),
    /// An expression, by the span of its shape (`ExpressionKind::span`,
    /// the key of every side table): the comment stands immediately before
    /// it, before or inside its parentheses -- the outermost expression
    /// beginning at the next token. A superclass or `<:` clause, the
    /// superclass factory's send, counts as one, by its clause's span, and
    /// so does a return statement, `^` included, by the statement's span.
    Expression(Span),
}

/// Every metadata comment of `module`, which `source` was parsed into, in
/// source order, each with its target. A tagged comment no rule of §4.3
/// associates -- one after an expression, or with a token such as a `.`
/// or an `=` between it and its candidate -- has none and is left out, as
/// is every comment without a tag: an ordinary comment is metadata that
/// specifies no tag (§4.3), which nothing reads. A comment inside a
/// pattern literal is part of the literal and never metadata.
///
/// `module` must be the parse of `source` (`crate::parse` or
/// `crate::parse_for_editor`); for any other `source` the answer is
/// meaningless.
pub fn metadata_of(module: &Module, source: &str) -> Vec<(MetadataTarget, Metadata)> {
    let mut targets = Targets::default();
    collect(module, &mut targets);
    targets
        .comments
        .sort_unstable_by_key(|(span, _)| span.start);
    let gaps = Gaps::new(&targets.comments, source);
    for name in std::mem::take(&mut targets.block_parameters) {
        let colon_end = gaps.preceding_token_end(name.start);
        if colon_end > 0 && source.as_bytes().get(colon_end as usize - 1) == Some(&b':') {
            targets.before(colon_end - 1, name.end, MetadataTarget::Parameter(name));
        }
    }
    let comments = &targets.comments;
    let following = gaps.following_token_starts(comments);
    let preceding = gaps.preceding_token_ends(comments);
    let mut found = Vec::new();
    for (index, (span, kind)) in comments.iter().enumerate() {
        if *kind != CommentKind::Block {
            continue;
        }
        let Some(metadata) = Metadata::read(*span, source) else {
            continue;
        };
        let target = targets
            .before
            .get(&following[index])
            .map(|(_, target)| *target)
            .or_else(|| targets.after.get(&preceding[index]).copied());
        if let Some(target) = target {
            found.push((target, metadata));
        }
    }
    found
}

/// The trivia between tokens: whitespace, as the lexer skips it, and the
/// comments.
struct Gaps<'source> {
    bytes: &'source [u8],
    /// Each comment's start by its end.
    starts: HashMap<u32, u32>,
}

impl<'source> Gaps<'source> {
    fn new(comments: &[(Span, CommentKind)], source: &'source str) -> Gaps<'source> {
        Gaps {
            bytes: source.as_bytes(),
            starts: comments
                .iter()
                .map(|(span, _)| (span.end, span.start))
                .collect(),
        }
    }

    /// The first offset at or after `position` past whitespace alone.
    fn past_whitespace(&self, position: u32) -> u32 {
        let mut position = position;
        while self
            .bytes
            .get(position as usize)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            position += 1;
        }
        position
    }

    /// The last offset at or before `position` back over whitespace alone.
    fn before_whitespace(&self, position: u32) -> u32 {
        let mut position = position;
        while position > 0
            && self
                .bytes
                .get(position as usize - 1)
                .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            position -= 1;
        }
        position
    }

    /// The end of the token before `position`, back over whitespace and
    /// comments; 0 at the source's start.
    fn preceding_token_end(&self, position: u32) -> u32 {
        let mut position = self.before_whitespace(position);
        while let Some(start) = self.starts.get(&position) {
            position = self.before_whitespace(*start);
        }
        position
    }

    /// For each of `comments`, sorted by start, the start of the token
    /// after it, past whitespace and comments; the source's end after the
    /// last token. A run of comments shares one answer, taken from the
    /// next comment's, so a run is crossed once, not once per comment.
    fn following_token_starts(&self, comments: &[(Span, CommentKind)]) -> Vec<u32> {
        let mut following = vec![0; comments.len()];
        for index in (0..comments.len()).rev() {
            let position = self.past_whitespace(comments[index].0.end);
            following[index] = match comments.get(index + 1) {
                Some((next, _)) if next.start == position => following[index + 1],
                _ => position,
            };
        }
        following
    }

    /// For each of `comments`, sorted by start, the end of the token
    /// before it, back over whitespace and comments, a run crossed once.
    fn preceding_token_ends(&self, comments: &[(Span, CommentKind)]) -> Vec<u32> {
        let mut preceding: Vec<u32> = Vec::with_capacity(comments.len());
        for (index, (span, _)) in comments.iter().enumerate() {
            let position = self.before_whitespace(span.start);
            let answer = match index.checked_sub(1) {
                Some(previous) if comments[previous].0.end == position => preceding[previous],
                _ => position,
            };
            preceding.push(answer);
        }
        preceding
    }
}

impl Metadata {
    /// The comment `span` of `source` read as metadata, whatever its
    /// target: `None` for a comment that is no metadata. A reader of
    /// comments that must tell metadata from an ordinary comment -- a
    /// house rule on comments, a doc comment's lookup past the metadata
    /// below it -- asks here rather than reading the text itself.
    pub fn read(span: Span, source: &str) -> Option<Metadata> {
        read(span, source)
    }
}

/// A comment's tag and payload, `None` for a comment that is no metadata:
/// a `(* ... *)` whose text does not open with `:`, a tag, and a second
/// `:`. A tag is a run of characters without whitespace -- §4.3 calls it
/// an identifier, yet its example writes `baz-metadata` and `x=meta` --
/// and is never empty. An unterminated comment, which only a recovered
/// parse has, is no metadata.
fn read(span: Span, source: &str) -> Option<Metadata> {
    let text = source.get(span.start as usize..span.end as usize)?;
    let inside = text.strip_prefix("(*")?.strip_suffix("*)")?;
    let opened = inside.trim_start().strip_prefix(':')?;
    let (tag, payload) = opened.split_once(':')?;
    if tag.is_empty() || tag.contains(char::is_whitespace) {
        return None;
    }
    Some(Metadata {
        tag: tag.to_string(),
        payload: payload.trim().to_string(),
        span,
    })
}

/// What the walk over the tree collects: every comment, and the targets
/// by the token next to them.
#[derive(Default)]
struct Targets {
    /// Every attached comment, the `///` blocks among them, since any
    /// comment may stand between a metadata comment and its target.
    comments: Vec<(Span, CommentKind)>,
    /// The targets a comment precedes, by the start of their first token;
    /// beside each, the end of what it covers, so that the outermost of
    /// several targets beginning at one token is kept.
    before: HashMap<u32, (u32, MetadataTarget)>,
    /// The targets a comment follows, by the end of their last token.
    after: HashMap<u32, MetadataTarget>,
    /// The names of the block parameters, whose `:` the tree does not
    /// record: a comment before the `:` precedes the parameter too.
    block_parameters: Vec<Span>,
}

impl Targets {
    fn before(&mut self, start: u32, end: u32, target: MetadataTarget) {
        let entry = self.before.entry(start).or_insert((end, target));
        if end > entry.0 {
            *entry = (end, target);
        }
    }

    fn comments(&mut self, comments: &[AttachedComment]) {
        self.comments
            .extend(comments.iter().map(|comment| (comment.span, comment.kind)));
    }

    fn parameter(&mut self, parameter: &Parameter) {
        let name = parameter.name.span;
        self.before(name.start, name.end, MetadataTarget::Parameter(name));
    }

    /// A method's or a factory's pattern: its parameters, which a comment
    /// may precede, and its end, which one may follow.
    fn pattern(&mut self, pattern: &MessagePattern, target: fn(Span) -> MetadataTarget) {
        let span = match pattern {
            MessagePattern::Unary {
                selector,
                type_parameters,
            } => type_parameters
                .as_ref()
                .map_or(selector.span, |types| selector.span.merge(types.span)),
            MessagePattern::Binary {
                selector,
                parameter,
            } => {
                self.parameter(parameter);
                selector.span.merge(parameter_span(parameter))
            }
            MessagePattern::Keyword { parts } => {
                let mut span: Option<Span> = None;
                for part in parts {
                    self.parameter(&part.parameter);
                    let covered = part.keyword.span.merge(parameter_span(&part.parameter));
                    span = Some(span.map_or(covered, |span| span.merge(covered)));
                }
                let Some(span) = span else {
                    return;
                };
                span
            }
        };
        self.after.insert(span.end, target(span));
    }
}

/// A parameter's name and the type annotation after it.
fn parameter_span(parameter: &Parameter) -> Span {
    parameter
        .type_annotation
        .as_ref()
        .map_or(parameter.name.span, |types| {
            parameter.name.span.merge(types.span)
        })
}

/// A node the walk has still to visit.
enum Node<'tree> {
    Class(&'tree ClassDeclaration),
    Body(&'tree ClassBody, Option<Span>),
    Superclass(&'tree SuperclassClause),
    Slot(&'tree SlotDeclaration),
    Method(&'tree MethodDeclaration),
    Code(&'tree CodeBody),
    Statement(&'tree Statement),
    Expression(&'tree Expression),
    Message(&'tree MessageSend),
}

/// Walks the tree from an explicit stack -- a chain of messages is as
/// deep as the source is long, so the walk never recurses -- collecting
/// every comment and every target.
fn collect(module: &Module, targets: &mut Targets) {
    targets.comments(&module.comments);
    let root = &module.root_class;
    targets.before(root.span.start, root.span.end, MetadataTarget::Module);
    let mut pending = vec![Node::Class(root)];
    while let Some(node) = pending.pop() {
        match node {
            Node::Class(class) => {
                targets.comments(&class.comments);
                if let Some(factory) = &class.factory {
                    targets.pattern(factory, MetadataTarget::Factory);
                }
                pending.extend(class.superclass.iter().map(Node::Superclass));
                pending.extend(class.mixin_applications.iter().map(Node::Superclass));
                pending.push(Node::Body(&class.instance_side, Some(class.span)));
                pending.extend(class.class_side.iter().map(|side| Node::Body(side, None)));
            }
            Node::Body(body, class) => {
                targets.comments(&body.comments);
                // The header ends at the `)` right before the members' `(`,
                // with nothing but whitespace and comments between, so a
                // comment after the header is one before that `(`.
                if let (Some(class), Some(members)) = (class, body.members_delimiters) {
                    targets.before(
                        members.open.start,
                        members.open.end,
                        MetadataTarget::Class(class),
                    );
                }
                pending.extend(body.header_slots.iter().map(Node::Slot));
                pending.extend(body.initializer_expressions.iter().map(Node::Statement));
                pending.extend(body.members.iter().map(|member| match member {
                    Member::Class(class) => Node::Class(class),
                    Member::LazySlot(slot) => Node::Slot(slot),
                    Member::Method(method) => Node::Method(method),
                }));
            }
            Node::Superclass(clause) => {
                targets.comments(&clause.comments);
                targets.before(
                    clause.span.start,
                    clause.span.end,
                    MetadataTarget::Expression(clause.span),
                );
                pending.extend(clause.messages.iter().map(Node::Message));
            }
            Node::Slot(slot) => {
                targets.comments(&slot.comments);
                targets.before(
                    slot.span.start,
                    slot.span.end,
                    MetadataTarget::Slot(slot.name.span),
                );
                pending.extend(slot.initializer.iter().map(Node::Expression));
            }
            Node::Method(method) => {
                targets.comments(&method.comments);
                targets.pattern(&method.pattern, MetadataTarget::Method);
                pending.push(Node::Code(&method.body));
            }
            Node::Code(body) => {
                targets.comments(&body.comments);
                pending.extend(body.temporaries.iter().map(Node::Slot));
                pending.extend(body.statements.iter().map(Node::Statement));
            }
            Node::Statement(statement) => {
                targets.comments(&statement.comments);
                let expression = match &statement.kind {
                    StatementKind::Expression(expression) => expression,
                    StatementKind::Return(expression) => {
                        let span = statement.span;
                        targets.before(span.start, span.end, MetadataTarget::Expression(span));
                        expression
                    }
                };
                pending.push(Node::Expression(expression));
            }
            Node::Expression(expression) => {
                targets.comments(&expression.comments);
                // An implicit receiver has no token; the send around it
                // begins where its selector does.
                if !expression.synthesized {
                    let shape = expression.kind.span();
                    let target = MetadataTarget::Expression(shape);
                    // Before the parentheses or inside them, before the shape.
                    targets.before(expression.span.start, expression.span.end, target);
                    targets.before(shape.start, expression.span.end, target);
                }
                match &expression.kind {
                    ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
                    ExpressionKind::Send(send) => {
                        pending.push(Node::Expression(&send.receiver));
                        pending.push(Node::Message(&send.message));
                    }
                    ExpressionKind::Cascade(cascade) => {
                        pending.push(Node::Expression(&cascade.receiver));
                        pending.push(Node::Message(&cascade.first));
                        pending.extend(cascade.rest.iter().map(Node::Message));
                    }
                    ExpressionKind::Assign { value, .. } => {
                        pending.push(Node::Expression(value));
                    }
                    ExpressionKind::Block(block) => {
                        targets.comments(&block.comments);
                        for parameter in &block.parameters {
                            targets.parameter(parameter);
                            targets.block_parameters.push(parameter.name.span);
                        }
                        pending.push(Node::Code(&block.body));
                    }
                    ExpressionKind::DynamicArray { elements, .. } => {
                        pending.extend(elements.iter().map(Node::Expression));
                    }
                }
            }
            Node::Message(message) => {
                targets.comments(&message.comments);
                pending.extend(message.arguments.iter().map(Node::Expression));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Metadata, MetadataTarget, metadata_of};
    use crate::span::Span;
    use indoc::indoc;

    /// The span of the `occurrence`-th (from 0) appearance of `needle`.
    fn span_of(source: &str, needle: &str, occurrence: usize) -> Span {
        let Some((start, _)) = source.match_indices(needle).nth(occurrence) else {
            panic!("`{needle}` appears {} times", occurrence + 1);
        };
        Span::from_offsets(start, start + needle.len())
    }

    fn metadata(source: &str) -> Vec<(MetadataTarget, Metadata)> {
        let Ok(module) = crate::parse(source) else {
            panic!("the module parses");
        };
        metadata_of(&module, source)
    }

    /// Each collected metadata as its target, tag and payload, its span
    /// checked to be the comment's.
    fn targets(source: &str) -> Vec<(MetadataTarget, String, String)> {
        metadata(source)
            .into_iter()
            .map(|(target, metadata)| {
                let text = metadata.span.text(source);
                assert!(
                    text.starts_with("(*") && text.ends_with("*)"),
                    "the span `{text}` is the whole comment"
                );
                (target, metadata.tag, metadata.payload)
            })
            .collect()
    }

    fn entry(target: MetadataTarget, tag: &str, payload: &str) -> (MetadataTarget, String, String) {
        (target, tag.to_string(), payload.to_string())
    }

    #[test]
    fn a_module_metadata_stands_above_its_class() {
        let source = indoc! {"
            (* :module: com.acme.json.Json@2.1.0 *)
            class Json = (
            ) (
            )
        "};
        assert_eq!(
            targets(source),
            [entry(
                MetadataTarget::Module,
                "module",
                "com.acme.json.Json@2.1.0"
            )]
        );
        let documented = indoc! {"
            Newspeak3
            'Json'
            /// Reads and writes JSON.
            /// It knows nothing else.
            (* :module: com.acme.json.Json@2.1.0 *)
            class Json = (
            ) (
            )
        "};
        assert_eq!(
            targets(documented),
            [entry(
                MetadataTarget::Module,
                "module",
                "com.acme.json.Json@2.1.0"
            )]
        );
    }

    #[test]
    fn a_parameter_metadata_belongs_to_the_parameter() {
        let source = indoc! {"
            class Reports usingPlatform: platform json: (* :module: com.acme.json.Json@2 *) json = (
            ) (
            )
        "};
        assert_eq!(
            targets(source),
            [entry(
                MetadataTarget::Parameter(span_of(source, "json", 2)),
                "module",
                "com.acme.json.Json@2"
            )]
        );
    }

    #[test]
    fn a_block_parameter_metadata_belongs_to_the_parameter() {
        let source = indoc! {"
            class Lists = (
            ) (
              each = (
                ^[(* :before: *) :item (* :between: *) | item]
              )
              split = (
                ^[: (* :inside: *) item | item]
              )
            )
        "};
        assert_eq!(
            targets(source),
            [
                entry(
                    MetadataTarget::Parameter(span_of(source, "item", 0)),
                    "before",
                    ""
                ),
                entry(
                    MetadataTarget::Parameter(span_of(source, "item", 2)),
                    "inside",
                    ""
                ),
            ]
        );
    }

    #[test]
    fn a_factory_metadata_follows_its_pattern() {
        let source = indoc! {"
            class Foo bar: x (*:exemplar: Foo bar: 3 *) = (
            ) (
            )
        "};
        assert_eq!(
            targets(source),
            [entry(
                MetadataTarget::Factory(span_of(source, "bar: x", 0)),
                "exemplar",
                "Foo bar: 3"
            )]
        );
    }

    #[test]
    fn a_method_metadata_follows_its_pattern() {
        let source = indoc! {"
            class Shape = (
            ) (
              area: scale (* :exemplar: area: 2 *) = (
                ^scale
              )
              perimeter (* :exemplar: perimeter *) (* :pure: *) = (
                ^4
              )
              + other (* :exemplar: + 1 *) = (
                ^other
              )
            )
        "};
        assert_eq!(
            targets(source),
            [
                entry(
                    MetadataTarget::Method(span_of(source, "area: scale", 0)),
                    "exemplar",
                    "area: 2"
                ),
                entry(
                    MetadataTarget::Method(span_of(source, "perimeter", 0)),
                    "exemplar",
                    "perimeter"
                ),
                entry(
                    MetadataTarget::Method(span_of(source, "perimeter", 0)),
                    "pure",
                    ""
                ),
                entry(
                    MetadataTarget::Method(span_of(source, "+ other", 0)),
                    "exemplar",
                    "+ 1"
                ),
            ]
        );
    }

    #[test]
    fn a_slot_and_an_expression() {
        let source = indoc! {"
            class Shape = (
              | (* :type: Integer *) width = (* :default: *) 3. |
            ) (
              (* :lazy: *) lazy cached = 1.
              area = (
                | (* :type: Integer *) scale |
                scale:: 2.
                ^ (* :result: *) (width * scale) printString
              )
            )
        "};
        let result = span_of(source, "(width * scale) printString", 0);
        assert_eq!(
            targets(source),
            [
                entry(
                    MetadataTarget::Slot(span_of(source, "width", 0)),
                    "type",
                    "Integer"
                ),
                entry(
                    MetadataTarget::Expression(span_of(source, "3", 0)),
                    "default",
                    ""
                ),
                entry(
                    MetadataTarget::Slot(span_of(source, "cached", 0)),
                    "lazy",
                    ""
                ),
                entry(
                    MetadataTarget::Slot(span_of(source, "scale", 0)),
                    "type",
                    "Integer"
                ),
                entry(MetadataTarget::Expression(result), "result", ""),
            ]
        );
    }

    #[test]
    fn a_nested_class_a_mixin_clause_and_parentheses() {
        let source = indoc! {"
            class Outer = (
            ) (
              class Inner = Object <: (* :mixin: *) Mixin (
              ) (* :inner: *)
              /// Inner's members.
              (
              )
              run = (
                ^ (* :product: *) ((* :sum: *) 3 + 4) * 2
              )
            )
        "};
        let inner = Span::new(
            span_of(source, "class Inner", 0).start,
            span_of(
                source,
                "  )
  run",
                0,
            )
            .start
                + 3,
        );
        assert_eq!(
            targets(source),
            [
                entry(
                    MetadataTarget::Expression(span_of(source, "Mixin", 0)),
                    "mixin",
                    ""
                ),
                entry(MetadataTarget::Class(inner), "inner", ""),
                entry(
                    MetadataTarget::Expression(span_of(source, "((* :sum: *) 3 + 4) * 2", 0)),
                    "product",
                    ""
                ),
                entry(
                    MetadataTarget::Expression(span_of(source, "3 + 4", 0)),
                    "sum",
                    ""
                ),
            ]
        );
    }

    #[test]
    fn the_specification_s_example() {
        // §4.3's example, its slot's `(:type: Integer *)` written as the
        // comment it is meant to be and the superclass clause without the
        // `=` before the class's body, which today's grammar has no place
        // for (§6.3).
        let source = indoc! {"
            class Foo bar: x (*:exemplar: Foo bar: #baz *) = (* :superfactory: Bla send metadata *) Bla bar (
                | (* :type: Integer *) x = 3. |
              (* :baz-metadata: *) baz: (* :x=meta: *) x.
            ) (* :class-meta: *) (
               baz: (* :type: Integer *) t (* :return-type: ^Integer *) (* :exemplar: baz: 42*) = (
                    (*:setter-meta: *) y:: (*:send-meta: *) x.
                    (*:return-meta: *) ^ (*:send-meta: *) x
                 )
            )
        "};
        let class = Span::from_offsets(0, source.trim_end().len());
        let method_pattern = Span::new(
            span_of(source, "baz:", 1).start,
            span_of(source, " t ", 0).end - 1,
        );
        let slot = Span::new(
            span_of(source, "x = 3", 0).start,
            span_of(source, "x = 3", 0).start + 1,
        );
        let parameter = Span::new(
            span_of(source, " t ", 0).start + 1,
            span_of(source, " t ", 0).end - 1,
        );
        let assignment = Span::new(
            span_of(source, "y::", 0).start,
            span_of(source, "x.", 1).start + 1,
        );
        let return_statement = Span::new(
            span_of(source, "^ (*", 0).start,
            span_of(source, "*) x\n", 0).end - 1,
        );
        let baz_send = Span::new(
            span_of(source, "baz:", 0).start,
            span_of(source, "x.", 0).start + 1,
        );
        assert_eq!(
            targets(source),
            [
                entry(
                    MetadataTarget::Factory(span_of(source, "bar: x", 0)),
                    "exemplar",
                    "Foo bar: #baz"
                ),
                entry(
                    MetadataTarget::Expression(span_of(source, "Bla bar", 0)),
                    "superfactory",
                    "Bla send metadata"
                ),
                entry(MetadataTarget::Slot(slot), "type", "Integer"),
                entry(MetadataTarget::Expression(baz_send), "baz-metadata", ""),
                entry(
                    MetadataTarget::Expression(Span::new(baz_send.end - 1, baz_send.end)),
                    "x=meta",
                    ""
                ),
                entry(MetadataTarget::Class(class), "class-meta", ""),
                entry(MetadataTarget::Parameter(parameter), "type", "Integer"),
                entry(
                    MetadataTarget::Method(method_pattern),
                    "return-type",
                    "^Integer"
                ),
                entry(
                    MetadataTarget::Method(method_pattern),
                    "exemplar",
                    "baz: 42"
                ),
                entry(MetadataTarget::Expression(assignment), "setter-meta", ""),
                entry(
                    MetadataTarget::Expression(Span::new(assignment.end - 1, assignment.end)),
                    "send-meta",
                    ""
                ),
                entry(
                    MetadataTarget::Expression(return_statement),
                    "return-meta",
                    ""
                ),
                entry(
                    MetadataTarget::Expression(Span::new(
                        return_statement.end - 1,
                        return_statement.end
                    )),
                    "send-meta",
                    ""
                ),
            ]
        );
    }

    /// `Metadata::read` reads a comment by its span alone, so that a
    /// comment no target takes in is metadata to it too.
    #[test]
    fn a_comment_reads_as_metadata_whatever_its_target() {
        let source = "1 (* :after: x *) (* plain *)";
        let tagged = span_of(source, "(* :after: x *)", 0);
        let Some(metadata) = super::Metadata::read(tagged, source) else {
            panic!("the tagged comment is metadata");
        };
        assert_eq!(
            (metadata.tag.as_str(), metadata.payload.as_str()),
            ("after", "x")
        );
        assert_eq!(metadata.span, tagged);
        let plain = span_of(source, "(* plain *)", 0);
        assert_eq!(super::Metadata::read(plain, source), None);
    }

    #[test]
    fn an_untagged_comment_is_no_metadata() {
        let source = indoc! {"
            (* plain *)
            class Plain = (
              (* : no tag *)
            ) (
              /// :module: x
              run = (
                (* :: empty *)
                (* :two words: x *)
                ^ (* *) 1
              )
            )
        "};
        assert_eq!(targets(source), []);
    }

    #[test]
    fn a_metadata_with_anything_but_comments_between_has_no_target() {
        let source = indoc! {"
            class Spaced = (
            ) (
              run = (
                1 (* :after-the-first: *). 2.
                ^3
              )
              run: count = (* :after-the-equals: *) (
                ^count
              )
            )
        "};
        assert_eq!(targets(source), []);
    }
}
