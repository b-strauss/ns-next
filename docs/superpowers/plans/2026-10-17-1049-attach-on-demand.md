# Comment attachment on demand — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sub-project A of the syntax crate's optimization: `attach_comments` stops building a tree of every commentable node and expands only the nodes a comment lies in, with every `AttachedComment` of every source unchanged.

**Architecture:** Three linear steps over the module, all reading one per-kind description (`describe`): a prepass collecting the empty parts and pattern literals, an iterative post-order walk writing each node's token extent into one pre-order array of 12-byte entries, and a descent from the module that expands a node only when comments lie inside it. Today's algorithm moves unchanged into `attach/reference.rs` (`#[cfg(test)]`), permanently, and an oracle test holds the new one to it over every source and over prefixes of every source.

**Tech Stack:** Rust 1.98.0, `crates/ns-syntax`, `cargo-nextest`, the corpus benchmark (`cargo bench -p ns-syntax --bench corpus`), samply and `dumpbin` for the profile.

**Spec:** `docs/superpowers/specs/2026-10-17-attach-on-demand-design.md` (designed with the user on 2026-10-17). Read it and the module documentation of `crates/ns-syntax/src/attach.rs` (lines 1–118, the rules) before Task 1. Its decisions are settled; this plan does not reopen them.

## Global Constraints

- Work on the branch `perf/syntax-benchmarks` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree (Task 4 adds one under `.temp/` for the measurement only). It exists already.
- Every commit is green: `cargo xtask gates commit` runs before each commit (it formats, lints warning-free, runs the suite, the doctests, the corpus parse and the plugin's check). The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 4, by the controller (`finishing-a-batch` skill), since `ns-syntax` is part of "the runtime"; an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`refactor:`, `perf:`, `test:`, `docs:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"; clippy's `min_ident_chars`): `index`, `position`, `children`, `extent`, never `i`, `idx`, `pos`, `ch`.
- **Every `AttachedComment` stays what it is today** for every source, valid or recovered. A difference the oracle finds is a defect of the new algorithm, never a reason to change the reference or the rules.
- **A chain of messages is walked link by link** (`CLAUDE.md`): every walk over the module runs from an explicit stack; nothing in `attach.rs` recurses over the tree.
- **Never panic on a recovered tree**: no `unwrap`/`expect` on anything a source decides; indices into the entry array are an internal invariant, checked by `debug_assert!`.
- No new dependency. No `unsafe`.
- Comments explain the decision -- what a check prevents -- not the mechanics (`CLAUDE.md`, "Conventions").
- Nothing outside `crates/ns-syntax/src/attach.rs`, `crates/ns-syntax/src/attach/reference.rs` and the documentation of Task 4 changes. The lexer does not change, so `NewspeakLexer.kt` does not.

## Review Focus

1. **Children a recovery made that start at the same token**: the reference orders its children last-declared first, then sorts them stably by first token; the new expansion must list them in the same order, or `child_around` picks another child. Covered by Task 1's prefix oracle (`the_attachment_attaches_as_the_reference_over_every_prefix`).
2. **A node that receives a `Before` comment from its parent, comments of its own, and an `After` comment from its parent**: its `comments` must stay in source order -- before, own, after -- although they are decided at two different expansions. Pinned by Task 3's `a_node_keeps_its_comments_in_source_order_across_two_expansions`.
3. **`Inner(n)` on a holder whose children overlap or end right before the comment**: the count leaves out each child's tokens before the gap and the `.` after a child only when the `.` stands before the gap. Pinned by Task 2's `inner_counts_a_period_only_before_the_gap` and the prefix oracle.
4. **A comment inside a pattern literal** is attached nowhere, and an empty part with a comment between its bars is kept, without it dropped. Covered by the oracle over `goldens/` (pattern literals and empty parts occur there) and by the existing tests `a_comment_before_empty_bars_attaches_as_without_them`, `a_comment_after_an_empty_class_side_attaches_as_without_it`.
5. **A chain of 100,000 messages and a cascade of 100,000 messages** each with comments: the work grows linearly and nothing overflows the stack. Pinned by Task 3's two work tests, run on a 256 KiB stack.

