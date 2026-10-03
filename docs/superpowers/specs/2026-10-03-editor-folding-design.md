# Code folding, comment spans and a fault-tolerant parse — design

Code folding in the JetBrains plugin: comments, class bodies, class sides,
method bodies, closures and slot sections collapse and expand. The fold
ranges come from the language server, computed from the AST, so they name
what they fold. Two things in `ns-syntax` make that possible and are built
with it: comment spans, and a parse that answers a module for every source,
so the folds — and the semantic colours and resolver errors — survive the
syntax errors a file passes through while it is typed.

One branch, three parts, in this order: comment spans and the editor entry
(part 1), the recovery (part 2), the server and the plugin (part 3).

## Part 1 — One parser, two entries; comment spans

**No second parser.** The recovery is improved inside the existing parser,
so the compiler and the editor read source through the same code. There is
no concrete syntax tree.

- `ns_syntax::parse` keeps its contract: `Err` on any error-severity
  diagnostic. Nothing that compiles ever sees a recovered module.
- `ns_syntax::parse_for_editor(source) -> EditorParse` is new, named after
  `resolve_for_editor`. It always answers:
  - `tokens` — the lexer's tokens, which the server's classification needs
    and today lexes for separately;
  - `module` — a `Module`, recovered where the source has errors;
  - `comments: Vec<Span>` — every comment, in source order;
  - `diagnostics` — the lexer's and the parser's, together.

  Lexer errors do not stop the parse here. The lexer already answers a
  complete token stream alongside its errors; an unterminated string or
  comment runs to the end of the file, as it does in the Kotlin lexer.
- A source longer than a span can address answers an empty module (a root
  class with an empty name and empty sides) and the one `source_too_long`
  diagnostic.

**Comment spans.** The lexer records the span of each comment as it skips
it. A nested comment is one span, from its outer `(*` to its outer `*)`; an
unterminated one runs to the end of the source. `lex` and `parse` drop the
list, so the runtime and the compiler see no change. The token stream is
unchanged, and with it `dump-tokens` and the comparison against the Kotlin
lexer.

The comments sit beside the AST, not in it: no consumer of the AST has to
skip them, and no rule has to decide which node a comment belongs to. A
later formatter attaches them to nodes by span; that is not part of this
work.

