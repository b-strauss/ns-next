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

The productions are transcribed from `NewspeakGrammar.ns`, a combinator grammar
in the official repository, vendored into `corpus/reference/` so the parser can
be checked against it. The load-bearing ones:

- `compilationUnit = languageId , classCategory , classDecl , eoi`. The
  category string is optional in the grammar (`corpus/reference/NewspeakGrammar.ns`
  line 202, `classCategory = string opt.`) and in §8.1
  (`classCategory → [string]`); the parser requires one and refuses a file
  without it with "expected a category string". An over-rejection, and a
  loud one: every corpus module and every golden carries a category, so
  nothing exercises the optional form.
- `classHeader = accessModifier? , classReservedWord , classId ,
  messagePattern? , '=' , superclassClause? , '(' , classComment? ,
  slotDecls? , initExprs , ')'`
- `superclassClause = superclassPrefix? , unaryMsg , message?`, where
  `superclassPrefix` is `outer <id>` | `self` | `super`
- §6.2's mixin application follows an explicit superclass clause:
  `mixinAppSuffix = (mixinOperator inheritanceClause)+ (dot | classBody)`,
  so `class C = S ms <: M1 m1 <: M2 m2 ( … )`, or the same ending in `.`
  with no body at all. `mixinOperator` is `<:`, one lexer token; each clause
  is parsed by the superclass clause's own `parse_superclass_clause` and kept,
  in written order, in `ClassDeclaration::mixin_applications`. What the
  runtime builds from it is [object-model.md](object-model.md), "Mixin
  application". Inside a `<...>` type body or a `where { ... }` clause `<:`
  is the bound operator (`<Foo <: Bar>`) and is dropped with it. No corpus
  module writes `<:` in a superclass clause, so `parse-corpus` does not
  exercise the form; `goldens/mixin/` does.
- `slotDecls = '|' slotDef* '|'` (sequential) or `'||' slotDef* '||'`
  (simultaneous). A `slotDef` is
  `accessModifier? slotDecl (('=' | '::=') expression)? '.'` — `=` is an
  immutable slot, `::=` a mutable one, and neither means an uninitialized
  mutable slot. A lazy slot declaration on the instance side
  (`lazy m ::= e.`, §6.3.3) parses, but the parser drops `lazy` and adds the
  slot to the ordinary slots: it is initialized eagerly, a deviation from
  §6.3.3's "initialized lazily, upon first use".
- `messagePattern = unarySelector | (binarySelector slotDecl) |
  (keyword slotDecl)+`
- `methodDecl = accessModifier? , messagePattern , '=' , '(' , codeBody , ')'`
- `block = '[' , blockParameters? , codeBody , ']'`;
  `blockParameters = (':' slotDecl)+ '|'`
- `literal = pattern | number | symbolConstant | string | tuple` — the
  grammar's `characterConstant` is gone ("Lexer" below); `tuple = '{' (expression sepOrTermBy '.') '}'`
- comments are `(* ... *)` and nest; `(* :literalmessage: ... *)` is a
  literal-message comment

**Where the grammar and the language specification disagree, the grammar wins
for syntax and `newspeak-spec.pdf` wins for semantics.** The specification is
explicitly a draft and says so in its own front matter. Where the vendored
corpus's actual behaviour and the specification text disagree, prefer the
corpus and note the discrepancy.

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
`where { ... }` constraint clauses. The eventual-send operator `<-:` is likewise
accepted and dropped, which makes an asynchronous send (§5.6) run as an
ordinary synchronous one and answer the value where the specification gives a
promise. Nothing in the runtime implements actors, so there is no promise to
answer with; the alternative would be to refuse the operator outright.

Two of the things dropped here change what a correct program answers, rather
than only what an incorrect one is allowed to say. Each has a row in
[README.md](README.md)'s deviations table; this section is what those rows
point at.

A code body's temporary list has the full slot-declaration form
(`corpus/reference/NewspeakGrammar.ns:148-150`, `temporaries = slotDecls`), so
`| x = expression. |` and `| x ::= expression. |` inside a method or block body
declare initialized local slots, and `CodeBody.temporaries` keeps the full
declaration -- name, mutability and initializer -- not just the name.

## Deviation: simultaneous slot clauses initialize sequentially

§6.3.2's `|| ... ||` declares slot clauses that are evaluated simultaneously,
resolving well-founded mutual recursion between them. The parser computes the
`simultaneous` flag only to choose the closing delimiter and then discards it,
so the clauses initialize in declaration order like ordinary ones: in
`|| first = second + 1. second = 2. ||`, `second` is still **nil** when `first`
is computed. Where the expressions do not raise, this too is silent wrong
output at exit status 0.

## The six reserved words

`newspeak-spec.pdf` §4.1 lists `self`, `super`, `outer`, `true`, `false` and
`nil` as reserved words, illegal as ordinary identifiers. The lexer keeps them
identifiers, as the canonical grammar does (`identifier = tokenFor: id`):
`true`, `false` and `nil` are recognized as literals, and `self`, `super` and
`outer` contextually by name at the parser and resolver level. Expression-level
`outer N foo` parses correctly, checked against
`corpus/modules/AccessModifierTesting.ns`, which uses it heavily.

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

`ns-next parse-corpus corpus/modules` must report **parsed 210, failed 0,
expected-fail 1, stale 0**. A file that legitimately cannot parse yet belongs in
`corpus/blacklist.toml` with a stated reason, and the `stale` count is what
keeps that list from accumulating entries that have quietly started working.

The one blacklisted file is a lexer ambiguity rather than a missing construct:
source that writes `expression -1` meaning `expression - 1`, where the lexer's rule makes
`-1` a negative literal. One file, `NewspeakColorizationForSqueak.ns`.

## Errors

Lexer and parser collect `Diagnostic { span, message, severity }` and report all
of them. Malformed input is never a panic.
