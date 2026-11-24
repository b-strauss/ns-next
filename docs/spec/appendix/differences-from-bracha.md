# Differences from Bracha's Newspeak

Where this book's language differs from Gilad Bracha's *Newspeak
Programming Language Draft Specification*: for each difference, what
Bracha's section says, what this book says, the rule's identifier, and
why. A construct Bracha marks *Unimplemented* that ns-next does not build
is a gap of this book, listed after the differences, and so is every
other construct Bracha requires that ns-next does not build yet.

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

## A non-integral literal is a binary float

**Bracha §5.1.1:** "Non-integral numbers denote an object that obeys the
protocol of class `FixedPointNumber`", an exact decimal.
**This book:** `2.5` and `1e3` denote the binary64 `Float` nearest their
value ([`semantics.literal.float`](../language/semantics.md#r-semantics.literal.float)),
so `(0.1 + 0.2) = 0.3` answers `false`; an exact decimal is a class of
its own, `Decimal`, with a literal of its own, `2.50s`, which is a gap
([`semantics.gap.decimal`](../language/semantics.md#r-semantics.gap.decimal)).
**Why:** a non-integral literal is a float in every language a reader
comes from, a float is what numeric code needs to be fast, and an exact
decimal is better asked for where it is meant; Bracha's own
implementation makes such a literal a `Fraction` and calls it "a bug".
Decided with the user on 2026-11-23 (`docs/design/specification.md`, "The
second batch's deviations", decision 1).

## A string literal holds the code points it is written with

**Bracha §5.1.5:** a string literal's value is "the encoding of the
string in Unicode normal form C".
**This book:** a string literal holds the code points its source text
holds, so a decomposed `é` is two code points and differs from a
precomposed one
([`semantics.literal.string`](../language/semantics.md#r-semantics.literal.string)).
**Why:** no language normalizes its literals, nor do Bracha's
implementations; normalizing only literals leaves every string built at
run time as it was, which is where decomposed text comes from, and the
tables cost about 127 KB of the binary. Decided with the user on
2026-11-24 (`docs/design/specification.md`, "The second batch's
deviations", decision 2).

## A module root's clauses are checked before anything runs

**Bracha §6.1:** a top-level class's inheritance clause is evaluated "as
if it was a top level expression", where it "will almost certainly
fail" -- when it is evaluated.
**This book:** a module root may write only the seven clauses of
`Object` and `Value` that work, and any other is a resolve error before
anything of the program runs
([`semantics.module.root-rule`](../language/semantics.md#r-semantics.module.root-rule)).
**Why:** a root has no enclosing object, so the only classes its clause
can reach are the ones the rule names, and a program must not run when
its artifact cannot be built -- left to evaluation, a module nothing
instantiated ran under `run` while `build` refused it. Decided with the
user on 2026-09-30 and adopted on 2026-11-24
(`docs/design/object-model.md`, "Mixin application", "The root rule").

## Gaps

- **Exact decimals** (Bracha §5.1.1): the class `Decimal` and its
  literal are decided and not built
  ([`semantics.gap.decimal`](../language/semantics.md#r-semantics.gap.decimal)).
- **Immutable tuples** (Bracha §5.1.7): a tuple literal is a mutable
  `Array` until `ImmutableArray` is built
  ([`semantics.gap.immutable-array`](../language/semantics.md#r-semantics.gap.immutable-array)).
- **Pattern literals** (Bracha §5.1.9) signal an error when evaluated
  ([`semantics.gap.pattern`](../language/semantics.md#r-semantics.gap.pattern)).
- **Actors** (Bracha §3.7, §5.6): there are no actors, promises or far
  references, and an asynchronous send signals an error when it runs
  ([`semantics.gap.actors`](../language/semantics.md#r-semantics.gap.actors)).
- **Tail calls** (Bracha §3.6), *Unimplemented* there, are not
  eliminated ([`semantics.gap.tail-calls`](../language/semantics.md#r-semantics.gap.tail-calls)).
- **Reflection** (Bracha §8.2): several builders' edits are not installed
  as one, and a superclass, the nested classes and a class from source
  are not edited
  ([`semantics.gap.reflection.several-builders`](../language/semantics.md#r-semantics.gap.reflection.several-builders));
  no mirror changes the class of an individual object
  ([`semantics.gap.reflection.object-class`](../language/semantics.md#r-semantics.gap.reflection.object-class))
  or reaches an activation
  ([`semantics.gap.reflection.activation`](../language/semantics.md#r-semantics.gap.reflection.activation)).
- **Object literals** (Bracha §5.1.10), *Unimplemented* there, are not
  parsed ([`syntax.gap.object-literal`](../language/grammar.md#r-syntax.gap.object-literal)).
- **Chains** (Bracha §5.4.4), *Unimplemented* there and without a grammar,
  are not parsed ([`syntax.gap.chain`](../language/grammar.md#r-syntax.gap.chain)).
- **Class declarations as expressions** inside a method or a closure
  (Bracha §6), *Unimplemented* there, are not parsed
  ([`syntax.gap.class-expression`](../language/grammar.md#r-syntax.gap.class-expression)).
