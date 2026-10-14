# Editor hints: initialize in the declaration, read-only

A temporary design. At the end of the batch its content moves into
`docs/design/editor-support.md` (a new section "Hints") and this file is
deleted in the same commit.

## Goal

The language server points at two things in any `.ns` file, as a subtle hint
the editor shows as a weak warning, each with a quick fix and one fix for the
whole file:

- **A, initialize in the declaration**: a slot declared bare whose first
  value a statement assigns can take that value in its declaration.
- **B, read-only**: a slot declared with `::=` that nothing ever assigns
  again can be declared with `=`.

Every fix preserves behaviour; where that cannot be proved from the source
alone, no hint is given. The formatter stays as it is: it changes layout
only, and these hints change code.

## The rules

A *section* is a code body (a method's or a closure's) with its temporaries
and statements, or a class's instance-side header with its slots and
initializer expressions. A section with a simultaneous slot clause (`|| ||`)
gets no hint A.

### Hint A

A slot `x` is a candidate when it is declared bare (`x`, no initializer, not
`lazy`) and a statement `x:: e.` of its own section assigns it. The statement
qualifies when no comment stands in it, in the text a fix deletes or on the
rest of its line, it is not parenthesized and is not a chain (`x:: y:: e`),
and one of these holds:

1. **At the start.** It is the section's first statement, and `e` does not
   mention `x`. Moving it must keep the order of evaluation: every
   existing initializer of the section first, then `e`. A local may be
   reordered for that -- its declaration moves after the last initialized
   one; a header slot is never reordered, since the order of header slots
   is the class's layout, so a header slot declared before an initialized
   one gets no hint. Each fix stands alone: once the first statement has
   moved, the next one is first, which is how "apply all" moves a whole
   run of them.
2. **After other statements.** `e` is a literal, `self`, `nil`, `true`,
   `false`, or an identifier whose innermost declaration in scope is a
   parameter (of a method, a closure, or, in a header, the factory) or a
   read-only local (`=`); no statement before it mentions the name `x`
   anywhere, nested closures included; and, as in 1, every existing
   initializer of the section still runs before it -- a local's declaration
   moves after the last initialized one when needed, a header slot must
   stand after it already. Such an `e` can neither fail nor observe
   anything those statements change, and neither they nor any initializer
   can observe `x` before it, so evaluating `e` earlier is unobservable.

### Hint B

A slot `x` declared with `::=` is a candidate when nothing can assign it:

- **A local** (a temporary of a method or closure): no `x::` anywhere in the
  body, nested closures included. Only `x::` writes a local: an
  implicit-receiver send `x: v` is resolved through the classes, never the
  locals (`resolver/mod.rs`).
- **A private header slot**: no `x::` and no send of `x:` -- implicit
  receiver, `self`, `super` or `outer` -- anywhere in the class declaration,
  nested classes and the class side included. A private member is reached
  only by sends from within the declaring class (`newspeak-spec.pdf`
  §5.8, §5.9, §6.3.1); `Message>>sendTo:` is an ordinary public send
  (§5.5); no program has a reflective slot write -- only the VM mirror,
  which the shipped modules alone hold.
- **A public or protected header slot**: never; `=` would remove a setter
  code outside the class may send.

The name test is conservative: an `x::` or `x:` that resolves to something
else (a shadowing closure local, another class's slot) still counts.

Only a module that parses without error gets hints.

## The server

- `crates/ns-syntax/src/hints/` computes the hints from the AST, as the
  printer reprints it: for each hint its kind, the span of the slot's name,
  its section, its message and its text edits in byte offsets, and "apply
  all". It holds no LSP type. It lives in `ns-syntax`, not `ns-analyzer`,
  because the behaviour oracle in `ns-cli`'s tests uses it, and `ns-cli`
  never depends on `ns-analyzer`; the analysis is syntactic, so `ns-syntax`
  is all it needs.
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
  - A appends ` ::= <value>.` to the declaration, the value copied verbatim
    from the source, its continuation lines shifted by the difference in
    indentation; deletes the statement and its period, and its line when
    that leaves the line blank; and, when a local is reordered, moves its
    declaration.
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
  cases -- guards, reordering, a header prefix out of order, private against
  protected header slots, a shadowing closure local, a commented statement,
  a chain -- each fix applied and compared with the expected text
  (`indoc!` modules).
- **The behaviour oracle**: "apply all" over every `goldens/**/*.ns`, each
  result run under both lowerings and held to the `status`, `stdout` and
  `stderr` in the `.toml` beside it; each result parses, and a second
  "apply all" changes nothing. The goldens themselves are not rewritten.
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
