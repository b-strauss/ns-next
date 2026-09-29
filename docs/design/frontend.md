# Frontend — lexer, parser, AST

> **Status:** built.
> **Code:** `crates/ns-syntax/` — lexer, tokens, AST, recursive-descent and
> Pratt parser, `Span`, diagnostics.

Turns source bytes into a `Module` AST with source spans, and reports lexer and
parser diagnostics. It has no dependency on `ns-runtime`, and nothing here should
change for runtime work.

## What a source file looks like

```
Newspeak3
'CategoryString'
class Name usingPlatform: platform = SuperclassClause (
    (* optional class comment *)
    |
    private Foo = platform collections Foo.
    bar ::= 0.
    |
    initExpr1.
    initExpr2.
) (
    public method = ( ^expr )
) : (
    public classSideMethod = ( ... )
)
```

Three bracketed sections in order: the class body (slots and initializer
expressions), the instance side, and — after `) : (` — the class side.

## Grammar

**The syntax is `newspeak-spec.pdf`'s grammar, strictly** (decided with the
user on 2026-09-27): every production of §4.2 to §8.1 is accepted as written,
and what it does not derive is refused. The canonical combinator grammar,
`NewspeakGrammar.ns`, vendored into `corpus/reference/`, is the reference
for what the specification leaves open or gets wrong, and nothing more.
`parser::declaration`'s `specification_tests` pin one rule each, with the
forms it accepts and those it refuses; the goldens named `spec_4_2_*`,
`spec_5_*`, `spec_6_*` and `spec_8_1_*` run the accepted ones.

**How the grammar is read.** §4.2 calls it "a PEG (Parser Expression Grammar)
with no distinction between scanning and parsing", and the specification
writes `classDeclaration` as two spelled-out alternatives precisely because an
optional part, once matched, is never retried as absent. The parser reads it
that way -- of several alternatives the first that matches in full wins, and
an optional part is taken whenever it matches -- over the lexer's tokens, not
over characters. Tokens are what a reader sees, and reading characters would
make spacing matter where nobody means it to; the one place that shows is
recorded as a deviation ("Words that are not reserved"). Where a rule is
ambiguous between a token and a production, the parser tries the production in
full and restores the cursor when it fails (`Parser::checkpoint`/`restore`),
as `at_lazy_slot`, `at_binary_equals_factory` and `parse_slot_declarations` do.

**Where the specification contradicts itself or cannot be meant literally**,
its own prose decides, and otherwise the canonical grammar:

- `expression → [setterKeyword] sendExpression` allows one setter keyword,
  but §5.4.3's rationale is "to enable chaining of setter sends, e.g.,
  `w::x::y`", and the canonical grammar has `setterKeyword star`: a chain is
  accepted.
- `slotDecls → seqSlotDecls | simSlotDecls` tries the sequential clause
  first, which read over characters matches every `||` as an empty clause and
  leaves the simultaneous one unreachable, though §6.3.2 defines it. The
  canonical grammar tries `simSlotDecls` first; so does the parser, falling
  back to the empty sequential clause when no simultaneous one matches
  (`[:x | || x + 1]`).
- `comma → ""` and the quoting inside `specialCharacter`, `twoDblQuotes` and
  `wildcardPattern` are transcription damage; the lexer reads them as a comma,
  a backslash, a doubled quote and an underscore.

The productions, and what the parser does with each:

- `compilationUnit → languageId toplevelClass`, `toplevelClass →
  classCategory classDeclaration`, `classCategory → [string]` (§8.1): the
  category is optional, and the module's class takes no access modifier --
  only `nestedClassDecl → [accessModifier] classDeclaration` has one.
- `classDeclaration` (§6): `class`, a name, an optional `typeParameterDecl`
  (`<[K, V]>`, dropped, "What is parsed and dropped"), then the factory's
  `messagePattern` and `=`, or `=` alone -- tried in that order, so
  `class Pair = other = Object new () ()` has the binary factory `=`.
