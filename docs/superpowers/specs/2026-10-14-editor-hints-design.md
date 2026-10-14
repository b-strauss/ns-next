# Editor hints: initialize in the declaration, read-only

A temporary design. At the end of the batch its content moves into
`docs/design/editor-support.md` (a new section "Hints") and this file is
deleted in the same commit.

## Goal

The language server points at two things in any `.ns` file, as a subtle hint
the editor shows as a weak warning, each with a quick fix and one fix for the
whole file:

- **A, initialize in the declaration**: a local declared bare whose first
  value a statement assigns can take that value in its declaration.
- **B, read-only**: a local declared with `::=` that nothing ever assigns
  again can be declared with `=`.

Both are given for the locals of methods and closures only. A class's
header slots get no hint of either kind (for now), private ones included.

Every fix preserves behaviour; where that cannot be proved from the source
alone, no hint is given. The formatter stays as it is: it changes layout
only, and these hints change code.

## The rules

A *section* is a code body, a method's or a closure's, with its temporaries
and statements. A class's header is no section: its slots get no hint, but
the closures in its expressions are sections like any other. A section
with a simultaneous slot clause (`|| ||`) gets no hint.

### Hint A

A local `x` is a candidate when it is declared bare (`x`, no initializer)
and a statement `x:: e.` of its own section assigns it. The statement
qualifies when no comment stands in it, in the text a fix deletes or on the
rest of its line, no comment is attached to it (it would then describe the
next statement), it is not parenthesized and is not a chain (`x:: y:: e`),
it is not the last statement of a closure body (a closure answers its last
statement's value, which an initializer does not have; a method answers
`self` without `^`), and one of these holds:

1. **At the start.** It is the section's first statement, and `e` does not
   mention `x`. Moving it must keep the order of evaluation: every
   existing initializer of the section first, then `e`. The local may be
   reordered for that -- its declaration moves after the last initialized
   one, unless a comment stands between it and the next local or after the
   last initialized one on its line (the move would copy the one or leave
   the other behind the moved declaration). Each fix stands alone: once
   the first statement has moved, the next one is first, which is how
   "apply all" moves a whole run of them.
2. **After other statements.** `e` is a literal, `self`, `nil`, `true`,
   `false`, or an identifier whose innermost declaration in scope is a
   parameter (of a method or a closure) or a read-only local (`=`); no
   statement before it and no initializer of the section mentions the
   name `x` anywhere, nested closures included (a closure made by an
   initializer could read `x` before the statement runs); and, as in 1,
   every existing initializer of the section still runs before it -- the
   local's declaration moves after the last initialized one when needed. Such an `e` can neither fail nor observe
   anything those statements change, and neither they nor any initializer
   can observe `x` before it, so evaluating `e` earlier is unobservable.

### Hint B

A local `x` declared with `::=` is a candidate when nothing can assign it:
no `x::` anywhere in the body, nested closures included. Only `x::` writes
a local: an implicit-receiver send `x: v` is resolved through the classes,
never the locals (`resolver/mod.rs`).

The name test is conservative: an `x::` that resolves to something else (a
shadowing closure local) still counts.

Only a module that parses without error gets hints.

## The server

- `crates/ns-analyzer/src/hints/` computes the hints from the AST, as the
  printer reprints it: for each hint its kind, the span of the local's name,
  its section, its message and its text edits in byte offsets, and "apply
  all". It holds no LSP type; the analysis is syntactic, so `ns-syntax`'s
  tree is all it needs.
- They are computed once per document version with the rest of the analysis
  (`analysis.rs`) and cached.
- `publishDiagnostics` carries them beside the errors: severity `HINT`, `code`
  `initialize-in-declaration` or `read-only`, `source` `ns-next`.
- The server announces `codeActionProvider`. `textDocument/codeAction`
  answers, for each hint the range touches, a `quickfix` action naming its
  diagnostic and carrying its edit; and, when the file has any hint, one
  action "Apply all hints in this file" beside them. The edits are plain
  `changes`: with full synchronisation the server always holds the text
  the client asks about.
- **Apply all** works in rounds until no hint is left: each round applies,
  in memory, at most one hint per section and none whose edits overlap
  another's (a moved statement may hold a closure with hints of its own),
  then analyses the result again. Every fix is valid on its own, so
  every round preserves behaviour. It answers one edit over the smallest
  range covering every change.
- **The edits are minimal**, so a file keeps its layout:
  - A appends ` ::= <value>.` to the declaration (` ::= <value>` before the
    `.` a bare local may be written with), the value copied verbatim
    from the source, its continuation lines shifted by the difference in
    indentation; deletes the statement and its period, and its line when
    that leaves the line blank; and, when a local is reordered, moves its
    declaration, its own `.` removed with it.
  - B replaces `::=` by `=`.
  - Every fix's result parses: the rules promise it, "apply all" checks it
    with a `debug_assert!`, and the behaviour oracle holds every golden to
    it. Parsing each fix's result while hints are computed would cost a
    parse per hint on every keystroke, so it is not done there.
- The plugin needs no code: the platform shows a `HINT` as a weak warning
  (`LspDiagnosticsSupport.getHighlightSeverity`) and offers its quick fixes
  (`LspCodeActionsSupport`). Checked by hand under `runIde` and recorded in
  `editor-support.md`.

## Tests

- **`hints.rs` unit tests**: each rule above, its positive and negative
  cases -- guards, reordering, header slots getting no hint of either
  kind, a shadowing closure local, a commented statement,
  a chain -- each fix applied and compared with the expected text
  (`indoc!` modules).
- **The behaviour oracle**: "apply all" over every `goldens/**/*.ns`, each
  result run under both lowerings and held to the `status`, `stdout` and
  `stderr` in the `.toml` beside it; each result parses, and a second
  "apply all" changes nothing. The goldens themselves are not rewritten.
  It is `ns-analyzer`'s `tests/hinted_goldens.rs`, which builds `ns-next`
  itself, and runs only in its own nextest profile, when the hint code
  changes: `cargo nextest run -p ns-analyzer --profile hints`.
- **The protocol test** (`tests/protocol.rs`): hint diagnostics are
  published, code actions answered, "apply all" among them.
- **The core-library gate**: a test, beside the formatting gate, that no
  shipped core-library module (`crates/ns-runtime/core-library/`) has a
  hint. The core library is brought to it in this batch; `goldens/`,
  `benchmarks/` and `corpus/` are not held to it.

## Documentation

- `docs/design/editor-support.md`: the section "Hints" (this design's
  rules and reasons), the feature list, the protocol tests' list.
- `CLAUDE.md`: `ns-analyzer`'s `lsp` gains code actions in its feature list.
