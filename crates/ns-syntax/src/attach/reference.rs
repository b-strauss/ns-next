//! The comment attachment as first written: a flat tree of every
//! commentable node, measured bottom-up, each comment placed by a descent
//! from the module. Compiled for tests only, it is the second derivation
//! `attach_comments` is held to over every source
//! (`the_attachment_attaches_as_the_reference_over_every_source`), as the
//! reference lowering is for the compiler. A change to the rules in
//! `attach.rs`'s module documentation is made here too, in this algorithm's
//! terms; the two never share what a node's own spans, shape, children and
//! empty parts are.

use crate::ast::*;
use crate::comment::Comment;
use crate::span::Span;
use crate::token::{Token, TokenKind};

use super::{
    attached, closes, literal_span, merge, placement, widen, widen_annotation, widen_delimiters,
    Tokens,
};

/// Attaches `comments` to `module` as `attach_comments` does, by the
/// first algorithm.
pub(super) fn attach_comments(
    module: &mut Module,
    tokens: &[Token],
    comments: &[Comment],
    source: &str,
) {
    if comments.is_empty() {
        return;
    }
    let mut tree = Tree::build(module, tokens, comments);
    tree.measure();
    for (index, comment) in comments.iter().enumerate() {
        if tree.inside_a_pattern(comment.span) {
            continue;
        }
        let (owner, anchor) = tree.place(comment.span, source);
        let placement = placement(comments, index, source);
        tree.nodes[owner]
            .comments
            .push(attached(*comment, anchor, placement, source));
    }
}

/// Every commentable node's extent, in the tree's order.
pub(super) fn extents(
    module: &mut Module,
    tokens: &[Token],
    comments: &[Comment],
) -> Vec<Option<(usize, usize)>> {
    let mut tree = Tree::build(module, tokens, comments);
    tree.measure();
    tree.nodes.iter().map(|node| node.extent).collect()
}

/// A commentable node of the tree, flattened.
struct Node<'tree> {
    comments: &'tree mut Vec<AttachedComment>,
    shape: Shape,
    /// The bytes some of the node's own tokens are known to span.
    known: Option<Span>,
    children: Vec<usize>,
    /// The first and the last token of the node, its children's included;
    /// `None` for a node without a token. Taken from the tokens, not from
    /// the node's span: a send's message spans its receiver too, a send
    /// ends before the type arguments after its selector, and a slot or a
    /// code body spans the token after it.
    extent: Option<(usize, usize)>,
}

/// What the extent of a node takes beyond its known span and its
/// children.
enum Shape {
    Module,
    Plain,
    /// A message's selector, whose token the tree does not record.
    Message {
        selector: SelectorShape,
        eventual: bool,
        end: u32,
    },
    /// The implicit receiver, which no token spells.
    Empty,
}

#[derive(Clone, Copy)]
enum SelectorShape {
    Unary,
    Binary,
    Keyword,
}

