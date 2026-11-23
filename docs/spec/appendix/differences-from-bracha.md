# Differences from Bracha's Newspeak

Where this book's language differs from Gilad Bracha's *Newspeak
Programming Language Draft Specification*: for each difference, what
Bracha's section says, what this book says, the rule's identifier, and
why. A construct Bracha marks *Unimplemented* that ns-next does not build
is a gap of this book, listed after the differences. The parts of the
language this book has not written yet -- its semantics -- record their
differences here when they are written.

## The language id is optional

**Bracha §8.1:** `compilationUnit → languageId toplevelClass`, the
language id required.
**This book:** the language id and the class category are both optional
and have no meaning
([`syntax.language-id`](../language/grammar.md#r-syntax.language-id)).
**Why:** nothing reads the id -- Bracha's canonical parser discards it
(`NewspeakParsing.ns`, `compilationUnit`'s `wrapper: [:lid :u :e | u]`) --
and every module of the repository leaves it out. Decided with the user on
2026-11-23 (`docs/design/frontend.md`, "The module header").

## `///` begins a line comment

**Bracha §4.2:** comments are `(* … *)` only, and `binSel →
(specialCharacter | "-") specialCharacter*` admits every run of special
characters, `///`, `+///` and `<///>` among them.
**This book:** `///` begins a comment to the end of its line wherever it
stands outside a string and a block comment, and a binary selector ends
before it, so no selector contains `///`
([`lex.comment.line`](../language/lexical-structure.md#r-lex.comment.line),
[`lex.binary-selector`](../language/lexical-structure.md#r-lex.binary-selector)).
**Why:** a comment wherever it stands is what a reader expects, and the
repository's documentation comments are written in it; a selector
containing `///` is the price, and no module of the corpus has one.
Decided with the user on 2026-11-23 (`docs/design/frontend.md`,
"Comments").

## There are no character literals

**Bracha §5.1.4:** a character literal `"c"` is a `Character` of one
printable ASCII character, `""""` the double quote; the section itself
announces "We plan on doing away with character literals, and having
people use string literals instead".
**This book:** there are no character literals and no `Character` class:
`"a"` is a one-character string, a double-quoted string has no escape, and
`"""` is the string holding one quote
([`lex.string.double-quoted`](../language/lexical-structure.md#r-lex.string.double-quoted),
[`lex.string.one-character`](../language/lexical-structure.md#r-lex.string.one-character)).
**Why:** it carries out the plan Bracha announces, and `"""` is how
Bracha's own corpus writes the quote -- seven modules, among them
`corpus/modules/NewspeakGrammar.ns`, `(char: """)` -- which `""""` would
lex as an unterminated string. Decided with the user on 2026-09-25 and
adopted on 2026-11-23 (`docs/design/frontend.md`, "Lexer").

## Whitespace only separates tokens

**Bracha §6.3.3, §6.3.4:** `accessModifier` and `lazyModifier` end in
`whitespace`, so read over characters `| public= 1. |` would declare a slot
named `public` while `| public = 1. |` is refused.
**This book:** the grammar is read over tokens and whitespace never
appears in a production, so the two are the same tokens and are both
refused ([`syntax.whitespace-free`](../language/grammar.md#r-syntax.whitespace-free)).
**Why:** a meaning that hangs on a space is one no reader sees. Decided
with the user on 2026-09-27 and adopted on 2026-11-23
(`docs/design/frontend.md`, "Words that are not reserved").

## `outer N` alone is an expression

**Bracha §5.9:** an outer send is `outer`, an identifier and a message
clause; `outer N` stands only as a send's receiver.
**This book:** `outer N` with no message after it is a primary, the
enclosing object the outer send would send its message to
([`syntax.outer.alone`](../language/grammar.md#r-syntax.outer.alone)).
**Why:** Bracha's canonical Newspeak accepts it, and a method can then
hand its enclosing object on without a method of the enclosing class's
own that answers `self`. Decided with the user on 2026-09-27 and adopted
on 2026-11-23 (`docs/design/dispatch.md`, "Shapes the resolver must
recognize").

## Gaps

- **Object literals** (Bracha §5.1.10), *Unimplemented* there, are not
  parsed ([`syntax.gap.object-literal`](../language/grammar.md#r-syntax.gap.object-literal)).
- **Chains** (Bracha §5.4.4), *Unimplemented* there and without a grammar,
  are not parsed ([`syntax.gap.chain`](../language/grammar.md#r-syntax.gap.chain)).
- **Class declarations as expressions** inside a method or a closure
  (Bracha §6), *Unimplemented* there, are not parsed
  ([`syntax.gap.class-expression`](../language/grammar.md#r-syntax.gap.class-expression)).