---

### Task 1: The reference attachment and the oracle

Today's algorithm is copied, unchanged, into `attach/reference.rs`, compiled only for tests. `attach.rs` keeps its own copy for now (Task 2 replaces it), so the oracle compares two identical implementations and passes: this task proves the oracle's harness, not the new algorithm.

**Files:**
- Create: `crates/ns-syntax/src/attach/reference.rs`
- Modify: `crates/ns-syntax/src/attach.rs` (add `#[cfg(test)] mod reference;`, make `every_source` `pub(super)`, move one test, add the oracle tests)

**Interfaces:**
- Produces: `reference::attach_comments(module: &mut Module, tokens: &[Token], comments: &[Comment], source: &str)`, `reference::extents(module: &mut Module, tokens: &[Token], comments: &[Comment]) -> Vec<Option<(usize, usize)>>`; in `attach::tests`: `pub(super) fn every_source() -> Vec<PathBuf>`, `fn unattached(source: &str) -> (Module, Vec<Token>, Vec<Comment>)`.

- [ ] **Step 1: Create `attach/reference.rs`**

Copy from `crates/ns-syntax/src/attach.rs`, character for character, these items: `struct Node`, `enum Shape`, `enum SelectorShape`, `enum Pending`, `struct Tree` with its whole `impl` (`build`, `measure`, `inside_a_pattern`, `place`, `child_around`, `child_ending_at`, `child_starting_at`, `own_tokens_before`, `first`, `last`), `enum Neighbour`, `struct Builder` with its whole `impl` (`add`, `flatten`, `flatten_class`, `flatten_side`, `flatten_expression`, `drop_if_empty`). Do **not** copy `attached`, `placement`, `closes`, `merge`, `widen`, `widen_annotation`, `widen_delimiters`, `literal_span` or `Tokens`: the reference imports those. Put this above them:

```rust
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
```

Fix only what the move needs: the copied items use `crate::comment::Comment` and `CommentKind` only through the imports above (remove `CommentKind`/`lines_of`/`starts_its_line` from the imports if the copied items do not use them; clippy says which).

- [ ] **Step 2: Declare the module and share the source list**

In `crates/ns-syntax/src/attach.rs`, after the `use` lines (line 123), add:

```rust
#[cfg(test)]
mod reference;
```

In `attach.rs`'s `mod tests`, change `fn every_source() -> Vec<PathBuf>` to `pub(super) fn every_source() -> Vec<PathBuf>`, and add below it:

```rust
    /// `source` lexed and parsed as `parse_for_editor` does, nothing
    /// attached: the module, the parser's tokens and the comments.
    pub(super) fn unattached(source: &str) -> (Module, Vec<Token>, Vec<Comment>) {
        let lexed = crate::lexer::Lexer::new(source).run_keeping_comments();
        let mut parser = crate::parser::Parser::new(source, lexed.tokens);
        let module = parser.parse_module();
        let (tokens, _) = parser.into_parts();
        (module, tokens, lexed.comments)
    }
```

- [ ] **Step 3: Move the tree test to the reference**

Move `every_comment_of_every_module_is_attached_once_to_a_tree_of_disjoint_extents` out of `attach.rs`'s tests into a `#[cfg(test)] mod tests { use super::*; use crate::attach::tests::every_source; … }` at the end of `reference.rs`. In it, the call `attach_comments(&mut module, &tokens, &lexed.comments, &source);` now names the reference's `attach_comments` (the `use super::*` brings it). Rename the test `the_reference_attaches_every_comment_once_to_a_tree_of_disjoint_extents`. `attach::tests` must be visible: change `mod tests` in `attach.rs` to `pub(super) mod tests`... it is private to `attach`, and `reference` is a child of `attach`, so `crate::attach::tests::every_source` with `pub(super)` on the function and the module declared `mod tests` is reachable from `attach::reference` (a descendant of `attach`). If the compiler disagrees, declare it `pub(super) mod tests`.

- [ ] **Step 4: Write the oracle tests**

Add to `attach.rs`'s `mod tests`:

```rust
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
```

- [ ] **Step 5: Run the attachment tests**

