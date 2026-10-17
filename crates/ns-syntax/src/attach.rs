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
//!   `||`, ‹each header slot› and its `.`, its closing bar, ‹each
//!   initializer expression› (a `Statement`) and its `.`, `)`, `(`, ‹each
//!   member› (a lazy slot with its `.`), `)`. Class side: `(` ‹each
//!   method› `)`.
//! - `SlotDeclaration` (a header slot, a temporary or a lazy slot): the
//!   access modifier and `lazy` as written, the name, the type
//!   annotation's tokens, `=` or `::=` ‹the initializer›. The `.` after
//!   the slot is its container's.
//! - `MethodDeclaration`: the access modifier; the pattern's tokens -- a
//!   unary selector and the type tokens after it, a binary selector, its
//!   parameter's name and type tokens, or each keyword, its parameter's
//!   name and type tokens; `^` and the return type's tokens; the `where`
//!   clause's tokens; `=`, `(` ‹the code body› and the `.` after its last
//!   statement, `)`.
//! - `CodeBody`: the temporaries' opening `|` or `||`, ‹each temporary›
//!   and its `.`, their closing bar, ‹each statement› and the `.` between
//!   it and the next. The `.` after the last statement is the owner's (the
//!   method or the block), and goes with the body.
//! - `Statement`: `^` for a return; ‹the expression›.
//! - `Expression`: each parenthesis's `(`, outermost first; the shape's own
//!   tokens -- an identifier's token, a literal's token (a pattern literal:
//!   as many as `crate::lex` answers for its `text`, the end of input not
//!   counted -- a run of `>` is one token there as in the parser), a setter
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
//!   parameters' `|`; ‹the code body› and the `.` after its last
//!   statement; `]`.
//! - `SuperclassClause`: `self`, `super`, or `outer` and the name after
//!   it; the receiver's name and the type argument tokens after it; ‹the
//!   message›.
//!
//! A `.` right after a child goes with that child: it is an own token of
//! the node around it, but one `Inner(n)` does not count, and as `P` below
//! it stands for the child. A cascade's `;` right after a message stands
//! for that message as `P` too, though `Inner(n)` counts it: a reprint
//! writes each `;` right after its message, so a comment beside it, before
//! or after, trails that message wherever the reprint puts the next. A reprint writes the period after a statement,
//! a slot, an initializer expression or a tuple's element by rules of its
//! own, so whether the source had one, and whether a comment stands
//! before or after it, must move no comment: `^1 (* c *).` and
//! `^1. (* c *)` both trail the code body.
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
//! starting after it; a `.` or a cascade's `;` right after a child stands
//! for that child as `P`.
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

#[cfg(test)]
mod reference;

/// Attaches each of `comments` (in source order, as the lexer answers
/// them) to its node of `module`, parsed from `source` into `tokens`.
pub(crate) fn attach_comments(
    module: &mut Module,
    tokens: &[Token],
    comments: &[Comment],
    source: &str,
) {
    attach(module, tokens, comments, source);
}

/// [`attach_comments`], answering the work it did: the nodes its walk
/// visited, the entries it measured, the nodes it expanded, the children
/// those had, each comment once per expansion it is carried into, and the
/// nodes that only received a comment beside them, with the children
/// describing them appended. Only a node a comment lies in is expanded, and
/// the walk and the loop over its records are linear, so a module of many
/// nodes and few comments costs one walk and one loop; a test holds the
/// work to growing linearly with the source.
fn attach(module: &mut Module, tokens: &[Token], comments: &[Comment], source: &str) -> usize {
    if comments.is_empty() {
        return 0;
    }
    let mut work = 0;
    let mut parts = Parts::new(tokens, comments);
    let records = prepass(module, &mut parts, &mut work);
    let (dropped, patterns) = parts.sorted();
    let kept = Tokens::kept(tokens, &dropped);
    let entries = measure(&records, &kept, &mut work);
    let placed = (0..comments.len())
        .filter(|index| !inside_a_pattern(&patterns, comments[*index].span))
        .collect();
    let context = Context {
        tokens: &kept,
        entries: &entries,
        comments,
        source,
    };
    descend(module, &context, placed, &mut work);
    work
}

/// A commentable node not yet described: the module or one of the nodes
/// the module documentation lists.
enum Pending<'tree> {
    Module(&'tree mut Module),
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

/// What the extent of a node takes beyond its known span and its
/// children.
#[derive(Clone, Copy)]
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

/// A node as [`describe`] answers it.
struct Description<'tree> {
    comments: &'tree mut Vec<AttachedComment>,
    shape: Shape,
    /// The bytes some of the node's own tokens are known to span.
    known: Option<Span>,
}