- `inheritanceClause → [inheritancePrefix] unarySelector [message]` (§6.1):
  a prefix (`outer N`, `self` or `super`), the receiver, and at most one
  message -- unary, binary or keyword, whose arguments may be any binary or
  unary expressions. `Object new foo` is refused.
- §6.2's mixin application follows an explicit superclass clause:
  `mixinAppSuffix → (mixinOperator inheritanceClause)+ (dot | classBody)`,
  so `class C = S ms <: M1 m1 <: M2 m2 ( … )`, or the same ending in `.`
  with no body at all. `mixinOperator` is `<:`, one lexer token; each clause
  is parsed by the superclass clause's own `parse_superclass_clause` and kept,
  in written order, in `ClassDeclaration::mixin_applications`. What the
  runtime builds from it is [object-model.md](object-model.md), "Mixin
  application". Inside a `<...>` type body or a `where { ... }` clause `<:`
  is the bound operator (`<Foo <: Bar>`) and is dropped with it.
- `classBody → classHeader sideDecl [classSideDecl]` (§6.3): the instance
  side is required, however empty -- `class C = () ()` -- and the class side
  optional. `classHeader → lparen [classComment] [slotDecls] initExprs
  rparen`, `initExprs → [expression (dot expression)* [dot]]`: initializer
  expressions separated by single dots. `sideDecl → lparen (nestedClassDecl |
  lazySlotDecl | methodDecl)* rparen` and `classSideDecl → colon lparen
  methodDecl* rparen` have no separator: a dot between members is refused.
- `slotDecls → '|' slotDef* '|'` (sequential) or `'||' slotDef* '||'`
  (simultaneous), in a class header and as a code body's temporaries alike.
  A simultaneous clause's declarations carry `simultaneous`, which the
  resolver and the compiler act on ([object-model.md](object-model.md),
  "Simultaneous slots").
  `slotDef → [accessModifier] slotDecl [((= | ::=) expression dot) | dot]`:
  `=` is an immutable slot, `::=` a mutable one, and neither an uninitialized
  mutable one; the dot is mandatory after an initializer. A lazy slot
  (`lazy m ::= e.`, §6.3.3) parses into a `SlotDeclaration` whose `lazy` is
  set; its dot is mandatory too. The runtime initializes it on first read
  (`docs/design/core-library.md`, "Lazy slots first").
- `methodDecl → [accessModifier] messagePattern equalSign lparen codeBody
  rparen` (§6.3.4), with `messagePattern` a unary selector, any binary
  selector and a parameter (`||`, `=`, `<` and `->` included), or keywords and
  parameters -- keywords only, never a setter keyword.
- `codeBody → [temporaries] statements` (§5.1.8), `statements →
  returnStatement | statementSequence | empty`, `statementSequence →
  expression [dot statements]`, `returnStatement → hat expression [dot]`
  (§7.2): statements separated by single dots, a trailing one allowed,
  nothing after a return, and no dot that ends no statement.
- `block → lbracket [blockParameters] codeBody rbracket`,
  `blockParameters → (colon slotDecl)+ vbar` (§5.1.8).
- Expressions, §5.4.3 and §5.4.4: `expression → setterKeyword*
  sendExpression` (a setter keyword stands at an expression's head and
  nowhere else), `sendExpression → implicitKeywordSend |
  cascadedMessageExpression`, `cascadedMessageExpression → primary
  [msgCascade]`, `msgCascade → nonEmptyMessages cascadeMsg*`: a cascade
  follows a primary and at least one message to it, never an implicit-receiver
  keyword send and never a parenthesized send alone, and each `;` part is a
  unary, binary or keyword message. Unary binds tighter than binary, binary
  than keyword, and `=`, `<`, `>`, `|` and `||` are binary selectors wherever
  a message can stand, since §4.2's `specialCharacter` includes them. Like the
  PEG's `binaryMsg*`, a selector with no operand after it is not taken, so a
  slot section's closing `|` stays one.
- `primary → unarySelector | literal | block | parenthesizedExpression`, and
  `literal → pattern | number | symbol | characterConstant | string | tuple`
  (§5.1): the character constant is a one-character string ("Lexer"), and
  `tuple → lcurly [expression (dot expression)* [dot]] rcurly` is the array
  literal. Smalltalk's literal array `#( ... )` is not Newspeak and the lexer
  refuses it by name.