**Bracket spans in the AST.** A fold runs from an opening token to its
closing one, and the AST today records neither for a method body, a class
side or a slot section. `ast::Delimiters { open: Span, close: Option<Span> }`
records them: `close` is `None` when recovery found none, and the fold then
ends where the node ends. It is set on `MethodDeclaration` (the body's
`( )`), `ClassBody` (the instance side's first `(` and last `)`, the class
side's `( )`, and the header's slot section `| |`), `CodeBody` (its
temporaries' `| |`) and `BlockExpression` (`[ ]`). Nothing in the runtime
reads them.

## Part 2 — Recovery in the parser

**Principle.** On a source without errors nothing changes: every new rule
acts only where the parser already reports an error today. A missing `.`
that silently merges two statements into one valid send stays as it is — it
is a valid parse, not a recovery.

**Resynchronisation.** Two means:

- *Balanced skipping* — `(…)`, `[…]` and `{…}` are skipped whole, never to
  the first closing bracket of any kind.
- *A member start* — what opens a member of a class side: a method,
  `pattern = (` (the existing `looks_like_method`); a nested class, `class`
  and a name (`at_nested_class`); a lazy slot's modifiers and name
  (`at_lazy_slot_prefix`); each with an access modifier before it. A
  category string opens no member: ns-next's grammar has categories only at
  a module's head.

The rules:

1. **Method body.** When the body ends without its `)`: at a member start,
   report the missing `)` and end the method there, so the next member is
   read normally; otherwise skip, balanced, to the method's `)` and consume
   it. This is the case that matters most while typing — `foo = (` with an
   unfinished body keeps every following method and the class side.
2. **Statements.** After an error in a statement — a statement after `^`
   included — skip, balanced, to the next `.` or to the body's closing
   token, and go on with the next statement rather than ending the body.
3. **Class sides.** Unrecognised tokens are skipped as one run, balanced, up
   to the next member start, `)` or the end of input, with one diagnostic
   for the run instead of one per token.
4. **Parenthesised expression, block, tuple, slot section, type
   annotation.** A missing closing token is searched for balanced; the
   search stops at a member start or at the closing token of the enclosing
   body and never runs past it. `| a <Foo |` no longer swallows the rest of
   the class.
5. **No invented declarations.** A declaration whose name is missing is
   left out rather than given an empty name: a parameter, a block
   parameter, a lazy slot, a nested class, the receiver of a superclass or
   `<:` clause (the clause is left out with it). A missing factory pattern
   is `None`; a missing cascade message is left out. The one exception is a
   root class with no name: it stays, with an empty name, because without
   it there is no module, and as no class's member it collides with
   nothing. Placeholder nodes exist only for expressions (`Literal::Nil`).
   This is what lets the resolver run on a recovered module without
   reporting errors the recovery caused: none of its five errors (a
   duplicate member, a name declared as both slot and method, `outer X`
   naming no enclosing class, too many `<:` clauses, a root clause defect)
   can arise from a declaration the source does not contain.
6. **Spans.** A recovered node ends at the last token it consumed, never at
   an unrelated token after it. The fold ranges need exact ends.
7. **Depth guards** on the two recursions that lack one: a chain of setter
   keywords (`x:: x:: …`) and nested pattern literals. Both can overflow the
   stack on hostile input today, which breaks the contract that malformed
   source produces diagnostics.

**Left as it is.** An unfinished body whose next line happens to be a valid
expression (`x foo. bar = (^2)`) is read as that expression. It is a valid
parse; a heuristic over indentation or lines is rejected. The folds are
worse in such a case, never wrong.

## Part 3 — Server, plugin, tests, documentation

**Analysis (`crates/ns-lsp/src/analysis.rs`).** Per document version:
`parse_for_editor`, then `resolve_for_editor` on its module whether or not
it was recovered, then the classification and the fold ranges. The findings
are every diagnostic together — the lexer's, the parser's and the
resolver's; the rule "the lexer's, else the parser's, else the resolver's"
goes. The semantic colours survive a syntax error. The `catch_unwind` guard
stays.

**Folding (`crates/ns-lsp/src/folding.rs`, new).** The server announces
`foldingRangeProvider` and answers `textDocument/foldingRange` from the
cached analysis. A range exists only when it spans more than one line, and
runs from the opening token to the closing one inclusive, with line and
UTF-16 character positions through the existing line index.

| Range | From — to | `collapsedText` | `kind` |
|---|---|---|---|
| Class body (instance side) | the first `(` after `=` to the instance side's `)`, header and member groups together | `(…)` | `region` |
| Class side | `(` to `)` after `:` | `(…)` | `region` |
| Method body | `(` to `)` | `(…)` | `region` |
| Closure | `[` to `]` | `[…]`, or `[:x :y \| …]` with parameters | `region` |
| Slot section | `\|` to `\|` (or `\|\|`) | `\|…\|` | `region` |
| Comment | `(*` to `*)` | its first line, then `…*)` | `comment` |

Multi-line parenthesised expressions and tuples do not fold. Nothing is
collapsed by the server; whether a range starts collapsed is the platform's
decision from `kind` and the IDE's folding settings.

**Plugin.** The platform registers `LspFoldingBuilder` for every language
(`lang.foldingBuilder language=""` in `intellij.platform.lsp.impl.xml`), and
`LspCustomization`'s default `foldingRangeCustomizer` is
`LspFoldingRangeSupport`, in 2026.1.4 as in 2026.2.3 — so no Kotlin is
expected. This is checked by hand under `runIde`; should the platform need
folding enabled for a language with a parser definition of its own, that
goes into `NewspeakLspClientDescriptor`'s `lspCustomization`. `plugin.xml`'s
description and change notes name folding. The Kotlin lexer does not change.

**Tests.**

- `ns-syntax`:
  - comment spans — nested, unterminated, `(*` inside a string;
  - one recovery test per typical in-progress edit — half a method
    (`foo = (` unfinished), a statement after `^`, an unclosed block, a
    nested class missing its `)`, an unclosed slot section, `| a <Foo |`,
    half a keyword pattern — each asserting that the members *after* the
    error survive, with their names and spans;
  - the depth guards — a long setter chain and deeply nested pattern
    literals answer diagnostics, not an overflow.
- `ns-lsp`, over `corpus/modules`, `goldens/` and `Kernel.ns`:
  - on a clean source `parse_for_editor` answers the same module as
    `parse`, and no diagnostics;
  - on every truncation at a token boundary (the existing robustness
    sets) no "internal error" — now through the resolver, the
    classification and the folding as well;
  - on those truncations, no declared name is empty except a root class's.
- Folding: one module per range kind; a single-line construct answers no
  range; the ranges after a half-typed method survive; the protocol test
  requests `textDocument/foldingRange` and checks the announced capability.
- The analysis tests that expect only the lexer's errors on a source with
  lexer errors are updated to the combined findings.

**Documentation.** `docs/design/editor-support.md` (scope, protocol
surface, analysis, folding, testing; "Planned later" no longer lists the
fault-tolerant parser as navigation's missing prerequisite),
`docs/design/frontend.md` (the recovery rules, `parse_for_editor`, comment
spans), `docs/OPEN-WORK.md` (navigation has its prerequisite; formatting's
comment part is done), `docs/PROGRESS.md`.

**Gates.** The per-commit gates of `CLAUDE.md`, with
`cargo nextest run -p ns-lsp` and `./editors/jetbrains/gradlew -p
editors/jetbrains check` for every commit touching `ns-syntax`, `ns-lsp` or
the plugin; at the end, the `finishing-a-batch` skill, `verifyPlugin`
included.