/// What the prepass collects besides counting: what decides which tokens
/// a comment is counted among, and which comments no node holds.
struct Parts<'input> {
    tokens: &'input [Token],
    comments: &'input [Comment],
    /// The tokens of the empty parts.
    dropped: Vec<Span>,
    /// The spans of the pattern literals.
    patterns: Vec<Span>,
}

impl<'input> Parts<'input> {
    fn new(tokens: &'input [Token], comments: &'input [Comment]) -> Parts<'input> {
        Parts {
            tokens,
            comments,
            dropped: Vec::new(),
            patterns: Vec::new(),
        }
    }

    /// The dropped tokens and the pattern literals, each by start.
    fn sorted(self) -> (Vec<Span>, Vec<Span>) {
        let Parts {
            mut dropped,
            mut patterns,
            ..
        } = self;
        dropped.sort_by_key(|span| span.start);
        patterns.sort_by_key(|span| span.start);
        (dropped, patterns)
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

/// Describes `node` and appends its children to `children` in the order
/// the prepass numbers them (the last appended is numbered first); given
/// `parts`, records the empty parts and the pattern literal the node
/// carries. The one place that says what each kind of node is made of:
/// the prepass and the descent both read it, so that the descent finds a
/// child at the index the prepass gave it.
fn describe<'tree>(
    node: Pending<'tree>,
    children: &mut Vec<Pending<'tree>>,
    parts: Option<&mut Parts<'_>>,
) -> Description<'tree> {
    match node {
        Pending::Module(module) => {
            let Module {
                root_class,
                comments,
                ..
            } = module;
            children.push(Pending::Class(root_class));
            Description {
                comments,
                shape: Shape::Module,
                known: None,
            }
        }
        Pending::Class(class) => describe_class(class, children, parts),
        Pending::Side(side) => describe_side(side, children, parts),
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
            children.extend(messages.iter_mut().map(Pending::Message));
            plain(comments, known)
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
            if let Some(initializer) = initializer {
                children.push(Pending::Expression(initializer));
            }
            plain(comments, known)
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
            children.push(Pending::Body(body));
            plain(comments, known)
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
                if let Some(parts) = parts {
                    parts.drop_if_empty(*temporaries_delimiters);
                }
            }
            let mut known = None;
            widen_delimiters(&mut known, *temporaries_delimiters);
            children.extend(temporaries.iter_mut().map(Pending::Slot));
            children.extend(statements.iter_mut().map(Pending::Statement));
            plain(comments, known)
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
            children.push(Pending::Expression(expression));
            plain(comments, known)
        }
        Pending::Expression(expression) => describe_expression(expression, children, parts),
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
            children.extend(arguments.iter_mut().map(Pending::Expression));
            Description {
                comments,
                shape,
                known,
            }
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
            children.push(Pending::Body(body));
            plain(comments, known)
        }
    }
}

fn plain(comments: &mut Vec<AttachedComment>, known: Option<Span>) -> Description<'_> {
    Description {
        comments,
        shape: Shape::Plain,
        known,
    }
}

fn describe_class<'tree>(
    class: &'tree mut ClassDeclaration,
    children: &mut Vec<Pending<'tree>>,
    parts: Option<&mut Parts<'_>>,
) -> Description<'tree> {
    let ClassDeclaration {
        superclass,
        mixin_applications,
        instance_side,
        class_side,
        comments,
        span,
        ..
    } = class;
    if let (Some(parts), Some(side)) = (parts, class_side.as_ref()) {
        if side.members.is_empty() && parts.drop_if_empty(side.delimiters) {
            // The `:` before the side's `(`.
            if let Some(Delimiters { open, .. }) = side.delimiters {
                let open_index = parts
                    .tokens
                    .partition_point(|token| token.span.start < open.start);
                if let Some(colon) = open_index.checked_sub(1).map(|index| parts.tokens[index]) {
                    if colon.kind == TokenKind::Colon {
                        parts.dropped.push(colon.span);
                    }
                }
            }
        }
    }
    let mut known = None;
    widen(&mut known, *span);
    if let Some(clause) = superclass {
        children.push(Pending::Superclass(clause));
    }
    children.extend(mixin_applications.iter_mut().map(Pending::Superclass));
    children.push(Pending::Side(instance_side));
    if let Some(side) = class_side {
        children.push(Pending::Side(side));
    }
    plain(comments, known)
}

fn describe_side<'tree>(
    side: &'tree mut ClassBody,
    children: &mut Vec<Pending<'tree>>,
    parts: Option<&mut Parts<'_>>,
) -> Description<'tree> {
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
        if let Some(parts) = parts {
            parts.drop_if_empty(*slot_delimiters);
        }
    }
    let mut known = None;
    widen_delimiters(&mut known, *delimiters);
    widen_delimiters(&mut known, *slot_delimiters);
    children.extend(header_slots.iter_mut().map(Pending::Slot));
    children.extend(initializer_expressions.iter_mut().map(Pending::Statement));
    children.extend(members.iter_mut().map(|member| match member {
        Member::Class(class) => Pending::Class(class),
        Member::LazySlot(slot) => Pending::Slot(slot),
        Member::Method(method) => Pending::Method(method),
    }));
    plain(comments, known)
}