- `pattern → "<" patternLiteral ">"` (§5.1.9): `_`, a number, symbol, string
  or tuple, or keywords each with an optional value -- `_`, a literal,
  `?name` or a nested pattern (`<x: <y: 1>>`, whose closing `>>` the parser
  splits). Parsed in full and kept as its text; what it evaluates to is a
  deviation ("What is parsed and dropped").
- `super` is followed by a message (§5.10) and `outer` by a class's name
  (§5.9), both at the parser. `outer N` with no message after it is the
  enclosing object ([dispatch.md](dispatch.md), "Shapes the resolver must
  recognize"). A bare `super`, which the parser accepted and the runtime
  raised on only when it ran, is a syntax error.
- comments are `(* ... *)` and nest; `(* :literalmessage: ... *)` and
  `(*:id: ... *)` metadata comments are comments.

## Lexer

Hand-written, single pass. It handles nested `(* *)` comments, literal-message
comments, both kinds of string literal, string escapes and radix numbers.

**A string literal is `'...'` or `"..."`** (§5.1.5), and there are no
character literals (decided with the user on 2026-09-25): §5.1.4 announces
"doing away with character literals, and having people use string literals
instead", so `"a"` is a one-character string and the `Character` class is
gone. A single-quoted string doubles an embedded `'` (`'it''s'`). A
double-quoted one has no escape and ends at the next `"`
(`lex_double_quoted_string`), so each kind holds the other's quote freely —
`'"'`, `"'"`. The one exception is the one-character form: any single
character between two quotes is a one-character string, `"` included, so
`"""` is the string holding a quote. That is how the corpus writes it, in
seven modules (`corpus/modules/NewspeakGrammar.ns`, `(char: """)`), which the
old character lexer read as one character whatever it was; §5.1.4's own
doubling, `""""`, would have lexed those as an unterminated string. `""""`
therefore reads as `"""` followed by the start of another string, and appears
in the corpus only inside comments.

**A string literal is not normalized to NFC**, a recorded deviation
([README.md](README.md), "Deviations from the language specification"):
§5.1.5 makes a literal's value "the encoding of the string in Unicode normal
form C", and the lexer keeps the code points the source text holds, so a
decomposed `é` stays two code points and differs from a precomposed one. No
code anywhere in the workspace normalizes. Closing it means normalizing where
source text becomes a string. It is not on the road to a high-performance VM
([OPEN-WORK.md](../OPEN-WORK.md)); the Unicode normalization data it needs is
to come from the `unicode-normalization` crate (noted with the user on
2026-09-29 for when it is taken up). The planned string representation
([object-model.md](object-model.md), "Planned: the object layout") stores
whatever code points it is given and does not depend on it.

**Numbers, selectors and symbols follow §4.2's lexical grammar.**

- *Negative numbers.* `decimalNum → ["-"] digits ...` has a sign, and
  `binSel → (specialCharacter | "-") specialCharacter*` makes `-` a binary
  selector: which one a `-` before a digit is depends on where it stands,
  which only the parser knows. The lexer always reads `-` against a digit as a
  negative number, and the parser splits it into `-` and the number where the
  grammar has a binary message (`Parser::split_negative_number`): `3 -4`,
  `3-4` and `x -1` subtract, `foo: -4` and `{-4}` hold a negative number,
  `3 - -4` is seven. An earlier rule decided by the whitespace before the `-`
  instead, and read the corpus's `sd start -1` as a dangling literal.
- *Binary selectors.* `-` starts a selector and continues none, since it is
  not a `specialCharacter`: `+-4` is `+` and a negative four, `->` one
  selector. §5.6's asynchronous send token `<-:` is one token of its own,
  `EventualSendOperator`; `<:` is §6.2's `MixinOperator`.
- *Radix numbers.* Each optional part is taken only when it is complete: `16r`
  is `16` followed by the name `r`, a fraction needs a digit after its dot,
  and only `decimalNum` has a leading sign, so `-16rFF` is `-16` and `rFF`
  while `16r-1F` is -31. Extended digits are uppercase. §5.1.1 gives a value
  only to a radix between 2 and 36 and digits below it; the parser refuses
  the rest (`2r12`, `37r1`), as the canonical parser does while it parses.
  A fraction or an exponent makes any number non-integral, a radix one
  included: `16r1.8` is 1.5 and `2r1e3` is 1000, its digits in its radix
  times ten to its exponent -- the canonical parser's reading, since the
  section gives an exponent no base
  (`goldens/spec/spec_5_1_1_a_radix_number_may_have_a_fraction_and_an_exponent`).
- *Symbols.* `sym → str | kws | binSel | id`, `kws → kw+`: every keyword of a
  symbol ends in its colon, so `#at:put:` is one symbol and `#at:put` is
  `#at:` followed by `put`.
- *No literal arrays.* `#( ... )` is refused by name
  (`` `#( ... )` is not Newspeak syntax; a tuple is written `{ element. element }` ``):
  neither the specification nor the canonical grammar has Smalltalk's literal
  array, and the tuple is §5.1.7's array literal (decided with the user on
  2026-09-27). Its image literal tag, 7, is retired
  ([image-format.md](image-format.md)).

