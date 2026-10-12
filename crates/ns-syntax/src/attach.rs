//! Every comment, attached to the node of the syntax tree it belongs to.
//!
//! A reprint from the tree alone puts each comment back where its
//! attachment says, so this rule is the contract the printer mirrors: a
//! printer counts a node's own tokens as listed here.
//!
//! # Commentable nodes and their own tokens
//!
//! The *commentable nodes* are `Module`, `ClassDeclaration`, `ClassBody`,
//! `SlotDeclaration`, `MethodDeclaration`, `CodeBody`, `Statement`,
//! `Expression`, `MessageSend`, `BlockExpression` and `SuperclassClause`.
//! Every other part -- an `Identifier`, a `TypeAnnotation`, a `Category`, a
//! keyword, a delimiter -- consists of *own tokens* of the innermost
//! commentable node around it. Each node's own tokens and commentable
//! children (in ‹ ›), in source order:
//!
//! - `Module`: the language id, the category string; ‹the root class›.
//! - `ClassDeclaration`: the access modifier, `class`, the name, the type
//!   parameters' tokens, the factory pattern's tokens (as a method's
//!   pattern below), `^` and the factory's return type's tokens, the
//!   factory's `where` clause's tokens, `=`; ‹the superclass clause›; per
//!   mixin application `<:` ‹its clause›; then `.` (a mixin application
//!   without a body) or ‹the instance side›; then `:` ‹the class side›.
//! - `ClassBody`, instance side: `(`, the slot section's opening `|` or
//!   `||`, ‹each header slot›, its closing bar, ‹each initializer
//!   expression› (a `Statement`) followed by its `.` where one is written,
//!   `)`, `(`, ‹each member›, `)`. Class side: `(` ‹each method› `)`.
//! - `SlotDeclaration` (a header slot, a temporary or a lazy slot): the
//!   access modifier and `lazy` as written, the name, the type
//!   annotation's tokens, `=` or `::=` ‹the initializer›, and the `.` after
//!   the slot where one is written.
//! - `MethodDeclaration`: the access modifier; the pattern's tokens -- a
//!   unary selector and the type tokens after it, a binary selector, its
//!   parameter's name and type tokens, or each keyword, its parameter's
//!   name and type tokens; `^` and the return type's tokens; the `where`
//!   clause's tokens; `=`, `(` ‹the code body› `)`.
//! - `CodeBody`: the temporaries' opening `|` or `||`, ‹each temporary›,
//!   their closing bar, ‹each statement› followed by its `.` where one is
//!   written, the last statement's included.
//! - `Statement`: `^` for a return; ‹the expression›.
//! - `Expression`: each parenthesis's `(`, outermost first; the shape's own
//!   tokens -- an identifier's token, a literal's token (a pattern literal:
//!   every token its text lexes to, a run of `>` one token), a setter
//!   keyword `name::` before ‹the assigned value›, a tuple's `{` ‹each
//!   element› followed by its `.` `}`, a cascade's ‹receiver› ‹first
//!   message› then `;` ‹message› for each further one, a send's ‹receiver›
//!   ‹message› and a block's ‹block› with no own token between; then the
//!   type argument tokens after the shape (`Box[Integer]`); then each `)`,
//!   innermost first, each followed by its parenthesis's type argument
//!   tokens (`(Box)[Integer]`). An expression the parser made up (the
//!   implicit receiver, `synthesized`) has no token.
//! - `MessageSend`: `<-:` when eventual; then a unary selector and the type
//!   argument tokens after it, or a binary selector ‹the argument›, or each
//!   keyword ‹its argument›.
//! - `BlockExpression`: `[`, each parameter's `:`, name and type tokens, the
//!   parameters' `|`; ‹the code body›; `]`.
//! - `SuperclassClause`: `self`, `super`, or `outer` and the name after
//!   it; the receiver's name and the type argument tokens after it; ‹the
//!   message›.
//!
//! A `.` right after a child goes with that child: it is an own token of
//! the node around it, but one `Inner(n)` does not count, and as `P` below
//! it stands for the child. A reprint writes the period after a statement,
//! an initializer expression or a tuple's element by rules of its own, so
//! whether the source had one must move no comment.
//!
//! The tokens counted are the parser's final ones (`Parser::into_parts`):
//! `3 -4` is the three tokens `3`, `-`, `4`, as a printer prints it. The
//! tokens of an *empty part*, which a reprint drops, are nobody's: a slot
//! section or temporaries without a declaration and without a comment
//! between the bars (`| |`, `||`, `|| ||`), and a class side without a
//! method and without a comment between its `( )` -- its `:` included.
//! The tree keeps such a part as written (`Module::without_positions`
//! reads it as absent); counting none of its tokens attaches every comment
//! as it attaches in the source without the part.
//!
//! # Where a comment goes
//!
//! A comment lies between two tokens. Find the innermost commentable node
//! `N` with an own token or a child's token on each side of it (the module
//! holds every comment). Among `N`'s commentable children and own tokens,
//! let `P` be the last one ending before the comment and `Q` the first one
//! starting after it; a `.` right after a child stands for that child as
//! `P`.
//!
//! 1. `P` is a child and no line break lies between the token before the
//!    comment and the comment → `After` on `P` (trailing).
//! 2. Otherwise, `Q` is a child → `Before` on `Q` (leading).
//! 3. Otherwise, `P` is a child and `Q` is no closer of `N` (`)`, `]`, `}`,
//!    `|`, `||`) → `After` on `P`.
//! 4. Otherwise → `Inner(n)` on `N`, `n` the number of `N`'s own tokens
//!    before the comment, a `.` that goes with a child not counted
//!    (dangling, before a closer, or between own tokens).
//!
//! A comment before the root class is `Before` it, one after it `After`
//! it, and one among the prologue's tokens `Inner(n)` on the module. A
//! comment inside a pattern literal is part of the literal's `text` and
//! attached nowhere else. Several comments on one node keep their source
//! order. A comment's `Placement` comes from its line: `OwnLine` when only
//! whitespace precedes it there, else `EndOfLine` when only whitespace
//! follows it up to a line break or the end (a `///` block is always one
//! of the two), else `Inline` -- other comments beside it on the line
//! counting as whitespace.
//!
//! The walk takes a chain of messages link by link from a stack, never by
//! recursing along a receiver, and never panics on a recovered tree: a
//! zero-width span covers no token, overlapping nodes from a recovery are
//! taken as they come, and a comment no node holds goes to the module.