fn describe_expression<'tree>(
    expression: &'tree mut Expression,
    children: &mut Vec<Pending<'tree>>,
    parts: Option<&mut Parts<'_>>,
) -> Description<'tree> {
    let Expression {
        kind,
        parentheses,
        type_arguments,
        synthesized,
        comments,
        span,
    } = expression;
    if *synthesized {
        return Description {
            comments,
            shape: Shape::Empty,
            known: None,
        };
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
                if let Some(parts) = parts {
                    parts.patterns.push(span);
                }
            }
        }
        ExpressionKind::Assign { target, .. } => widen(&mut known, target.span),
        ExpressionKind::DynamicArray { span, .. } => widen(&mut known, *span),
        ExpressionKind::Send(_) | ExpressionKind::Cascade(_) | ExpressionKind::Block(_) => {}
    }
    match kind {
        ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
        ExpressionKind::Send(send) => {
            let Send {
                receiver, message, ..
            } = &mut **send;
            children.push(Pending::Expression(receiver));
            children.push(Pending::Message(message));
        }
        ExpressionKind::Cascade(cascade) => {
            let Cascade {
                receiver,
                first,
                rest,
                ..
            } = &mut **cascade;
            children.push(Pending::Expression(receiver));
            children.push(Pending::Message(first));
            children.extend(rest.iter_mut().map(Pending::Message));
        }
        ExpressionKind::Assign { value, .. } => children.push(Pending::Expression(value)),
        ExpressionKind::Block(block) => children.push(Pending::Block(block)),
        ExpressionKind::DynamicArray { elements, .. } => {
            children.extend(elements.iter_mut().map(Pending::Expression));
        }
    }
    plain(comments, known)
}

/// What [`measure`] needs of a commentable node, recorded by the prepass in
/// the order of the entries.
#[derive(Clone, Copy)]
struct Record {
    known: Option<Span>,
    shape: Shape,
    /// The index of the node's parent, [`NO_PARENT`] for the module.
    parent: usize,
}

/// Records every commentable node, and the empty parts and the pattern
/// literals: the tokens a comment is counted among depend on the empty
/// parts, so the extents are measured once the walk is over, from what it
/// recorded rather than by a second walk.
fn prepass(module: &mut Module, parts: &mut Parts<'_>, work: &mut usize) -> Vec<Record> {
    // A module has about as many commentable nodes as tokens; reserving
    // that spares the records' growth its copies.
    let mut records = Vec::with_capacity(parts.tokens.len());
    let mut stack = vec![(Pending::Module(module), NO_PARENT)];
    let mut children = Vec::new();
    while let Some((node, parent)) = stack.pop() {
        let index = records.len();
        let Description { shape, known, .. } = describe(node, &mut children, Some(&mut *parts));
        records.push(Record {
            known,
            shape,
            parent,
        });
        stack.extend(children.drain(..).map(|child| (child, index)));
    }
    *work += records.len();
    records
}

/// A commentable node's extent and the size of its subtree, itself
/// included. The entries stand in the order the prepass visits the nodes --
/// a node, then its children's subtrees, the last appended child's first --
/// so that a node's children follow from its index and their sizes.
#[derive(Clone, Copy)]
struct Entry {
    first: u32,
    last: u32,
    size: u32,
}

/// The `first` of a node without a token. No token has this index: a
/// source is at most `u32::MAX` bytes and a kept token at least one.
const NO_TOKEN: u32 = u32::MAX;

/// The parent of the module.
const NO_PARENT: usize = usize::MAX;

impl Entry {
    const EMPTY: Entry = Entry {
        first: NO_TOKEN,
        last: NO_TOKEN,
        size: 0,
    };

    fn extent(self) -> Option<(usize, usize)> {
        (self.first != NO_TOKEN).then_some((self.first as usize, self.last as usize))
    }

    fn widen(&mut self, extent: (usize, usize)) {
        let (first, last) = self.extent().map_or(extent, |known| merge(known, extent));
        self.first = index_word(first);
        self.last = index_word(last);
    }
}

/// `index` as an entry stores it; every token index fits (see
/// [`NO_TOKEN`]).
fn index_word(index: usize) -> u32 {
    debug_assert!(index < NO_TOKEN as usize);
    u32::try_from(index).unwrap_or(NO_TOKEN - 1)
}

