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
- `literal = pattern | number | symbolConstant | characterConstant | string |
  tuple`; `tuple = '{' (expression sepOrTermBy '.') '}'`
- comments are `(* ... *)` and nest; `(* :literalmessage: ... *)` is a
  literal-message comment

**Where the grammar and the language specification disagree, the grammar wins
for syntax and `newspeak-spec.pdf` wins for semantics.** The specification is
explicitly a draft and says so in its own front matter. Where the vendored
corpus's actual behaviour and the specification text disagree, prefer the
corpus and note the discrepancy.

## Lexer

Hand-written, single pass. It handles nested `(* *)` comments, literal-message
comments, character constants, string escapes and radix numbers.

Character constants are double-quote delimited (`"a"`), per the grammar's
`characterConstant`. The double quote is free for this because comments are
`(* *)` and strings are `'...'`. `lex_character_constant` consumes exactly one
character between the delimiters whatever that character is, including `"`
itself, so `""""` — the quote character — lexes correctly;
`corpus/modules/VCSSmalltalkSourceMirrors.ns` uses it and parses.

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
annotations, `[T]` type parameters on class names and selectors, and
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

## Deviation: mixin application is not implemented

§6.2's mixin application, `class C = Base <: Trait (...)(...)`, does not parse.
This one fails loudly -- a parse error naming the position -- so it cannot
produce a wrong answer. No corpus module writes `<:` in a superclass clause;
the eleven files that contain the operator use it inside `where { ... }` type
constraints, which are parsed and dropped, so `parse-corpus` does not exercise
the form either way.

## Deviation: the six reserved words are not lexer-reserved

`newspeak-spec.pdf` §4.1 lists `self`, `super`, `outer`, `true`, `false` and
`nil` as reserved words, illegal as ordinary identifiers. The lexer treats them
as ordinary identifiers instead: `true`, `false` and `nil` are recognized as
literals, and `self`, `super` and `outer` are recognized contextually by name at
the parser and resolver level. Expression-level `outer N foo` parses correctly,
checked against `corpus/modules/AccessModifierTesting.ns`, which uses it
heavily.

This only under-rejects programs that illegally use one of the six as an
ordinary identifier. It cannot affect a correct program, and no corpus file
exercises the illegal form.

## Deviation: `:=` is accepted as a synonym for a setter send

§5.4.3 gives Newspeak exactly one assignment form, the setter send `id:: e`
(desugaring to `id: e`, "Setter Sends"). The lexer also recognizes `:=`
(`TokenKind::Assign`) and `parse_expression` accepts `identifier := expression`
as a second spelling of the same thing, producing the same `Expression::Assign`
node and so resolved and lowered exactly as `id:: e` is
([dispatch.md](dispatch.md), "Setter sends"). Nothing in the specification
grammar, the canonical grammar
(`corpus/reference/NewspeakGrammar.ns`), Primordial Soup or SOMns accepts
`:=` -- all four know only the setter-send form -- so this is Smalltalk
syntax carried over rather than a reading of Newspeak's own grammar. It only
over-accepts: a program written with `id:: e` behaves exactly as the
specification requires, so no correct program is affected, but a program
that writes `id := e` instead -- which the specification does not accept at
all -- parses and runs anyway. Several goldens and tests use `:=`, since
nothing before this step distinguished it from `::`.
Decided with the user (2026-09-19) as the next piece of work after the
specification gaps, ahead of closure conversion: remove `:=` from the lexer
and parser, and convert every golden and test that uses it to `::`
([PROGRESS.md](../PROGRESS.md), "Where the next session starts").

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