Comments are retained as tokens for tooling and skipped by the parser.

## AST

`Module`, `ClassDeclaration`, `ClassHeader`, `SuperclassClause`,
`SlotDeclaration { name, mutability, initializer }`, `MethodDeclaration { pattern, body }`,
`MessagePattern`, `CodeBody { temporaries, statements }`, `Statement`, `Expression` (`Identifier`,
`Literal`, `Block`, `Send`, `Cascade`, `Assign`, `Array`), `Literal`.

**Every node carries a `Span`** — byte offsets into the source, as
`Span { start: u32, end: u32 }`. Spans are how the resolver attaches bindings,
how errors point at source, and how provenance is recorded. The compiler looks
each binding up by its node's span once and bakes the resolution into an
instruction, so nothing at run time consults a span to find a binding.

## What is parsed and dropped

Typed-Newspeak declaration syntax parses and is discarded, because there is no
type AST: `<Type>` annotations on parameters and slots, `^<Type>` return
annotations, `[T]` type parameters on class names and selectors and type
arguments on a superclass or `<:` clause's receiver (`<: Collection[E]`), and
`where { ... }` constraint clauses. §4.3 acknowledges this "built-in optional
type annotation syntax based on the Strongtalk type system" without giving it
a grammar, so the canonical grammar's is followed, and §6's own
`typeParameterDecl`, `class Map <[K, V]> new: capacity <Integer>`, is
accepted beside the canonical `class List[E]` (decided with the user on
2026-09-27). The eventual-send operator `<-:` is likewise
accepted and dropped, which makes an asynchronous send (§5.6) run as an
ordinary synchronous one and answer the value where the specification gives a
promise. Nothing in the runtime implements actors, so there is no promise to
answer with; the alternative would be to refuse the operator outright.

A pattern literal (§5.1.9) is parsed in full and kept as its source text,
but lowers to **nil**: the section makes `<_>` `Pattern wildcard` and `<l>`
`Pattern literal: l`, and there is no `Pattern` class for either to reach.

**Planned (noted with the user on 2026-09-28): lower a pattern literal to
the send §5.1.9 defines.** `<_>` becomes `Pattern wildcard`, `<l>` becomes
`Pattern literal: l`, and a keyword pattern `<x: 1 y: ?z>` becomes
`Pattern keywords: kws patterns: pats`, with `kws` the list of its keyword
symbols and `pats` the list of its values -- `_`, a literal, a `?name`
variable pattern or a nested pattern, each lowered the same way. `Pattern` is
an implicit-receiver send, so whatever the environment binds it to answers:
by default `Object`'s accessor to the library's class, and until a library
provides one, a clean `doesNotUnderstand: #Pattern` instead of a silent
`nil`. This needs a pattern AST in place of today's source text, which
`parse_pattern_literal` already walks; the library side has prior art in the
corpus, `corpus/modules/NSPatternMatching.ns` (`<val: ?y> => [ y ]`).
Closing it retires the deviations-table row.