Run: `cargo nextest run -p ns-syntax -E 'test(/attach/)'`
Expected: PASS, the three tests named above among them (the two implementations are still identical). Note the time `the_attachment_attaches_as_the_reference_over_every_prefix` takes; report it if it is above 30 s.

- [ ] **Step 6: Commit**

```bash
cargo xtask gates commit
git add crates/ns-syntax/src/attach.rs crates/ns-syntax/src/attach/reference.rs
git commit -m "test: keep the comment attachment's first algorithm as its reference"
```

---

### Task 2: The attachment on demand

Replace `attach.rs`'s tree with the three steps of the spec. The oracle of Task 1 is the test that fails while the new code is wrong.

**Files:**
- Modify: `crates/ns-syntax/src/attach.rs` (delete the old `Node`, `Shape`, `SelectorShape`, `Pending`, `Tree`, `Neighbour`, `Builder` and their impls from this file -- the reference keeps its copies; add the new items below; keep `attached`, `placement`, `closes`, `merge`, `widen`, `widen_annotation`, `widen_delimiters`, `literal_span`, `Tokens`)

**Interfaces:**
- Consumes: `reference::extents`, `tests::every_source`, `tests::unattached` (Task 1).
- Produces: `fn attach(module: &mut Module, tokens: &[Token], comments: &[Comment], source: &str) -> usize` (the work count, read by Task 3); `attach_comments` keeps its signature and calls it.

- [ ] **Step 1: Write the failing extent test**

Add to `attach.rs`'s `mod tests`:

```rust
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
        let count = prepass(module, &mut parts, &mut work);
        let kept = Tokens::kept(tokens, &parts.sorted().0);
        measure(module, &kept, count, &mut work)
            .iter()
            .map(|entry| entry.extent())
            .collect()
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-syntax -E 'test(the_walk_measures)'`
Expected: FAIL to compile -- `Parts`, `prepass`, `measure`, `Tokens::kept` are not defined.

- [ ] **Step 3: Replace the tree with the new items**

Delete from `attach.rs` the old `attach_comments` body, `Node`, `Shape`, `SelectorShape`, `Pending`, `Tree` and its impl, `Neighbour`, `Builder` and its impl. Keep `Tokens` and its impl, and add `kept` to it. Then add the following (doc comments as written; they are part of the code):