/// Every commentable node's extent, by the rules the module documentation
/// gives, from the prepass's `records`: one loop from the last index down.
/// A node's descendants follow it in the entries, so its children are
/// finished when it is reached, each having merged its extent and added its
/// size into the node's entry.
fn measure(records: &[Record], tokens: &Tokens, work: &mut usize) -> Vec<Entry> {
    let mut entries = vec![
        Entry {
            size: 1,
            ..Entry::EMPTY
        };
        records.len()
    ];
    let mut cursors = Cursors::default();
    for (index, record) in records.iter().enumerate().rev() {
        let children_extent = entries[index].extent();
        let mut entry = Entry {
            size: entries[index].size,
            ..Entry::EMPTY
        };
        let extent = extent_of(
            tokens,
            &mut cursors,
            record.known,
            record.shape,
            children_extent,
        );
        if let Some(extent) = extent {
            entry.widen(extent);
        }
        entries[index] = entry;
        if record.parent != NO_PARENT {
            let parent = &mut entries[record.parent];
            parent.size += entry.size;
            if let Some(extent) = extent {
                parent.widen(extent);
            }
        }
    }
    debug_assert_eq!(
        entries.first().map_or(0, |module| module.size as usize),
        records.len()
    );
    *work += entries.len();
    entries
}

/// Where each of [`extent_of`]'s searches found its token last. The loop
/// takes the nodes in roughly descending source order, so the next answer
/// lies near the last one, and a search from there costs the logarithm of
/// the distance rather than of the module's tokens.
#[derive(Default)]
struct Cursors {
    first: usize,
    past: usize,
    lead: usize,
}

/// `slice.partition_point(predicate)`, searched from `from` outwards: by
/// steps of 1, 2, 4, ... towards the answer, then by halves within the last
/// step. Like `partition_point`, it requires `slice` partitioned by
/// `predicate`.
fn partition_point_near<Item>(
    slice: &[Item],
    from: usize,
    predicate: impl Fn(&Item) -> bool,
) -> usize {
    let from = from.min(slice.len());
    let (low, high) = if slice.get(from).is_some_and(&predicate) {
        // The answer lies after `from`.
        let mut low = from + 1;
        let mut step = 1;
        loop {
            let probe = low + step - 1;
            match slice.get(probe) {
                Some(item) if predicate(item) => {
                    low = probe + 1;
                    step *= 2;
                }
                Some(_) => break (low, probe),
                None => break (low, slice.len()),
            }
        }
    } else {
        // The answer lies at `from` or before it.
        let mut high = from;
        let mut step = 1;
        loop {
            let Some(probe) = high.checked_sub(step) else {
                break (0, high);
            };
            if predicate(&slice[probe]) {
                break (probe + 1, high);
            }
            high = probe;
            step *= 2;
        }
    };
    low + slice[low..high].partition_point(predicate)
}

/// A node's extent from its known span, its shape and its children's
/// merged extent.
fn extent_of(
    tokens: &Tokens,
    cursors: &mut Cursors,
    known: Option<Span>,
    shape: Shape,
    children: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    let mut extent = known
        .and_then(|known| tokens.covering_near(known, cursors))
        .into_iter()
        .chain(children)
        .reduce(merge);
    match shape {
        Shape::Module => extent = tokens.len().checked_sub(1).map(|last| (0, last)),
        Shape::Plain => {}
        Shape::Empty => extent = None,
        Shape::Message {
            selector,
            eventual,
            end,
        } => {
            let lead = match selector {
                SelectorShape::Unary => {
                    cursors.lead =
                        partition_point_near(&tokens.starts, cursors.lead, |start| *start < end);
                    cursors.lead.checked_sub(1)
                }
                SelectorShape::Binary | SelectorShape::Keyword => {
                    children.and_then(|(first, _)| first.checked_sub(1))
                }
            };
            if let Some(lead) = lead {
                let first = if eventual {
                    lead.saturating_sub(1)
                } else {
                    lead
                };
                extent = Some(extent.map_or((first, lead), |extent| merge(extent, (first, lead))));
            }
        }
    }
    extent
}

/// Whether `span` lies inside one of `patterns` (sorted by start): such a
/// comment is part of the literal's text and attached nowhere.
fn inside_a_pattern(patterns: &[Span], span: Span) -> bool {
    let before = patterns.partition_point(|pattern| pattern.start <= span.start);
    before
        .checked_sub(1)
        .is_some_and(|index| span.end <= patterns[index].end)
}

/// What the descent reads.
struct Context<'input> {
    tokens: &'input Tokens,
    entries: &'input [Entry],
    comments: &'input [Comment],
    source: &'input str,
}

/// A node to expand, the comments inside it, and the comments its parent
/// placed before and after it -- kept with it so that its `comments` end
/// up in source order: before, its own, after.
struct Expansion<'tree> {
    node: Pending<'tree>,
    index: usize,
    inside: Vec<usize>,
    before: Vec<usize>,
    after: Vec<usize>,
}