Two of the things dropped here -- the asynchronous send and the pattern
literal -- change what a correct program answers, rather than only what an
incorrect one is allowed to say. Each has a row in [README.md](README.md)'s
deviations table; this section is what those rows point at.

A code body's temporary list has the full slot-declaration form
(`corpus/reference/NewspeakGrammar.ns:148-150`, `temporaries = slotDecls`), so
`| x = expression. |` and `| x ::= expression. |` inside a method or block body
declare initialized local slots, and `CodeBody.temporaries` keeps the full
declaration -- name, mutability and initializer -- not just the name.

## The six reserved words

`newspeak-spec.pdf` §4.1 lists `self`, `super`, `outer`, `true`, `false` and
`nil` as reserved words, illegal as ordinary identifiers. The lexer keeps them
identifiers, as the canonical grammar does (`identifier = tokenFor: id`):
`true`, `false` and `nil` are recognized as literals, and `self`, `super` and
`outer` contextually by name at the parser and resolver level. Expression-level
`outer N foo` parses correctly, checked against
`corpus/modules/AccessModifierTesting.ns`, which uses it heavily. `super` is
followed by a message and `outer` by a class's name, or the parser refuses
them (§5.10, §5.9; decided with the user on 2026-09-27); a bare `super`, which
the runtime once raised on only when it ran, never reaches it.

**The parser refuses every other use** (decided with the user on
2026-09-25): a reserved word naming a parameter, a block parameter, a slot or
temporary, a class or a method, the target of a setter send (`nil:: 3`), or a
unary selector (`3 nil`, `3 printString; self`) is an error naming what it
may not be — `` `nil` is a reserved word and cannot name a parameter (§4.1) ``
(`Parser::refuse_reserved_word`). Until then those programs were accepted, and
not harmlessly: `foo: nil = ( ^nil )` answered the literal `nil` whatever
argument it was sent. A keyword that merely begins with one, `ifNil:` or
`selfish:`, is not an identifier and is unaffected. No corpus module used the
illegal forms; one golden and one test did, and were renamed.

## Words that are not reserved

§4.1 reserves the six words above and nothing else, so `class`, `public`,
`protected`, `private` and `lazy` are identifiers like any other (decided
with the user on 2026-09-27: the syntax follows the specification strictly).
The lexer gives the first four tokens of their own, `Class`, `Public`,
`Protected` and `Private`, so the declarations that start with one can find
it, and leaves `lazy` an `Identifier`; `parser::is_name` is the one place
that says all of them are names. Wherever the grammar takes an
`identifier` -- a name read, written or sent, a parameter, a block
parameter, a class, a superclass clause's receiver, `outer`'s class name, a
literal array's bare word, the `languageId` -- any of them is accepted.

Where a word could also begin a declaration, the grammar decides, and the
grammar is a PEG (§4.2, and the specification's own `classDeclaration`
spells its two alternatives out for exactly this reason): an optional part
is taken whenever it matches and never retried as absent, and of several
alternatives the first that matches in full wins. So:

- `slotDef` and `methodDecl` start with `[accessModifier]`. An access
  modifier's word standing first is the modifier, so it names a slot, a
  temporary or a method only behind one: `| public public = 1. |` and
  `public private = ( ... )` are accepted, `| public |` and
  `public = ( ... )` are refused. `class` and `lazy` need no modifier.
- `sideDecl` tries a nested class, then a lazy slot, then a method.
  `[accessModifier] class <name>` is a nested class, whatever the name;
  `public class = ( ... )` is a method named `class`. §6.3.3's
  `lazySlotDecl` is `((lazy [accessModifier]) | (accessModifier lazy))
  slotDecl (= | ::=) expression dot`, and it is a lazy slot only if all of
  that matches: `lazy lazy = 1.` is a lazy slot named `lazy`, `lazy public
  public = 1.` a public one named `public`, while `lazy = ( ... )` and
  `private lazy = ( ... )` are methods named `lazy`, and `lazy public = 1.`
  is refused (`Parser::at_lazy_slot`).