/// A node still to be flattened, and the index of its parent.
enum Pending<'tree> {
    Class(&'tree mut ClassDeclaration),
    Side(&'tree mut ClassBody),
    Superclass(&'tree mut SuperclassClause),
    Slot(&'tree mut SlotDeclaration),
    Method(&'tree mut MethodDeclaration),
    Body(&'tree mut CodeBody),
    Statement(&'tree mut Statement),
    Expression(&'tree mut Expression),
    Message(&'tree mut MessageSend),
    Block(&'tree mut BlockExpression),
}

struct Tree<'tree> {
    nodes: Vec<Node<'tree>>,
    tokens: Tokens,
    /// The spans of the pattern literals, in source order.
    patterns: Vec<Span>,
}

impl<'tree> Tree<'tree> {
    /// Flattens `module`, its root first, each node before its children.
    fn build(module: &'tree mut Module, tokens: &[Token], comments: &[Comment]) -> Tree<'tree> {
        let mut builder = Builder {
            nodes: Vec::new(),
            pending: Vec::new(),
            dropped: Vec::new(),
            patterns: Vec::new(),
            tokens,
            comments,
        };
        let Module {
            root_class,
            comments,
            ..
        } = module;
        builder.nodes.push(Node {
            comments,
            shape: Shape::Module,
            known: None,
            children: Vec::new(),
            extent: None,
        });
        builder.pending.push((Pending::Class(root_class), 0));
        while let Some((node, parent)) = builder.pending.pop() {
            builder.flatten(node, parent);
        }
        let Builder {
            nodes,
            mut dropped,
            mut patterns,
            ..
        } = builder;
        dropped.sort_by_key(|span| span.start);
        patterns.sort_by_key(|span| span.start);
        let mut kept = Tokens {
            starts: Vec::with_capacity(tokens.len()),
            ends: Vec::with_capacity(tokens.len()),
            kinds: Vec::with_capacity(tokens.len()),
        };
        for token in tokens {
            if token.kind == TokenKind::EndOfInput
                || dropped
                    .binary_search_by_key(&token.span.start, |span| span.start)
                    .is_ok()
            {
                continue;
            }
            kept.starts.push(token.span.start);
            kept.ends.push(token.span.end);
            kept.kinds.push(token.kind);
        }
        Tree {
            nodes,
            tokens: kept,
            patterns,
        }
    }

    /// Each node's extent, from its children's -- a child is flattened
    /// after its parent -- and its children in source order.
    fn measure(&mut self) {
        for index in (0..self.nodes.len()).rev() {
            let mut children = std::mem::take(&mut self.nodes[index].children);
            children.retain(|child| self.nodes[*child].extent.is_some());
            children.sort_by_key(|child| self.nodes[*child].extent.map(|(first, _)| first));
            let children_extent = children
                .iter()
                .filter_map(|child| self.nodes[*child].extent)
                .reduce(merge);
            let node = &self.nodes[index];
            let mut extent = node
                .known
                .and_then(|known| self.tokens.covering(known))
                .into_iter()
                .chain(children_extent)
                .reduce(merge);
            match node.shape {
                Shape::Module => {
                    extent = self.tokens.len().checked_sub(1).map(|last| (0, last));
                }
                Shape::Plain => {}
                Shape::Empty => extent = None,
                Shape::Message {
                    selector,
                    eventual,
                    end,
                } => {
                    let lead = match selector {
                        SelectorShape::Unary => self
                            .tokens
                            .starts
                            .partition_point(|start| *start < end)
                            .checked_sub(1),
                        SelectorShape::Binary | SelectorShape::Keyword => {
                            children_extent.and_then(|(first, _)| first.checked_sub(1))
                        }
                    };
                    if let Some(lead) = lead {
                        let first = if eventual {
                            lead.saturating_sub(1)
                        } else {
                            lead
                        };
                        extent = Some(
                            extent.map_or((first, lead), |extent| merge(extent, (first, lead))),
                        );
                    }
                }
            }
            let node = &mut self.nodes[index];
            node.extent = extent;
            node.children = children;
        }
    }

    fn inside_a_pattern(&self, span: Span) -> bool {
        let before = self
            .patterns
            .partition_point(|pattern| pattern.start <= span.start);
        before
            .checked_sub(1)
            .is_some_and(|index| span.end <= self.patterns[index].end)
    }

    /// The node a comment at `span` belongs to, and where on it.
    fn place(&self, span: Span, source: &str) -> (usize, Anchor) {
        // The comment stands before token `gap`, after token `gap - 1`.
        let gap = self
            .tokens
            .starts
            .partition_point(|start| *start < span.start);
        let mut holder = 0;
        while let Some(child) = self.child_around(holder, gap) {
            holder = child;
        }
        let previous = gap
            .checked_sub(1)
            .map(|index| match self.child_ending_at(holder, index) {
                Some(child) => Neighbour::Child(child),
                None if matches!(
                    self.tokens.kinds[index],
                    TokenKind::Dot | TokenKind::Semicolon
                ) =>
                {
                    index
                        .checked_sub(1)
                        .and_then(|before| self.child_ending_at(holder, before))
                        .map_or(Neighbour::Token(index), Neighbour::Child)
                }
                None => Neighbour::Token(index),
            });
        let next = (gap < self.tokens.len()).then(|| {
            self.child_starting_at(holder, gap)
                .map_or(Neighbour::Token(gap), Neighbour::Child)
        });
        let line_break_before = gap.checked_sub(1).is_some_and(|index| {
            source
                .get(self.tokens.ends[index] as usize..span.start as usize)
                .is_some_and(|between| between.contains(['\n', '\r']))
        });
        let next_closes = matches!(
            next,
            Some(Neighbour::Token(index)) if closes(self.tokens.kinds[index])
        );
        match (previous, next) {
            (Some(Neighbour::Child(child)), _) if !line_break_before => (child, Anchor::After),
            (_, Some(Neighbour::Child(child))) => (child, Anchor::Before),
            (Some(Neighbour::Child(child)), _) if !next_closes => (child, Anchor::After),
            _ => (holder, Anchor::Inner(self.own_tokens_before(holder, gap))),
        }
    }

    /// The child of `holder` with a token on each side of `gap`.
    fn child_around(&self, holder: usize, gap: usize) -> Option<usize> {
        let children = &self.nodes[holder].children;
        let before = children.partition_point(|child| self.first(*child) < gap);
        let child = *children.get(before.checked_sub(1)?)?;
        (gap <= self.last(child)).then_some(child)
    }

    fn child_ending_at(&self, holder: usize, token: usize) -> Option<usize> {
        let children = &self.nodes[holder].children;
        let before = children.partition_point(|child| self.first(*child) <= token);
        let child = *children.get(before.checked_sub(1)?)?;
        (self.last(child) == token).then_some(child)
    }

    fn child_starting_at(&self, holder: usize, token: usize) -> Option<usize> {
        let children = &self.nodes[holder].children;
        let position = children.partition_point(|child| self.first(*child) < token);
        let child = *children.get(position)?;
        (self.first(child) == token).then_some(child)
    }

    /// How many of `holder`'s own tokens stand before `gap`, a `.` that
    /// goes with the child before it not counted.
    fn own_tokens_before(&self, holder: usize, gap: usize) -> u32 {
        let node = &self.nodes[holder];
        let first = node.extent.map_or(0, |(first, _)| first);
        let not_counted: usize = node
            .children
            .iter()
            .filter_map(|child| self.nodes[*child].extent)
            .filter(|(_, last)| *last < gap)
            .map(|(first, last)| {
                let period = last + 1 < gap && self.tokens.kinds[last + 1] == TokenKind::Dot;
                last - first + 1 + usize::from(period)
            })
            .sum();
        let own = gap.saturating_sub(first).saturating_sub(not_counted);
        u32::try_from(own).unwrap_or(u32::MAX)
    }

    /// A child's first token; children without a token are not kept.
    fn first(&self, child: usize) -> usize {
        self.nodes[child].extent.map_or(0, |(first, _)| first)
    }

    fn last(&self, child: usize) -> usize {
        self.nodes[child].extent.map_or(0, |(_, last)| last)
    }
}

#[derive(Clone, Copy)]
enum Neighbour {
    Child(usize),
    Token(usize),
}

struct Builder<'tree, 'input> {
    nodes: Vec<Node<'tree>>,
    pending: Vec<(Pending<'tree>, usize)>,
    /// The tokens of the empty parts.
    dropped: Vec<Span>,
    patterns: Vec<Span>,
    tokens: &'input [Token],
    comments: &'input [Comment],
}

impl<'tree> Builder<'tree, '_> {
    fn add(
        &mut self,
        parent: usize,
        comments: &'tree mut Vec<AttachedComment>,
        shape: Shape,
        known: Option<Span>,
    ) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node {
            comments,
            shape,
            known,
            children: Vec::new(),
            extent: None,
        });
        self.nodes[parent].children.push(index);
        index
    }

    fn flatten(&mut self, node: Pending<'tree>, parent: usize) {
        match node {
            Pending::Class(class) => self.flatten_class(class, parent),
            Pending::Side(side) => self.flatten_side(side, parent),
            Pending::Superclass(clause) => {
                let SuperclassClause {
                    receiver_type_arguments,
                    messages,
                    comments,
                    span,
                    ..
                } = clause;
                let mut known = None;
                widen(&mut known, *span);
                widen_annotation(&mut known, receiver_type_arguments);
                let index = self.add(parent, comments, Shape::Plain, known);
                self.pending.extend(
                    messages
                        .iter_mut()
                        .map(|message| (Pending::Message(message), index)),
                );
            }
            Pending::Slot(slot) => {
                let SlotDeclaration {
                    name,
                    type_annotation,
                    initializer,
                    comments,
                    span,
                    ..
                } = slot;
                let mut known = None;
                widen(
                    &mut known,
                    Span::new(span.start.min(name.span.end), name.span.end),
                );
                widen_annotation(&mut known, type_annotation);
                let index = self.add(parent, comments, Shape::Plain, known);
                if let Some(initializer) = initializer {
                    self.pending.push((Pending::Expression(initializer), index));
                }
            }
            Pending::Method(method) => {
                let MethodDeclaration {
                    body,
                    body_delimiters,
                    comments,
                    span,
                    ..
                } = method;
                let mut known = None;
                widen(&mut known, *span);
                widen_delimiters(&mut known, *body_delimiters);
                let index = self.add(parent, comments, Shape::Plain, known);
                self.pending.push((Pending::Body(body), index));
            }
            Pending::Body(body) => {
                let CodeBody {
                    temporaries,
                    temporaries_delimiters,
                    statements,
                    comments,
                    ..
                } = body;
                if temporaries.is_empty() {
                    self.drop_if_empty(*temporaries_delimiters);
                }
                let mut known = None;
                widen_delimiters(&mut known, *temporaries_delimiters);
                let index = self.add(parent, comments, Shape::Plain, known);
                self.pending.extend(
                    temporaries
                        .iter_mut()
                        .map(|temporary| (Pending::Slot(temporary), index)),
                );
                self.pending.extend(
                    statements
                        .iter_mut()
                        .map(|statement| (Pending::Statement(statement), index)),
                );
            }
            Pending::Statement(statement) => {
                let Statement {
                    kind,
                    comments,
                    span,
                } = statement;
                let mut known = None;
                let expression = match kind {
                    StatementKind::Return(expression) => {
                        // The `^` the statement starts at.
                        widen(
                            &mut known,
                            Span::new(span.start, span.start.saturating_add(1)),
                        );
                        expression
                    }
                    StatementKind::Expression(expression) => expression,
                };
                let index = self.add(parent, comments, Shape::Plain, known);
                self.pending.push((Pending::Expression(expression), index));
            }
            Pending::Expression(expression) => self.flatten_expression(expression, parent),
            Pending::Message(message) => {
                let MessageSend {
                    selector,
                    arguments,
                    eventual,
                    type_arguments,
                    comments,
                    span,
                } = message;
                let mut known = None;
                widen_annotation(&mut known, type_arguments);
                let shape = Shape::Message {
                    selector: match selector {
                        Selector::Unary(_) => SelectorShape::Unary,
                        Selector::Binary(_) => SelectorShape::Binary,
                        Selector::Keyword(_) => SelectorShape::Keyword,
                    },
                    eventual: *eventual,
                    end: span.end,
                };
                let index = self.add(parent, comments, shape, known);
                self.pending.extend(
                    arguments
                        .iter_mut()
                        .map(|argument| (Pending::Expression(argument), index)),
                );
            }
            Pending::Block(block) => {
                let BlockExpression {
                    body,
                    delimiters,
                    comments,
                    span,
                    ..
                } = block;
                let mut known = None;
                widen(&mut known, *span);
                widen_delimiters(&mut known, *delimiters);
                let index = self.add(parent, comments, Shape::Plain, known);
                self.pending.push((Pending::Body(body), index));
            }
        }
    }

    fn flatten_class(&mut self, class: &'tree mut ClassDeclaration, parent: usize) {
        let ClassDeclaration {
            superclass,
            mixin_applications,
            instance_side,
            class_side,
            comments,
            span,
            ..
        } = class;
        if let Some(side) = class_side.as_ref() {
            if side.members.is_empty() && self.drop_if_empty(side.delimiters) {
                // The `:` before the side's `(`.
                if let Some(Delimiters { open, .. }) = side.delimiters {
                    let open_index = self
                        .tokens
                        .partition_point(|token| token.span.start < open.start);
                    if let Some(colon) = open_index.checked_sub(1).map(|index| self.tokens[index]) {
                        if colon.kind == TokenKind::Colon {
                            self.dropped.push(colon.span);
                        }
                    }
                }
            }
        }
        let mut known = None;
        widen(&mut known, *span);
        let index = self.add(parent, comments, Shape::Plain, known);
        if let Some(clause) = superclass {
            self.pending.push((Pending::Superclass(clause), index));
        }
        self.pending.extend(
            mixin_applications
                .iter_mut()
                .map(|clause| (Pending::Superclass(clause), index)),
        );
        self.pending.push((Pending::Side(instance_side), index));
        if let Some(side) = class_side {
            self.pending.push((Pending::Side(side), index));
        }
    }

    fn flatten_side(&mut self, side: &'tree mut ClassBody, parent: usize) {
        let ClassBody {
            header_slots,
            initializer_expressions,
            members,
            delimiters,
            slot_delimiters,
            comments,
            ..
        } = side;
        if header_slots.is_empty() {
            self.drop_if_empty(*slot_delimiters);
        }
        let mut known = None;
        widen_delimiters(&mut known, *delimiters);
        widen_delimiters(&mut known, *slot_delimiters);
        let index = self.add(parent, comments, Shape::Plain, known);
        self.pending.extend(
            header_slots
                .iter_mut()
                .map(|slot| (Pending::Slot(slot), index)),
        );
        self.pending.extend(
            initializer_expressions
                .iter_mut()
                .map(|statement| (Pending::Statement(statement), index)),
        );
        self.pending.extend(members.iter_mut().map(|member| {
            let member = match member {
                Member::Class(class) => Pending::Class(class),
                Member::LazySlot(slot) => Pending::Slot(slot),
                Member::Method(method) => Pending::Method(method),
            };
            (member, index)
        }));
    }

    fn flatten_expression(&mut self, expression: &'tree mut Expression, parent: usize) {
        let Expression {
            kind,
            parentheses,
            type_arguments,
            synthesized,
            comments,
            span,
        } = expression;
        if *synthesized {
            self.add(parent, comments, Shape::Empty, None);
            return;
        }
        let mut known = None;
        widen(&mut known, *span);
        widen_annotation(&mut known, type_arguments);
        for parenthesis in parentheses.iter() {
            widen_delimiters(&mut known, Some(parenthesis.delimiters));
            widen_annotation(&mut known, &parenthesis.type_arguments);
        }
        match &*kind {
            ExpressionKind::Identifier(identifier) => widen(&mut known, identifier.span),
            ExpressionKind::Literal(literal) => {
                let span = literal_span(literal);
                widen(&mut known, span);
                if matches!(literal, Literal::Pattern { .. }) {
                    self.patterns.push(span);
                }
            }
            ExpressionKind::Assign { target, .. } => widen(&mut known, target.span),
            ExpressionKind::DynamicArray { span, .. } => widen(&mut known, *span),
            ExpressionKind::Send(_) | ExpressionKind::Cascade(_) | ExpressionKind::Block(_) => {}
        }
        let index = self.add(parent, comments, Shape::Plain, known);
        match kind {
            ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
            ExpressionKind::Send(send) => {
                let Send {
                    receiver, message, ..
                } = &mut **send;
                self.pending.push((Pending::Expression(receiver), index));
                self.pending.push((Pending::Message(message), index));
            }
            ExpressionKind::Cascade(cascade) => {
                let Cascade {
                    receiver,
                    first,
                    rest,
                    ..
                } = &mut **cascade;
                self.pending.push((Pending::Expression(receiver), index));
                self.pending.push((Pending::Message(first), index));
                self.pending.extend(
                    rest.iter_mut()
                        .map(|message| (Pending::Message(message), index)),
                );
            }
            ExpressionKind::Assign { value, .. } => {
                self.pending.push((Pending::Expression(value), index));
            }
            ExpressionKind::Block(block) => self.pending.push((Pending::Block(block), index)),
            ExpressionKind::DynamicArray { elements, .. } => {
                self.pending.extend(
                    elements
                        .iter_mut()
                        .map(|element| (Pending::Expression(element), index)),
                );
            }
        }
    }

    /// Drops the tokens of an empty part's `delimiters` when no comment
    /// stands between them, and answers whether it did. A part never
    /// closed is the recovery's, and kept.
    fn drop_if_empty(&mut self, delimiters: Option<Delimiters>) -> bool {
        let Some(Delimiters {
            open,
            close: Some(close),
        }) = delimiters
        else {
            return false;
        };
        let first_after_open = self
            .comments
            .partition_point(|comment| comment.span.start < open.end);
        let holds_a_comment = self
            .comments
            .get(first_after_open)
            .is_some_and(|comment| comment.span.end <= close.start);
        if holds_a_comment {
            return false;
        }
        self.dropped.push(open);
        if close != open {
            self.dropped.push(close);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attach::tests::every_source;

    /// Over every module the repository holds: a node's children lie
    /// inside it and apart from each other, and every comment but those a
    /// pattern literal holds is attached exactly once.
    #[test]
    fn the_reference_attaches_every_comment_once_to_a_tree_of_disjoint_extents() {
        for path in every_source() {
            let source = std::fs::read_to_string(&path).expect("read");
            let name = path.display();
            let lexed = crate::lexer::Lexer::new(&source).run_keeping_comments();
            let mut parser = crate::parser::Parser::new(&source, lexed.tokens);
            let mut module = parser.parse_module();
            let (tokens, diagnostics) = parser.into_parts();
            assert!(
                diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.severity != crate::Severity::Error),
                "{name}: {diagnostics:?}"
            );
            attach_comments(&mut module, &tokens, &lexed.comments, &source);
            let mut tree = Tree::build(&mut module, &tokens, &lexed.comments);
            tree.measure();
            for (index, node) in tree.nodes.iter().enumerate() {
                let mut previous_last = None;
                for child in &node.children {
                    let (first, last) = tree.nodes[*child].extent.expect("a kept child");
                    let (outer_first, outer_last) = node.extent.expect("a parent");
                    assert!(
                        outer_first <= first && last <= outer_last,
                        "{name}: node {index}'s child {child} lies outside it"
                    );
                    assert!(
                        previous_last.is_none_or(|previous| previous < first),
                        "{name}: node {index}'s children overlap at {child}"
                    );
                    previous_last = Some(last);
                }
            }
            let attached: usize = tree.nodes.iter().map(|node| node.comments.len()).sum();
            let in_patterns = lexed
                .comments
                .iter()
                .filter(|comment| tree.inside_a_pattern(comment.span))
                .count();
            assert_eq!(attached, lexed.comments.len() - in_patterns, "{name}");
        }
    }
}
