# The ns-next Specification

*A living document: it describes ns-next as it is today, and has no
versions or editions.*

## Introduction

This book specifies ns-next: the Newspeak language it runs, the run-time
semantics of its operations, its bytecode, its image format, and the
invariants its implementation keeps. It is self-contained. The chapter
"Overview of Newspeak" introduces the language informally; the parts
after it are normative. Gilad Bracha's
*Newspeak Programming Language Draft Specification*
(`docs/references/newspeak-spec.pdf`, transcribed in
`docs/references/newspeak-spec.md`) is its source: where ns-next follows
that document, the rule is stated again here; where that document is
silent, contradicts itself or is marked *Unimplemented*, this book says
what ns-next does; and where ns-next departs from it, this book states
ns-next's rule and the appendix "Differences from Bracha's Newspeak" says
how it differs and why. A section of Bracha's document is cited as
"Bracha §5.9".

### The parts

1. **The language**: the lexical structure, the grammar, and the meaning
   of a program.
2. **The run-time semantics**: the operations the meaning is built from,
   as abstract operations.
3. **The bytecode**: the instructions, their encoding and the rules the
   decoder checks.
4. **The image format**: the structure of an image, its tags, its
   validation, its bounds, and the operations that load and write one.
5. **The VM invariants**: the internal rules no program observes whose
   breach would make one of the first four parts false.

The first is the language every implementation of it would share; the
other four are this implementation's.

### Conventions

**Rules.** Every normative rule is preceded by its identifier, a line of
the form `r[lex.comment.line]` alone on its line and at its start; a line
that begins with `r[` is always one, and an indented `r[...]` is a
sample. An identifier is one or
more segments joined by `.`, each a lowercase letter followed by
lowercase letters, digits and `-`; its first segment names the part --
`lex` and `syntax` for the language's lexical structure and grammar, and,
as the parts are written, `semantics`, `runtime`, `bytecode`, `image` and
`vm`. An identifier is unique in the book and never reused for another
rule. In the rendered book each identifier is an anchor and lists the
tests that check its rule; the appendix "Untested rules" lists the rules
no test names yet.

**Tests name their rules.** A golden names the rules it checks in its
`.toml`, `rules = ["syntax.outer.alone"]`; a Rust test in a comment line
`// spec: syntax.outer.alone` directly above its `#[test]` -- or, for a
test `for_each_lowering!` makes under both lowerings, directly above its
function -- one line per rule, which the book lists with the test's
name. A reference to an identifier the book does not declare, or a
`// spec:` line above no test, fails the suite, and so does a link to a
chapter or a rule the book does not have.

**Gaps.** Where this book describes a construct ns-next does not build --
one Bracha marks *Unimplemented*, or one a decision left open -- the rule
says so: what the construct would be, and what ns-next does instead. A gap
is a defect the book records, not a rule it makes.

**Notation.** The grammar is a parsing expression grammar over tokens:
an optional part is taken whenever it matches, and of several alternatives
the first that matches in full wins. Beneath it a lexical grammar over
characters says what the tokens are. Semantics are written as named
abstract operations in numbered steps. Machine-readable tables stand in
fenced blocks whose info string begins with `spec-table`, and tests hold
them to the code.

**Changes.** Because the book is living, a change to ns-next that changes
observable behaviour, a format or an invariant changes the rule for it in
this book in the same commit, and a rule no longer true is rewritten,
never kept beside its successor.

The book is rendered by `cargo xtask spec` into
`target/spec/html/index.html`.