**One deviation, decided with the user on 2026-09-27:** the grammar's
`accessModifier` and `lazyModifier` end in `whitespace`, so taken literally
`| public= 1. |` would declare a slot named `public` beside a refused
`| public = 1. |`. With a lexer the two are the same tokens and are treated
alike, both refused. [README.md](README.md)'s deviations table records it.

`parser::declaration`'s `unreserved_word_tests` pin every position, the
refused forms included; `goldens/spec/spec_4_1_the_unreserved_words_*` and
`spec_6_3_3_the_unreserved_words_name_lazy_slots` run each accepted one.
§5.9's `outer N` takes any identifier for `N`, and one naming no enclosing
class is the compile-time error §5.9 makes it ([dispatch.md](dispatch.md)).

## Possible future extensions

Two constructs the specification describes and itself marks
*Unimplemented* are not parsed, by decision (2026-09-27), and would each be
a design of its own rather than a conformance fix:

- **Chains** (§5.4.4), `e :| μ`, equivalent to `(e) μ`: `label: "foo" :|
  color: Color red` is `(label: "foo") color: Color red`, sugar for sending
  to the previous result without parentheses, "cascades for functional
  programming". The section gives no grammar, leaves open what `μ` may be
  and how a chain meets a setter send or a cascade, and ends "It's an open
  question if chains stay in the language". No prior art implements them.
  The parser would resolve them into ordinary sends; `:|` would be a new
  token in both lexers.
- **Object literals** (§5.1.10), `objectLiteral → [identifier [keywordMsg]]
  classBody`: an anonymous class declaration that evaluates to a new object,
  a new class per evaluation over one shared mixin, whose enclosing object is
  the current *activation* -- so it sees the method's locals, which no
  enclosing object in this runtime does today. The production is reachable
  from no other, so the specification does not say where one may stand, and
  it is ambiguous with a parenthesized expression and a keyword send; SOMns,
  the one prior art that implements it, prefixes it with its own keyword,
  `objL()( ... )`, for exactly that reason.

## `:=` is refused

§5.4.3 gives Newspeak exactly one assignment form, the setter send `id:: e`
(desugaring to `id: e`, "Setter Sends"), which `parse_expression` reads at an
expression's head as an `Expression::Assign` ([dispatch.md](dispatch.md),
"Setter sends"). Nothing in the specification grammar, the canonical grammar
(`corpus/reference/NewspeakGrammar.ns`), Primordial Soup or SOMns accepts
`:=`, Smalltalk's assignment. The lexer refuses it by name — one diagnostic,
`` `:=` is not Newspeak syntax; a setter send is written `name:: expression` ``
— rather than splitting it into `:` and `=` and letting the parser report
something unrelated further on. Until 2026-09-19 it was accepted as a synonym
for the setter send, a deviation that only over-accepted; removing it
converted every golden and test that spelled it, with no recorded outcome
moving ([PROGRESS.md](../PROGRESS.md), "M4 — Removing `:=`").

## The corpus as a parser gate

`corpus/` holds every `.ns` file from `newspeaklanguage/newspeak` at a fixed
upstream commit, recorded with its URL in `corpus/PROVENANCE.md`. It is a manual
snapshot rather than a submodule or subtree: refreshing it is a deliberate,
reviewed action, and the upstream license travels with the files.

`ns-next parse-corpus corpus/modules` must report **parsed 211, failed 0**,
and `crates/ns-syntax/tests/corpus.rs` holds every module to the same. There
is no list of exceptions (removed 2026-09-27 with its last entry, decided
with the user): the corpus is real Newspeak and the parser follows the
specification's grammar, so a module that fails to parse is either a parser
defect or a program the specification refuses, and either way a decision to
take and record here, not a file to set aside.

## Errors

Lexer and parser collect `Diagnostic { span, message, severity }` and report all
of them. Malformed input is never a panic.