/// A child with a token, as its parent's expansion sees it: its extent,
/// its entry's index and its position among the children `describe`
/// appended.
#[derive(Clone, Copy)]
struct Child {
    first: usize,
    last: usize,
    index: usize,
    position: usize,
}

/// What a holder's expansion decided for one child.
#[derive(Default)]
struct Placed {
    inside: Vec<usize>,
    before: Vec<usize>,
    after: Vec<usize>,
}

/// Where one comment goes, as its holder's expansion decides it; a slot
/// is a child's index into the holder's sorted children.
enum Target {
    Inside(usize),
    Before(usize),
    After(usize),
    Holder(Anchor),
}

#[derive(Clone, Copy)]
enum Neighbour {
    Child(usize),
    Token(usize),
}

/// Places every comment of `placed` from the module down, expanding a node
/// only when a comment lies inside it.
fn descend(module: &mut Module, context: &Context<'_>, placed: Vec<usize>, work: &mut usize) {
    let mut expansions = vec![Expansion {
        node: Pending::Module(module),
        index: 0,
        inside: placed,
        before: Vec::new(),
        after: Vec::new(),
    }];
    let mut scratch = Vec::new();
    while let Some(Expansion {
        node,
        index,
        inside,
        before,
        after,
    }) = expansions.pop()
    {
        let mut declared = Vec::new();
        let description = describe(node, &mut declared, None);
        let children = context.children(index, declared.len());
        *work += 1 + declared.len() + inside.len();
        let mut decided: Vec<Placed> = children.iter().map(|_| Placed::default()).collect();
        let mut own_tokens = None;
        context.push(description.comments, &before, Anchor::Before);
        for comment in inside {
            match context.place(index, &children, comment, &mut own_tokens) {
                Target::Inside(slot) => decided[slot].inside.push(comment),
                Target::Before(slot) => decided[slot].before.push(comment),
                Target::After(slot) => decided[slot].after.push(comment),
                Target::Holder(anchor) => context.push(description.comments, &[comment], anchor),
            }
        }
        context.push(description.comments, &after, Anchor::After);
        let mut declared: Vec<Option<Pending<'_>>> = declared.into_iter().map(Some).collect();
        for (child, decided) in children.iter().zip(decided) {
            let Placed {
                inside,
                before,
                after,
            } = decided;
            if inside.is_empty() && before.is_empty() && after.is_empty() {
                continue;
            }
            let Some(node) = declared[child.position].take() else {
                continue;
            };
            if inside.is_empty() {
                let description = describe(node, &mut scratch, None);
                *work += 1 + scratch.len();
                scratch.clear();
                context.push(description.comments, &before, Anchor::Before);
                context.push(description.comments, &after, Anchor::After);
            } else {
                expansions.push(Expansion {
                    node,
                    index: child.index,
                    inside,
                    before,
                    after,
                });
            }
        }
    }
}

/// The entry indices of the `count` children of the node at `index`, in
/// the order `describe` appends them.
fn child_indices(entries: &[Entry], index: usize, count: usize) -> Vec<usize> {
    let mut indices = vec![0; count];
    let mut next = index + 1;
    for position in (0..count).rev() {
        indices[position] = next;
        next += entries[next].size as usize;
    }
    debug_assert_eq!(next, index + entries[index].size as usize);
    indices
}

