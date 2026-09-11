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

- `compilationUnit = languageId , classCategory , classDecl , eoi`
- `classHeader = accessModifier? , classReservedWord , classId ,
  messagePattern? , '=' , superclassClause? , '(' , classComment? ,
  slotDecls? , initExprs , ')'`
- `superclassClause = superclassPrefix? , unaryMsg , message?`, where
  `superclassPrefix` is `outer <id>` | `self` | `super`
- `slotDecls = '|' slotDef* '|'` (sequential) or `'||' slotDef* '||'`
  (simultaneous). A `slotDef` is
  `accessModifier? slotDecl (('=' | '::=') expression)? '.'` — `=` is an
  immutable slot, `::=` a mutable one, and neither means an uninitialized
  mutable slot.
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
`(* *)` and strings are `'...'`. `lex_char_constant` consumes exactly one
character between the delimiters whatever that character is, including `"`
itself, so `""""` — the quote character — lexes correctly;
`corpus/modules/VCSSmalltalkSourceMirrors.ns` uses it and parses.

Comments are retained as tokens for tooling and skipped by the parser.

## AST

`Module`, `ClassDecl`, `ClassHeader`, `SuperclassClause`,
`SlotDecl { name, mutability, init }`, `MethodDecl { pattern, body }`,
`MessagePattern`, `CodeBody { temps, statements }`, `Stmt`, `Expr` (`Ident`,
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
accepted and dropped.

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
source that writes `expr -1` meaning `expr - 1`, where the lexer's rule makes
`-1` a negative literal. One file, `NewspeakColorizationForSqueak.ns`.

## Errors

Lexer and parser collect `Diagnostic { span, message, severity }` and report all
of them. Malformed input is never a panic.
