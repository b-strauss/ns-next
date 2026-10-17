# Comment attachment on demand -- design

Sub-project A of the syntax crate's optimization (branch
`perf/syntax-benchmarks`). Sub-project B, fewer allocations in the AST,
gets a spec of its own after A is measured. This spec is temporary: at the
end of the batch its content moves into `docs/design/frontend.md` and the
file is deleted.

## Why

`cargo bench -p ns-syntax --bench corpus` over the 211 corpus modules
(release, 2026-10-17): `parse` 115 ms for 6.3 MB and 675,000 tokens,
170 ns per token; the lexer alone 9.3 ms, 13.8 ns per token. A profile of
the whole corpus (samply, every phase) puts `attach_comments` at 38 % of
the time, the parser at 37 %, dropping the module at 14 %, the lexer at
11 %. Within the attachment, `Tree::build` growing its vectors -- the
`nodes` vector and one `children` vector per node -- is 14 % of the
whole. The tree holds every commentable node of a module that has at least
one comment: `KernelTests.ns` has 104 comments and 44,107 nodes, each
`Node` 80 bytes plus its own `children` allocation.

What placing a comment needs is less: the descent from the module to the
innermost node with a token on each side of the comment, and at each node
on that path its direct children's extents (first and last token). The
inside of a child no comment lies in is never read.

## What stays

- The rules -- the module documentation of `attach.rs`, "Commentable
  nodes and their own tokens" and "Where a comment goes" -- and so every
  `AttachedComment` any source gets: anchor, placement, lines, column,
  span. The new algorithm is an implementation of the same rules, held to
  the old one.
- `attach_comments`' signature and its callers (`parse`,
  `parse_for_editor`).
- Robustness: no panic on a recovered tree, overlapping nodes taken as they
  come, a comment no node holds goes to the module, a chain of messages
  walked link by link from an explicit stack, never by recursing along a
  receiver.

## The algorithm

Three linear steps; no step walks a subtree more than once.

1. **The prepass.** An iterative walk (explicit stack) over the module
   collects the tokens of the empty parts -- a slot section or temporaries
   without a declaration and without a comment between the bars, a class
   side without a method and without a comment between its parentheses,
   its `:` included -- the spans of the pattern literals, and the number
   of commentable nodes. From the first, as today, the tokens counted: the
   parser's, without the end of input and without an empty part's.
2. **The extents.** A second iterative walk computes in post-order each
   commentable node's extent by exactly the rules of today's `measure`:
   the tokens covering its known own spans, merged with its children's
   extents; the module spans every token; an implicit receiver has none; a
   message adds its lead (a unary selector's token, or the token before
   its first argument for a binary or keyword one, and the `<-:` before
   it when eventual). It writes them into one array, sized by the
   prepass's count, in pre-order: per node the first token, the last token
   and the size of its subtree, three `u32` (12 bytes). There is no
   per-node structure and no per-node allocation.
3. **The descent on demand.** From the module, a node is expanded only
   when comments are assigned to it. Its direct children and their array
   indices follow from the pre-order and the subtree sizes; those with an
   extent are sorted by their first token, as `measure` sorts them. A
   comment whose gap lies inside a child goes to that child; the others
   are placed on this node by rules 1 to 4. Comments and children are both
   in source order, so `Inner(n)` -- the node's own tokens before the
   comment, a child's tokens and the `.` that goes with it not counted --
   is counted in one merge over the children for all of the node's
   comments, not per comment over every child; the quadratic corner of
   `OPEN-WORK.md` ("Comment attachment: a super-linear corner") goes with
   it. The comments are pushed into the node's `comments` through the
   `&mut` borrow the expansion holds.

One function, `describe`, says per kind of commentable node its known own
spans, its `Shape`, its children in order, and the empty parts and
pattern literals it carries. All three steps read it, so the pre-order
indices of the walks and of the descent agree; `debug_assert!`s check that
the descent's index arithmetic lands on the node the walk numbered.

Cost: two linear walks with no per-node allocation, one array of 12 bytes
per node, and the expansion only along the comments' paths. How much this
saves against today's tree is a measurement, not a derivation: the
estimate is that the attachment's share falls to between a third and a
quarter of its 38 %.

## Files

- `crates/ns-syntax/src/attach.rs`: the rules' documentation,
  `attach_comments`, the helpers both implementations share unchanged --
  the leaves that turn a decided placement into an `AttachedComment`
  (`attached`, `placement`) and the token predicates and arithmetic
  (`closes`, `merge`, `Tokens` and its `covering`) -- and the new
  `describe`, the two walks and the descent. What a node's own spans,
  shape, children and empty parts are is not shared: the reference keeps
  its own `flatten` and `drop_if_empty`.
- `crates/ns-syntax/src/attach/reference.rs`, `#[cfg(test)]` only: today's
  algorithm -- `Builder` with its own `flatten`, `Tree`, `measure`,
  `place` -- moved unchanged, the independent second derivation the new
  one is held to, as `compiler/reference.rs` is for the compiler. It
  stays permanently (decided with the user, 2026-10-17); its module
  comment says that a change to the rules is made in both.

## Tests

- **The oracle over every source.** For every `.ns` under `corpus/`,
  `goldens/`, `benchmarks/` and `crates/ns-runtime/core-library/`: the
  module parsed and attached by the reference and by the new algorithm
  are equal (`Module: PartialEq`, every `AttachedComment` included).
- **The oracle over recovered trees.** The same through
  `parse_for_editor` over prefixes of every source, cut at 16 points per
  file spread over its length, so the recovery's overlapping and
  unfinished nodes are covered.
- **Linear work.** A chain of 100,000 unary messages with a comment at its
  innermost receiver, and a cascade of 100,000 messages with a comment
  before every `;`: a count the new algorithm keeps in test builds of the
  work the walks and the descent do (nodes visited, nodes expanded and
  children visited) grows linearly with the length -- checked at two
  lengths -- rather than a time bound.
- **The existing tests.** The disjoint-extents test
  (`every_comment_of_every_module_is_attached_once_to_a_tree_of_disjoint_extents`)
  runs on both: on the reference's tree as today and on the new extents
  array. Every attachment, recovery, printer and formatting test stays as
  it is and passes.

## Measurement

The corpus benchmark under the `measuring-performance` protocol: the
alignment pin, several builds a side, before against after; the
per-build numbers, the pooled result and the spread; and a profile after,
to say where the attachment's remaining time goes.

## Documentation

- `docs/design/frontend.md`, "Comments": how the attachment works and what
  it costs, with the measured numbers.
- `docs/OPEN-WORK.md`: "Comment attachment: a super-linear corner, and its
  cost on every boot from source" rewritten to what is left -- the
  runtime pays for the attachment and discards the comments -- and an
  item for sub-project B, fewer allocations in the AST (the parser's
  share, and the 14 % of dropping the module).

## Not in scope

- A `parse` without the attachment for the runtime (the rest of the
  OPEN-WORK item above): it touches `ns-runtime`, and is a question of its
  own.
- Any change to the AST, the parser or the lexer (sub-project B).
