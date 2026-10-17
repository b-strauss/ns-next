# Local extents for an error-free parse -- design

A follow-up to the comment attachment on demand (`attach.rs`;
`docs/design/frontend.md`, "Comments"), on the branch
`perf/local-extents`. This spec is temporary: at the end of the batch its
content moves into `frontend.md` and the file is deleted.

## Why

Under mimalloc, the allocator of both binaries, `parse` of `Kernel.ns`
takes about 1.05 ms, and the attachment about 0.45 ms of it (samply,
2026-10-17, a release build with line tables and the alignment pin). A
module without a comment skips the attachment (`attach.rs:148`); a module
with one pays a prepass that describes every commentable node and a loop
that measures every node's extent, about 0.25 ms of the 0.45, and
`describe` another 0.07. Building the attached comments, the per-line
`String`s included, is about 0.12 ms and is not this spec's concern.

The descent needs an extent only for the children of the nodes it
expands. A spike (2026-10-17, not committed) compared, for every
commentable node of every source the oracle tests read, the extent the
loop measures with the extent of the tokens covering the spans the node
itself carries (`describe`'s `known`, no child consulted):

- On the whole sources -- error-free parses -- the two agree for every
  `Expression`, `BlockExpression`, `ClassDeclaration`,
  `MethodDeclaration`, `SuperclassClause`, `ClassBody` and the `Module`,
  about 400,000 nodes, and disagree for most `Statement`s,
  `SlotDeclaration`s, `MessageSend`s and `CodeBody`s, whose `known` does
  not cover their children (a statement's is its `^`, a slot's its name
  and type, a message's its type arguments, a body's its temporaries'
  bars); their own `span` fields do not match the rule either (a slot's
  takes in its `.`, a message's starts at a keyword the rule does not
  count as its lead).
- On the prefixes of the sources -- recovered parses -- they disagree for
  the `ClassBody` too: a recovered node's spans need not cover its
  children.

## What stays

- The rules -- the module documentation of `attach.rs` -- and so every
  `AttachedComment` any source gets.
- `attach_comments`' signature and its callers.
- The prepass and the loop over the records, as the path of a parse with
  an error, unchanged; the descent, its `Inner` counts, `push` and the
  placement unchanged but for where a child's extent comes from.
- The reference (`attach/reference.rs`) and the oracle tests.

## The design

**Two paths.** `attach` takes the *local* path when the parse has no
error -- `parse` always, `parse_for_editor` whenever the source parses
cleanly, which is most keystrokes -- and the *walk* path, today's
prepass and loop, otherwise. The callers in `lib.rs` already know
whether the parse had an error and say so.

**The empty parts and the pattern literals from the parser.** The kept
tokens leave out an empty part's tokens, and the descent leaves out a
comment inside a pattern literal; the prepass collects both today. On the
local path the parser has collected them: when it finishes a slot section
or temporaries without a declaration, or a class side without a member,
it records the delimiters (and a class side's `:`), and when it builds a
pattern literal its span. `Parts::drop_if_empty`'s test -- no comment
between the delimiters -- is applied to the recorded parts as it is
applied today, so `Tokens::kept` gets the same list. The two lists leave
the parser through `Parser::into_parts`, beside the tokens and the
diagnostics, never into the tree.

**A child's extent, local.** On the local path the descent asks for a
child's extent when it expands the child's parent, from the child alone:

- `Module`: every token, as today.
- An `Expression`, `BlockExpression`, `ClassDeclaration`,
  `MethodDeclaration`, `SuperclassClause` or `ClassBody`: the tokens
  covering `describe`'s `known`; a synthesized expression none.
- A `Statement`, `SlotDeclaration`, `MessageSend` or `CodeBody`:
  `extent_of` over `describe`'s `known` and `shape` and the merged extent
  of its children, each child's extent local in turn -- for a
  `CodeBody` its first and its last child, since an error-free body's
  children stand in source order.

The recursion is bounded: a `Statement`, a slot or a message reaches
expressions, which stop; a body reaches statements and slots. No chain of
messages is followed, since an expression's extent never asks its
receiver. The searches are `Tokens::covering`'s binary searches, two per
child.

**Why it is exact on an error-free parse**, and only there: the parser
starts an `Expression`, a block, a class, a method, a superclass clause
and a class side at its first token and ends it at its last, so their
spans cover their subtrees; the four other kinds are composed from those
by the rule itself. A recovered tree breaks the first part, which is why
the walk stays for it. The oracle tests check the claim, not this
argument.

**What is no longer paid** on the local path: the prepass's `describe`
of every node, the `Vec<Record>` and the `Vec<Entry>` sized by the
nodes, and the loop. What is added: two searches per child of an
expanded node, and the parser's two lists. The estimate, before
measuring, is the attachment of `Kernel.ns` from about 0.45 to about
0.15 ms.

## Tests

- **The oracle over every source** runs both paths: the whole sources
  take the local path, and a variant forces the walk on them too; both
  must attach as the reference.
- **The oracle over every prefix** takes whichever path the parse
  decides; a prefix that parses without an error takes the local path.
- **The parser's lists** equal the prepass's: for every source, the
  empty parts and pattern literals the parser recorded are the ones the
  prepass collects.
- **A debug build checks the local path against the walk** on every
  parse that takes it: it attaches a clone of the module by the walk and
  asserts the two equal, so every parse in the workspace's debug suites
  -- goldens, formatting properties, the analyzer's tests -- is an
  oracle case. A release build does not.
- The linear-work tests stay, now counting the local path's work too;
  every existing attachment, recovery, printer and formatting test passes
  unchanged.

## Measurement

The corpus benchmark under mimalloc and on the platform heap, before
against after, under the `measuring-performance` protocol; `Kernel.ns`
and the corpus total, `parse` and `parse_for_editor`.

## Documentation

`frontend.md`, "Comments": the two paths and why the local one is exact;
the numbers. `OPEN-WORK.md`, "Comment attachment: the runtime pays for
it and discards the comments": its figures.

## Not in scope

- The per-line `String`s of an `AttachedComment`.
- A `parse` without the attachment for the runtime.
- Any change to the AST.