use crate::ast::*;
use crate::comment::{lines_of, starts_its_line, Comment, CommentKind};
use crate::span::Span;
use crate::token::{Token, TokenKind};

/// Attaches each of `comments` (in source order, as the lexer answers
/// them) to its node of `module`, parsed from `source` into `tokens`.
pub(crate) fn attach_comments(
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
    /// ends before the type arguments after its selector, and a bare slot
    /// or a code body spans the token after it.
    extent: Option<(usize, usize)>,
}

/// What the extent of a node takes beyond its known span and its
/// children.
enum Shape {
    Module,
    Plain,
    /// A slot's `.`, when the parser ended the slot's span at it.
    Slot {
        end: u32,
    },
    /// A body's `.` after its last statement.
    Body {
        has_statements: bool,
    },
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

/// The tokens a comment is placed among: the parser's, without the end of
/// input and without an empty part's.
struct Tokens {
    starts: Vec<u32>,
    ends: Vec<u32>,
    kinds: Vec<TokenKind>,
}

impl Tokens {
    fn len(&self) -> usize {
        self.starts.len()
    }

    /// The first and the last token overlapping `span`.
    fn covering(&self, span: Span) -> Option<(usize, usize)> {
        if span.start >= span.end {
            return None;
        }
        let first = self.ends.partition_point(|end| *end <= span.start);
        let past = self.starts.partition_point(|start| *start < span.end);
        (first < past).then(|| (first, past - 1))
    }
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
            let next_is_a_period = |last: usize, end: u32| {
                self.tokens.kinds.get(last + 1) == Some(&TokenKind::Dot)
                    && self.tokens.ends[last + 1] <= end
            };
            match node.shape {
                Shape::Module => {
                    extent = self.tokens.len().checked_sub(1).map(|last| (0, last));
                }
                Shape::Plain => {}
                Shape::Empty => extent = None,
                Shape::Slot { end } => {
                    if let Some((first, last)) = extent {
                        if next_is_a_period(last, end) {
                            extent = Some((first, last + 1));
                        }
                    }
                }
                Shape::Body { has_statements } => {
                    if let Some((first, last)) = extent {
                        if has_statements && next_is_a_period(last, u32::MAX) {
                            extent = Some((first, last + 1));
                        }
                    }
                }
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
                None if self.tokens.kinds[index] == TokenKind::Dot => index
                    .checked_sub(1)
                    .and_then(|before| self.child_ending_at(holder, before))
                    .map_or(Neighbour::Token(index), Neighbour::Child),
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

/// Whether a token of this kind closes the node it stands in: what stands
/// before it on its own lines dangles in the node.
fn closes(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RightParenthesis
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::Bar
            | TokenKind::DoubleBar
    )
}

fn merge(first: (usize, usize), second: (usize, usize)) -> (usize, usize) {
    (first.0.min(second.0), first.1.max(second.1))
}

fn widen(known: &mut Option<Span>, span: Span) {
    if span.start < span.end {
        *known = Some(known.map_or(span, |known| known.merge(span)));
    }
}

fn widen_annotation(known: &mut Option<Span>, annotation: &Option<TypeAnnotation>) {
    if let Some(annotation) = annotation {
        widen(known, annotation.span);
    }
}

fn widen_delimiters(known: &mut Option<Span>, delimiters: Option<Delimiters>) {
    if let Some(delimiters) = delimiters {
        widen(known, delimiters.open);
        if let Some(close) = delimiters.close {
            widen(known, close);
        }
    }
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
                let index = self.add(parent, comments, Shape::Slot { end: span.end }, known);
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
                let shape = Shape::Body {
                    has_statements: !statements.is_empty(),
                };
                let index = self.add(parent, comments, shape, known);
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

fn literal_span(literal: &Literal) -> Span {
    match literal {
        Literal::Integer { span, .. }
        | Literal::Float { span, .. }
        | Literal::String { span, .. }
        | Literal::Symbol { span, .. }
        | Literal::Boolean { span, .. }
        | Literal::Nil { span }
        | Literal::Pattern { span, .. } => *span,
    }
}

/// `comment` as the tree keeps it, at `anchor`.
fn attached(
    comment: Comment,
    anchor: Anchor,
    placement: Placement,
    source: &str,
) -> AttachedComment {
    let start = comment.span.start as usize;
    let text = comment.span.text(source);
    let lines = lines_of(text)
        .into_iter()
        .map(|line| match comment.kind {
            CommentKind::Line => {
                let line = line.trim_start();
                line.strip_prefix("///")
                    .unwrap_or(line)
                    .trim_end()
                    .to_string()
            }
            CommentKind::Block => line.trim_end().to_string(),
        })
        .collect();
    let line_start = source[..start]
        .rfind(['\n', '\r'])
        .map_or(0, |index| index + 1);
    let column = match comment.kind {
        CommentKind::Line => 0,
        CommentKind::Block => {
            u32::try_from(source[line_start..start].chars().count()).unwrap_or(u32::MAX)
        }
    };
    AttachedComment {
        kind: comment.kind,
        lines,
        column,
        anchor,
        placement,
        span: comment.span,
    }
}

/// Where the comment at `index` of `comments` stands on its line. The
/// comments beside it on the line count as nothing, so that comments
/// written side by side on their own line are all `OwnLine`, and side by
/// side after code all `EndOfLine`.
fn placement(comments: &[Comment], index: usize, source: &str) -> Placement {
    let on_one_line = |gap: &str| !gap.contains(['\n', '\r']) && gap.trim().is_empty();
    let mut run_start = comments[index].span.start as usize;
    for previous in comments[..index].iter().rev() {
        match source.get(previous.span.end as usize..run_start) {
            Some(gap) if on_one_line(gap) => run_start = previous.span.start as usize,
            _ => break,
        }
    }
    let mut run_end = comments[index].span.end as usize;
    for next in &comments[index + 1..] {
        match source.get(run_end..next.span.start as usize) {
            Some(gap) if on_one_line(gap) => run_end = next.span.end as usize,
            _ => break,
        }
    }
    if starts_its_line(source, run_start) {
        Placement::OwnLine
    } else if source[run_end..]
        .chars()
        .take_while(|character| *character != '\n' && *character != '\r')
        .all(char::is_whitespace)
    {
        Placement::EndOfLine
    } else {
        Placement::Inline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Every `.ns` file under `directory`, at any depth.
    fn sources_under(directory: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources_under(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "ns") {
                found.push(path);
            }
        }
    }

    fn every_source() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = Vec::new();
        for directory in [
            "corpus/modules",
            "goldens",
            "benchmarks",
            "crates/ns-runtime/core-library",
        ] {
            sources_under(&root.join(directory), &mut found);
        }
        found.sort();
        assert!(found.len() > 300, "too few sources: {}", found.len());
        found
    }

    /// Over every module the repository holds: a node's children lie
    /// inside it and apart from each other, and every comment but those a
    /// pattern literal holds is attached exactly once.
    #[test]
    fn every_comment_of_every_module_is_attached_once_to_a_tree_of_disjoint_extents() {
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

    #[test]
    fn comments_side_by_side_share_their_placement() {
        let source = "class A = () (\n  (* a *) (* b *)\n  m = ( ^1 (* c *) (* d *) )\n)";
        let module = crate::parse(source).expect("parse");
        let method = module.root_class.instance_side.methods().next().expect("m");
        let placements: Vec<Placement> = method
            .comments
            .iter()
            .chain(&method.body.comments)
            .map(|comment| comment.placement)
            .collect();
        assert_eq!(
            placements,
            [
                Placement::OwnLine,
                Placement::OwnLine,
                Placement::Inline,
                Placement::Inline
            ]
        );
    }

    /// The period after an initializer expression goes with it, so a
    /// comment dangling after the last one counts the same with or
    /// without it.
    #[test]
    fn a_period_after_an_element_is_not_counted() {
        for source in [
            "class A = ( x.\n  (* c *)\n) ()",
            "class A = ( x\n  (* c *)\n) ()",
        ] {
            let module = crate::parse(source).expect("parse");
            let side = &module.root_class.instance_side;
            assert_eq!(side.comments[0].anchor, Anchor::Inner(1), "{source}");
        }
    }

    #[test]
    fn a_line_block_keeps_its_text_after_the_markers() {
        let source = "class A = () (\n    ///  two spaces  \n    ///\n    m = ()\n)";
        let module = crate::parse(source).expect("parse");
        let method = module.root_class.instance_side.methods().next().expect("m");
        assert_eq!(method.comments[0].lines, ["  two spaces", ""]);
        assert_eq!(method.comments[0].column, 0);
    }

    #[test]
    fn a_crlf_block_comment_has_no_carriage_return() {
        let source = "class A = () (\r\n  (* one\r\n     two *)\r\n  m = ()\r\n)";
        let module = crate::parse(source).expect("parse");
        let method = module.root_class.instance_side.methods().next().expect("m");
        assert_eq!(method.comments[0].lines, ["(* one", "     two *)"]);
        assert_eq!(method.comments[0].placement, Placement::OwnLine);
    }

    #[test]
    fn a_comment_after_an_empty_class_side_attaches_as_without_it() {
        let written = crate::parse("class A = () () : ( ) (* end *)").expect("parse");
        let printed = crate::parse("class A = () () (* end *)").expect("parse");
        assert_eq!(written.root_class.comments[0].anchor, Anchor::After);
        assert_eq!(written.without_positions(), printed.without_positions());
    }

    #[test]
    fn a_comment_before_empty_bars_attaches_as_without_them() {
        let written = crate::parse("class A = ( (* c *) | | x. ) ()").expect("parse");
        let statement = &written.root_class.instance_side.initializer_expressions[0];
        assert_eq!(statement.comments.len(), 1);
        assert_eq!(statement.comments[0].anchor, Anchor::Before);
    }

    #[test]
    fn a_dangling_comment_in_a_method_body_is_inner_on_the_method() {
        let module = crate::parse("class A = () ( m = ( ^1.\n  (* done *)\n) )").expect("parse");
        let method = module.root_class.instance_side.methods().next().expect("m");
        // `m`, `=` and `(` stand before it, the body between.
        assert_eq!(method.comments[0].anchor, Anchor::Inner(3));
        assert!(method.body.comments.is_empty());
    }

    /// The expression the first method of `source`'s root class returns.
    fn returned(module: &Module) -> &Expression {
        let method = module.root_class.instance_side.methods().next().expect("m");
        let StatementKind::Return(value) = &method.body.statements[0].kind else {
            panic!("a return");
        };
        value
    }

    #[test]
    fn a_comment_between_cascade_messages_leads_the_next() {
        let module = crate::parse(
            "class A = () ( m = ( ^x foo;
  (* then *)
  bar ) )",
        )
        .expect("parse");
        let ExpressionKind::Cascade(cascade) = &returned(&module).kind else {
            panic!("a cascade");
        };
        assert_eq!(cascade.rest[0].comments[0].anchor, Anchor::Before);
    }

    #[test]
    fn a_comment_before_an_eventual_message_trails_the_receiver() {
        let module =
            crate::parse("class A = () ( m = ( ^x (* later *) <-: foo ) )").expect("parse");
        let ExpressionKind::Send(send) = &returned(&module).kind else {
            panic!("a send");
        };
        assert_eq!(send.receiver.comments[0].anchor, Anchor::After);
        assert!(send.message.comments.is_empty());
    }

    #[test]
    fn a_comment_after_the_eventual_operator_is_inner_on_the_message() {
        let module =
            crate::parse("class A = () ( m = ( ^x <-: (* later *) foo ) )").expect("parse");
        let ExpressionKind::Send(send) = &returned(&module).kind else {
            panic!("a send");
        };
        assert_eq!(send.message.comments[0].anchor, Anchor::Inner(1));
    }
}