```rust
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

/// [`attach_comments`], answering the work it did: the nodes its walks
/// visited, the nodes it expanded, the children those had and the comments
/// it placed. Only a node a comment lies in is expanded, and every walk is
/// linear, so a module of many nodes and few comments costs two walks; a
/// test holds the work to growing linearly with the source.
fn attach(module: &mut Module, tokens: &[Token], comments: &[Comment], source: &str) -> usize {
    if comments.is_empty() {
        return 0;
    }
    let mut work = 0;
    let mut parts = Parts::new(tokens, comments);
    let count = prepass(module, &mut parts, &mut work);
    let (dropped, patterns) = parts.sorted();
    let kept = Tokens::kept(tokens, &dropped);
    let entries = measure(module, &kept, count, &mut work);
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
/// the walks number them (the last appended is numbered first); given
/// `parts`, records the empty parts and the pattern literal the node
/// carries. The one place that says what each kind of node is made of:
/// the prepass, the walk and the descent all read it, so that the descent
/// finds a child at the index the walk gave it.
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

/// Counts the commentable nodes and records the empty parts and the
/// pattern literals: the tokens a comment is counted among depend on the
/// first, so they are known before anything is measured.
fn prepass(module: &mut Module, parts: &mut Parts<'_>, work: &mut usize) -> usize {
    let mut stack = vec![Pending::Module(module)];
    let mut count = 0;
    while let Some(node) = stack.pop() {
        count += 1;
        describe(node, &mut stack, Some(&mut *parts));
    }
    *work += count;
    count
}

/// A commentable node's extent and the size of its subtree, itself
/// included. The entries stand in the order the walks visit the nodes --
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

enum Step<'tree> {
    Visit {
        node: Pending<'tree>,
        parent: usize,
    },
    Finish {
        index: usize,
        parent: usize,
        shape: Shape,
        known: Option<Span>,
    },
}

/// Every commentable node's extent, by the rules the module documentation
/// gives: one walk, children before their parent, each child's extent
/// merged into its parent's entry until the parent finishes.
fn measure(module: &mut Module, tokens: &Tokens, count: usize, work: &mut usize) -> Vec<Entry> {
    let mut entries = Vec::with_capacity(count);
    let mut steps = vec![Step::Visit {
        node: Pending::Module(module),
        parent: NO_PARENT,
    }];
    let mut children = Vec::new();
    while let Some(step) = steps.pop() {
        match step {
            Step::Visit { node, parent } => {
                let index = entries.len();
                entries.push(Entry::EMPTY);
                let Description { shape, known, .. } = describe(node, &mut children, None);
                steps.push(Step::Finish {
                    index,
                    parent,
                    shape,
                    known,
                });
                steps.extend(
                    children
                        .drain(..)
                        .map(|node| Step::Visit { node, parent: index }),
                );
            }
            Step::Finish {
                index,
                parent,
                shape,
                known,
            } => {
                let children_extent = entries[index].extent();
                let mut entry = Entry {
                    size: index_word(entries.len() - index),
                    ..Entry::EMPTY
                };
                if let Some(extent) = extent_of(tokens, known, shape, children_extent) {
                    entry.widen(extent);
                    if parent != NO_PARENT {
                        entries[parent].widen(extent);
                    }
                }
                entries[index] = entry;
            }
        }
    }
    debug_assert_eq!(entries.len(), count);
    *work += entries.len();
    entries
}

/// A node's extent from its known span, its shape and its children's
/// merged extent.
fn extent_of(
    tokens: &Tokens,
    known: Option<Span>,
    shape: Shape,
    children: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    let mut extent = known
        .and_then(|known| tokens.covering(known))
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
                SelectorShape::Unary => tokens
                    .starts
                    .partition_point(|start| *start < end)
                    .checked_sub(1),
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
```

Add `kept` to `impl Tokens`:

```rust
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
```

Add `inside_a_pattern` as a free function:

```rust
/// Whether `span` lies inside one of `patterns` (sorted by start): such a
/// comment is part of the literal's text and attached nowhere.
fn inside_a_pattern(patterns: &[Span], span: Span) -> bool {
    let before = patterns.partition_point(|pattern| pattern.start <= span.start);
    before
        .checked_sub(1)
        .is_some_and(|index| span.end <= patterns[index].end)
}
```

- [ ] **Step 4: Add the descent**

```rust
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
                let own_tokens = own_tokens
                    .get_or_insert_with(|| OwnTokens::new(self, holder, children));
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
            first: context.entries[holder].extent().map_or(0, |(first, _)| first),
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
```

- [ ] **Step 5: Add the disjointness test and the `Inner` period test**

Add to `attach.rs`'s `mod tests`:

```rust
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
            let count = prepass(&mut module, &mut parts, &mut work);
            let kept = Tokens::kept(&tokens, &parts.sorted().0);
            let entries = measure(&mut module, &kept, count, &mut work);
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

    /// A `.` after a child counts as the child's only when it stands
    /// before the comment: before `(* c *)` the class body has `(` and the
    /// `.` after `x`, after it nothing of `x`'s.
    #[test]
    fn inner_counts_a_period_only_before_the_gap() {
        for (source, expected) in [
            ("class A = ( x.\n  (* c *)\n) ()", Anchor::Inner(1)),
            ("class A = ( x\n  (* c *)\n.) ()", Anchor::Inner(1)),
        ] {
            let module = crate::parse(source).expect("parse");
            let side = &module.root_class.instance_side;
            assert_eq!(side.comments[0].anchor, expected, "{source}");
        }
    }
```

If the second source of `inner_counts_a_period_only_before_the_gap` does not parse, or its anchor is not `Inner(1)` under the reference too, replace it with a source of your own where a child's `.` stands after an `Inner` comment, and assert what the reference answers for it (`reference::attach_comments` over `unattached(source)`); report the source you chose.

- [ ] **Step 6: Run the attachment tests**

Run: `cargo nextest run -p ns-syntax`
Expected: PASS, every test, among them `the_walk_measures_the_extents_the_reference_measures`, `the_walk_measures_a_tree_of_disjoint_extents`, `the_attachment_attaches_as_the_reference_over_every_source`, `the_attachment_attaches_as_the_reference_over_every_prefix`. If an oracle test fails, find the first differing comment by comparing the two modules' comments node by node (write a throwaway helper, do not commit it), fix the new code, and never the reference.

- [ ] **Step 7: Lint**

Run: `cargo clippy -p ns-syntax --all-targets`
Expected: no warning. The deleted items must leave nothing unused in `attach.rs`; an item only the reference uses (for instance `Tokens`'s fields) stays, used by both.

- [ ] **Step 8: Commit**

```bash
cargo xtask gates commit
git add crates/ns-syntax/src/attach.rs
git commit -m "perf: attach a comment by expanding only the nodes it lies in"
```

---

### Task 3: Linear work and the order of a node's comments

**Files:**
- Modify: `crates/ns-syntax/src/attach.rs` (tests only)

**Interfaces:**
- Consumes: `attach(module, tokens, comments, source) -> usize` (Task 2), `tests::unattached` (Task 1).

- [ ] **Step 1: Write the tests**

Add to `attach.rs`'s `mod tests`:

```rust
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

    /// `x (* c *) foo foo …`: the comment lies as deep as the chain is
    /// long.
    fn chain(links: usize) -> String {
        format!(
            "class A = () ( m = ( ^x (* c *){} ) )",
            " foo".repeat(links)
        )
    }

    /// `x foo (* c *) ; bar (* c *) ; bar …`, a comment before every `;`,
    /// each on a line of its own so that no comment has another beside it.
    fn cascade(messages: usize) -> String {
        format!(
            "class A = () ( m = ( ^x foo{} ) )",
            " (* c *)\n  ; bar".repeat(messages)
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
        let method = module.root_class.instance_side.methods().next().expect("at:put:");
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
```

- [ ] **Step 2: Run them**

Run: `cargo nextest run -p ns-syntax -E 'test(/the_work_grows|source_order_across/)'`
Expected: PASS. If `a_node_keeps_its_comments_in_source_order_across_two_expansions` fails only on the `Inner` count, check what the reference answers for the same source (`reference::attach_comments` over `unattached(source)`) and assert that; the order of the three is what the test pins. If a work test fails, the descent or a walk is not linear: find the loop, never loosen the bound.

- [ ] **Step 3: Commit**

```bash
cargo xtask gates commit
git add crates/ns-syntax/src/attach.rs
git commit -m "test: hold the comment attachment to linear work and to source order"
```

---

### Task 4: Measure, profile, document

**Files:**
- Modify: `docs/design/frontend.md` ("Comments", and the benchmark paragraph under "The corpus as a parser gate")
- Modify: `docs/OPEN-WORK.md` ("Comment attachment: a super-linear corner, and its cost on every boot from source"; a new item for sub-project B)

- [ ] **Step 1: Build the trees to compare**

Before is commit `f90be321` (the benchmark, the old attachment); after is `HEAD`. Three builds a side, each in a target directory of its own, all with the alignment pin (`measuring-performance` skill), in PowerShell from the repository root:

```powershell
git worktree add .temp/before f90be321
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
foreach ($build in 1..3) {
  $env:CARGO_TARGET_DIR = "D:\Workspace\ns-next\.temp\target-before-$build"
  cargo bench --manifest-path .temp/before/Cargo.toml -p ns-syntax --bench corpus --no-run
  $env:CARGO_TARGET_DIR = "D:\Workspace\ns-next\.temp\target-after-$build"
  cargo bench -p ns-syntax --bench corpus --no-run
}
Remove-Item Env:RUSTFLAGS
Remove-Item Env:CARGO_TARGET_DIR
```

Three builds of one tree in three target directories differ only if the build is not reproducible; if `Get-FileHash` shows the three `corpus-*.exe` of a side identical, say so in the report -- one binary measured three times is then what was measured.

- [ ] **Step 2: Run them, interleaved**

Each binary three times, before and after alternating, `--rounds 40`, from `crates/ns-syntax` (the benchmark reads `../../corpus/modules` relative to its manifest, so either tree's binary reads this tree's corpus -- the same modules for both):

```powershell
Set-Location crates/ns-syntax
foreach ($run in 1..3) { foreach ($build in 1..3) { foreach ($side in "before","after") {
  $exe = Get-ChildItem "D:\Workspace\ns-next\.temp\target-$side-$build\release\deps\corpus-*.exe" | Select-Object -First 1
  & $exe.FullName --rounds 40 --top 0 2>&1 | Select-String "^(lex|parse|editor) " | ForEach-Object { "$side $build $run $_" }
} } }
Set-Location ../..
```

(`--top 0` is refused by the benchmark -- a count must be positive -- so use `--top 1` instead.)

Record per build and side the median total of `parse` and `editor`; pool per side (the median of the nine runs); give the spread (min–max of the nine).

- [ ] **Step 3: Profile after**

Following how the baseline profile was taken (samply 0.13 on Windows does not read the binary's PDB names, so `dumpbin` resolves them):

```bash
cd /d/Workspace/ns-next
CARGO_PROFILE_BENCH_DEBUG=true cargo bench -q -p ns-syntax --bench corpus --no-run
EXE=$(ls -t target/release/deps/corpus-*.exe | head -1)
SCRATCH=<the session's scratchpad directory>
"/c/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.51.36231/bin/Hostx64/x64/dumpbin.exe" /disasm:nobytes $EXE > $SCRATCH/disasm.txt
cd crates/ns-syntax
samply record --save-only --unstable-presymbolicate -o "$SCRATCH/after.json" ../../$EXE --rounds 30
```

Map each address to the function `dumpbin` lists above it and aggregate self and inclusive samples per function (a short Python script; the profile is the Firefox processed format, `threads[].samples.stack` into `stackTable`/`frameTable`, addresses relative to the library). Report the share of `ns_syntax::attach::*` against the 38 % of the baseline, and the top five functions under it.

- [ ] **Step 4: Document**

In `docs/design/frontend.md`, section "Comments": after the paragraph that names `attach.rs`, add a paragraph saying how the attachment works now -- the prepass, the one walk into an entry per node (12 bytes), the descent that expands only a node a comment lies in, the reference in `attach/reference.rs` that every source and every prefix of one is held to -- and what it costs, with the pooled numbers of Step 2 (before and after, per side, the spread) and the attachment's share from Step 3.

In `docs/OPEN-WORK.md`, rewrite "Comment attachment: a super-linear corner, and its cost on every boot from source": the corner is gone (an `Inner` count is two searches, and `the_work_grows_linearly_with_comments_among_many_children` holds it); what stays is that the runtime pays for the attachment and discards the comments, with the attachment's cost on `Kernel.ns` measured afresh (`cargo bench -p ns-syntax --bench corpus -- ../runtime/core-library --filter Kernel.ns` from `crates/ns-syntax` gives `parse`; the item's earlier figures are replaced, not kept beside). Add an item "**Fewer allocations in the syntax tree** (sub-project B of the syntax crate's optimization)": the parser's share and dropping the module's (14 % of the baseline profile), what allocates there (a `String` per identifier and selector, a `Box` or `Vec` per node, `parse_keyword_parts` growing its vectors), and that it needs a design of its own, the AST being read by `ns-runtime`, `ns-analyzer`, `ns-tooling` and `ns-cli`.

- [ ] **Step 5: Remove the measurement's trees**

```powershell
git worktree remove .temp/before --force
Remove-Item -Recurse -Force .temp/target-before-1, .temp/target-before-2, .temp/target-before-3, .temp/target-after-1, .temp/target-after-2, .temp/target-after-3
```

- [ ] **Step 6: Commit**

Markdown only, so no gate runs (`CLAUDE.md`, "Every commit" is every commit that changes more than Markdown):

```bash
git add docs/design/frontend.md docs/OPEN-WORK.md
git commit -m "docs: describe the comment attachment on demand and what it saved"
```

Then report to the controller: the per-build numbers, the pooled result, the spread, the profile's shares, and what the measurement did not cover (one machine, Windows' heap allocator, the corpus only).