impl Context<'_> {
    /// The children of the node at `index` that have a token, by first
    /// token. Listed last-appended first before the stable sort, as the
    /// reference lists them, so that children a recovery made to start at
    /// the same token keep the reference's order.
    fn children(&self, index: usize, count: usize) -> Vec<Child> {
        let indices = child_indices(self.entries, index, count);
        let mut children: Vec<Child> = (0..count)
            .rev()
            .filter_map(|position| {
                let index = indices[position];
                self.entries[index].extent().map(|(first, last)| Child {
                    first,
                    last,
                    index,
                    position,
                })
            })
            .collect();
        children.sort_by_key(|child| child.first);
        children
    }

    /// Where the comment at `comment` goes among the holder at `holder`
    /// and its `children`, by rules 1 to 4.
    fn place(
        &self,
        holder: usize,
        children: &[Child],
        comment: usize,
        own_tokens: &mut Option<OwnTokens>,
    ) -> Target {
        let span = self.comments[comment].span;
        let tokens = self.tokens;
        // The comment stands before token `gap`, after token `gap - 1`.
        let gap = tokens.starts.partition_point(|start| *start < span.start);
        let around = children.partition_point(|child| child.first < gap);
        if let Some(slot) = around.checked_sub(1) {
            if gap <= children[slot].last {
                return Target::Inside(slot);
            }
        }
        let ending_at = |token: usize| {
            children
                .partition_point(|child| child.first <= token)
                .checked_sub(1)
                .filter(|slot| children[*slot].last == token)
        };
        let previous = gap.checked_sub(1).map(|index| match ending_at(index) {
            Some(slot) => Neighbour::Child(slot),
            None if matches!(tokens.kinds[index], TokenKind::Dot | TokenKind::Semicolon) => index
                .checked_sub(1)
                .and_then(ending_at)
                .map_or(Neighbour::Token(index), Neighbour::Child),
            None => Neighbour::Token(index),
        });
        let next = (gap < tokens.len()).then(|| match children.get(around) {
            Some(child) if child.first == gap => Neighbour::Child(around),
            _ => Neighbour::Token(gap),
        });
        let line_break_before = gap.checked_sub(1).is_some_and(|index| {
            self.source
                .get(tokens.ends[index] as usize..span.start as usize)
                .is_some_and(|between| between.contains(['\n', '\r']))
        });
        let next_closes = matches!(
            next,
            Some(Neighbour::Token(index)) if closes(tokens.kinds[index])
        );
        match (previous, next) {
            (Some(Neighbour::Child(slot)), _) if !line_break_before => Target::After(slot),
            (_, Some(Neighbour::Child(slot))) => Target::Before(slot),
            (Some(Neighbour::Child(slot)), _) if !next_closes => Target::After(slot),
            _ => {
                let own_tokens =
                    own_tokens.get_or_insert_with(|| OwnTokens::new(self, holder, children));
                Target::Holder(Anchor::Inner(own_tokens.before(gap)))
            }
        }
    }

    /// Pushes each of `comments` onto `target` at `anchor`.
    fn push(&self, target: &mut Vec<AttachedComment>, comments: &[usize], anchor: Anchor) {
        for comment in comments {
            target.push(attached(
                self.comments[*comment],
                anchor,
                placement(self.comments, *comment, self.source),
                self.source,
            ));
        }
    }
}

/// How many of a holder's own tokens stand before a gap: its tokens from
/// its first on, less each child's that ends before the gap and the `.`
/// after such a child when the `.` too stands before the gap. Built once
/// per holder with an `Inner` comment, so that counting for each of them
/// is two searches, not a walk over every child: a holder of many children
/// with a comment between each two would otherwise cost their product.
struct OwnTokens {
    first: usize,
    /// The children's last tokens, ascending.
    lasts: Vec<usize>,
    /// `tokens[count]`: the tokens of the first `count` children by last.
    tokens: Vec<usize>,
    /// `periods[count]`: how many of the first `count` have a `.` after.
    periods: Vec<usize>,
}

impl OwnTokens {
    fn new(context: &Context<'_>, holder: usize, children: &[Child]) -> OwnTokens {
        let mut by_last = children.to_vec();
        by_last.sort_by_key(|child| child.last);
        let mut tokens = vec![0];
        let mut periods = vec![0];
        for child in &by_last {
            let period = context.tokens.kinds.get(child.last + 1) == Some(&TokenKind::Dot);
            tokens.push(tokens[tokens.len() - 1] + child.last - child.first + 1);
            periods.push(periods[periods.len() - 1] + usize::from(period));
        }
        OwnTokens {
            first: context.entries[holder]
                .extent()
                .map_or(0, |(first, _)| first),
            lasts: by_last.iter().map(|child| child.last).collect(),
            tokens,
            periods,
        }
    }

    fn before(&self, gap: usize) -> u32 {
        let ended = self.lasts.partition_point(|last| *last < gap);
        // A child ending right before the gap has its `.`, if any, at the
        // gap or after it: that `.` is not before the comment.
        let ended_earlier = self.lasts.partition_point(|last| last + 1 < gap);
        let not_counted = self.tokens[ended] + self.periods[ended_earlier];
        let own = gap.saturating_sub(self.first).saturating_sub(not_counted);
        u32::try_from(own).unwrap_or(u32::MAX)
    }
}

/// The tokens a comment is placed among: the parser's, without the end of
/// input and without an empty part's.
struct Tokens {
    starts: Vec<u32>,
    ends: Vec<u32>,
    kinds: Vec<TokenKind>,
}

impl Tokens {
    /// The parser's `tokens` without the end of input and without those
    /// `dropped` (sorted by start) holds.
    fn kept(tokens: &[Token], dropped: &[Span]) -> Tokens {
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
        kept
    }

    fn len(&self) -> usize {
        self.starts.len()
    }

    /// The first and the last token overlapping `span`, as the reference
    /// measures them.
    #[cfg(test)]
    fn covering(&self, span: Span) -> Option<(usize, usize)> {
        if span.start >= span.end {
            return None;
        }
        let first = self.ends.partition_point(|end| *end <= span.start);
        let past = self.starts.partition_point(|start| *start < span.end);
        (first < past).then(|| (first, past - 1))
    }

    /// [`Tokens::covering`], each search started where `cursors` left it.
    fn covering_near(&self, span: Span, cursors: &mut Cursors) -> Option<(usize, usize)> {
        if span.start >= span.end {
            return None;
        }
        cursors.first = partition_point_near(&self.ends, cursors.first, |end| *end <= span.start);
        cursors.past = partition_point_near(&self.starts, cursors.past, |start| *start < span.end);
        (cursors.first < cursors.past).then(|| (cursors.first, cursors.past - 1))
    }
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

    pub(super) fn every_source() -> Vec<PathBuf> {
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

    /// `source` lexed and parsed as `parse_for_editor` does, nothing
    /// attached: the module, the parser's tokens and the comments.
    pub(super) fn unattached(source: &str) -> (Module, Vec<Token>, Vec<Comment>) {
        let lexed = crate::lexer::Lexer::new(source).run_keeping_comments();
        let mut parser = crate::parser::Parser::new(source, lexed.tokens);
        let module = parser.parse_module();
        let (tokens, _) = parser.into_parts();
        (module, tokens, lexed.comments)
    }

    /// The module `source` parses to, attached by `attach_comments` and by
    /// the reference: equal, or the name of the source and where it was cut.
    fn attaches_as_the_reference(source: &str, name: &str) {
        let (module, tokens, comments) = unattached(source);
        let mut expected = module.clone();
        super::reference::attach_comments(&mut expected, &tokens, &comments, source);
        let mut actual = module;
        attach_comments(&mut actual, &tokens, &comments, source);
        assert!(actual == expected, "{name}: attached unlike the reference");
    }

    /// Every module the repository holds attaches as the reference
    /// attaches it.
    #[test]
    fn the_attachment_attaches_as_the_reference_over_every_source() {
        for path in every_source() {
            let source = std::fs::read_to_string(&path).expect("read");
            attaches_as_the_reference(&source, &path.display().to_string());
        }
    }

    /// Every module cut at fifteen points spread over its length, parsed
    /// as an editor parses it: the recovery's unfinished and overlapping
    /// nodes attach as the reference attaches them.
    #[test]
    fn the_attachment_attaches_as_the_reference_over_every_prefix() {
        const CUTS: usize = 16;
        for path in every_source() {
            let source = std::fs::read_to_string(&path).expect("read");
            for part in 1..CUTS {
                let mut cut = source.len() * part / CUTS;
                while !source.is_char_boundary(cut) {
                    cut -= 1;
                }
                let name = format!("{} cut at byte {cut}", path.display());
                attaches_as_the_reference(&source[..cut], &name);
            }
        }
    }

    /// The new walk measures the same extents as the reference's tree, as
    /// a multiset: the two number their nodes in different orders.
    #[test]
    fn the_walk_measures_the_extents_the_reference_measures() {
        for path in every_source() {
            let source = std::fs::read_to_string(&path).expect("read");
            let (mut module, tokens, comments) = unattached(&source);
            let mut expected = super::reference::extents(&mut module, &tokens, &comments);
            let mut actual = extents(&mut module, &tokens, &comments);
            expected.sort_unstable();
            actual.sort_unstable();
            assert!(actual == expected, "{}: extents differ", path.display());
        }
    }

    /// Every extent the walk measures, for the test above.
    fn extents(
        module: &mut Module,
        tokens: &[Token],
        comments: &[Comment],
    ) -> Vec<Option<(usize, usize)>> {
        let mut work = 0;
        let mut parts = Parts::new(tokens, comments);
        let records = prepass(module, &mut parts, &mut work);
        let kept = Tokens::kept(tokens, &parts.sorted().0);
        measure(&records, &kept, &mut work)
            .iter()
            .map(|entry| entry.extent())
            .collect()
    }

    /// The search from a starting point answers what `partition_point`
    /// answers, from every starting point, past the end included, and for
    /// every bound: below, between, at a run of duplicates and above.
    #[test]
    fn the_search_near_a_point_answers_as_partition_point() {
        let slices: [&[u32]; 6] = [
            &[],
            &[5],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
            &[2, 2, 2, 4, 4, 6, 8, 8, 8, 8, 10],
            &[7, 7, 7, 7, 7],
            &[0, 3, 3, 9, 20, 21, 22, 40, 41, 100],
        ];
        for slice in slices {
            for bound in 0..=101 {
                let predicate = |item: &u32| *item < bound;
                let expected = slice.partition_point(predicate);
                for from in 0..=slice.len() + 2 {
                    assert_eq!(
                        partition_point_near(slice, from, predicate),
                        expected,
                        "{slice:?}, below {bound}, from {from}"
                    );
                }
            }
        }
    }

    /// Over every module the repository holds: in the walk's entries, a
    /// node's children lie inside it and apart from each other.
    #[test]
    fn the_walk_measures_a_tree_of_disjoint_extents() {
        for path in every_source() {
            let source = std::fs::read_to_string(&path).expect("read");
            let name = path.display();
            let (mut module, tokens, comments) = unattached(&source);
            let mut work = 0;
            let mut parts = Parts::new(&tokens, &comments);
            let records = prepass(&mut module, &mut parts, &mut work);
            let kept = Tokens::kept(&tokens, &parts.sorted().0);
            let entries = measure(&records, &kept, &mut work);
            let mut stack = vec![(Pending::Module(&mut module), 0)];
            while let Some((node, index)) = stack.pop() {
                let mut declared = Vec::new();
                describe(node, &mut declared, None);
                let indices = child_indices(&entries, index, declared.len());
                let mut extents: Vec<(usize, usize)> = indices
                    .iter()
                    .filter_map(|child| entries[*child].extent())
                    .collect();
                extents.sort_unstable();
                let mut previous_last = None;
                for (first, last) in extents {
                    let (outer_first, outer_last) = entries[index].extent().expect("a parent");
                    assert!(
                        outer_first <= first && last <= outer_last,
                        "{name}: a child of node {index} lies outside it"
                    );
                    assert!(
                        previous_last.is_none_or(|previous| previous < first),
                        "{name}: the children of node {index} overlap"
                    );
                    previous_last = Some(last);
                }
                stack.extend(declared.into_iter().zip(indices));
            }
        }
    }

    /// Before a dangling comment the class body's own tokens are `(` --
    /// and the slot section's bars when it has slots -- each child's
    /// tokens and the `.` after it not counted.
    #[test]
    fn inner_counts_the_own_tokens_around_several_children() {
        for (source, expected) in [
            ("class A = ( x. y. z.\n  (* c *)\n) ()", Anchor::Inner(1)),
            (
                "class A = ( | a b | x. y.\n  (* c *)\n) ()",
                Anchor::Inner(3),
            ),
        ] {
            let module = crate::parse(source).expect("parse");
            let side = &module.root_class.instance_side;
            assert_eq!(side.comments[0].anchor, expected, "{source}");
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

    /// The work `attach` does for `source`, the attachment run on a
    /// 256 KiB stack: a walk that recursed along a chain would overflow it.
    fn work_for(source: &str) -> usize {
        const STACK_BYTES: usize = 256 * 1024;
        let (module, tokens, comments) = unattached(source);
        let source = source.to_string();
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(move || {
                let mut module = module;
                let work = attach(&mut module, &tokens, &comments, &source);
                (work, module)
            })
            .expect("spawn the attaching thread")
            .join()
            .expect("the attachment runs on a small stack")
            .0
    }

    /// `x (* c *) foo foo ...`: the comment lies as deep as the chain is
    /// long.
    fn chain(links: usize) -> String {
        format!(
            "class A = () ( m = ( ^x (* c *){} ) )",
            " foo".repeat(links)
        )
    }

    /// `x foo (* c *) ; bar (* c *) ; bar ...`, a comment before every `;`,
    /// each on a line of its own so that no comment has another beside it.
    fn cascade(messages: usize) -> String {
        format!(
            "class A = () ( m = ( ^x foo{} ) )",
            " (* c *)
  ; bar"
                .repeat(messages)
        )
    }

    fn assert_linear(short: usize, long: usize, what: &str) {
        assert!(long > 0 && short > 0, "{what}: no work counted");
        assert!(
            long <= 2 * short + short / 10,
            "{what}: twice the length took {long} against {short}"
        );
    }

    #[test]
    fn the_work_grows_linearly_along_a_chain() {
        let short = work_for(&chain(50_000));
        let long = work_for(&chain(100_000));
        assert!(short > 50_000, "every link is visited: {short}");
        assert_linear(short, long, "a chain");
    }

    #[test]
    fn the_work_grows_linearly_with_comments_among_many_children() {
        let short = work_for(&cascade(50_000));
        let long = work_for(&cascade(100_000));
        assert_linear(short, long, "a cascade");
    }

    /// A method with a comment before it, one among its pattern's tokens
    /// and one after it: the side's expansion places the first and the
    /// last, the method's own the middle one, and the three stay in
    /// source order.
    #[test]
    fn a_node_keeps_its_comments_in_source_order_across_two_expansions() {
        let source = indoc::indoc! {"
            class A = () (
              (* before *)
              at: index (* inner *) put: value = ( ^index ) (* after *)
            )
        "};
        let module = crate::parse(source).expect("parse");
        let method = module
            .root_class
            .instance_side
            .methods()
            .next()
            .expect("at:put:");
        let found: Vec<(&str, Anchor)> = method
            .comments
            .iter()
            .map(|comment| (comment.lines[0].as_str(), comment.anchor))
            .collect();
        assert_eq!(
            found,
            [
                ("(* before *)", Anchor::Before),
                ("(* inner *)", Anchor::Inner(2)),
                ("(* after *)", Anchor::After),
            ]
        );
    }
}
