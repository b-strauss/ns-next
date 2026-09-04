# ns-next — Newspeak Runtime, Milestone 1 Design

Status: approved for planning
Date: 2026-09-01
Author: Benjamin

## Summary

`ns-next` is a new, native runtime for the Newspeak programming language
(Gilad Bracha's Smalltalk descendant), implemented in Rust. This document
specifies **Milestone 1**: a tree-walking interpreter that parses the
official `Newspeak3` textual syntax, resolves names according to Newspeak's
lexical scoping rules, and executes a subset of real modules against a small
core library implemented in Rust.

Milestone 1 is explicitly a stepping stone. Later milestones are expected to
add a bytecode compiler and stack VM, a real garbage collector, bignums,
actors, and a reflection (mirrors) API. The architecture is chosen so those
can be added without restructuring the frontend.

**Read `2026-09-04-ns-next-liveness-images-vm-design.md` before implementing
this milestone.** It is a cross-cutting constraints spec that binds every
milestone, this one included: even though M1 implements none of live
debugging, mirrors, portable images, or actors (see Non-Goals below), its
object model, frame representation, and dispatch must already be shaped so
those can be added later without restructuring the runtime — the same way
this document's architecture protects the frontend. In particular: reify
activations (heap-reachable frames with a growable, indexable temp vector,
never only Rust locals/call stack), dispatch only through mutable method
dictionaries (never bake in a method-body pointer), give objects stable
identity and an identity hash from the start, and retain each method's/class's
source text with a provenance record.

## Goals

- Parse **every** `.ns` file in the official `newspeaklanguage/newspeak`
  repository without error (a documented blacklist of known hard cases
  may be exempted).
- Execute custom Newspeak test modules and the subset of real modules whose
  platform requirements are covered by the Milestone 1 core library.
- Faithfully implement Newspeak name resolution: no global namespace;
  names resolve through method/block scopes, `self` slots, and the `outer`
  chain of enclosing classes.
- Establish a clean crate boundary between syntax frontend and runtime, with
  an explicit name-resolution pass as the seam a future bytecode compiler
  plugs into.

## Non-Goals (Milestone 1)

- Bytecode compilation, JIT, or any VM beyond a tree-walker.
- A real garbage collector. Milestone 1 uses `Rc<RefCell<_>>`; reference
  cycles leak. This is an accepted, documented limitation.
- Arbitrary-precision integers. Milestone 1 uses `i64` and raises a runtime
  error on overflow.
- Actors / concurrency.
- A mirrors / reflection API.
- Typechecker.
- Hopscotch, IDE tooling, VCS integration.
- Full platform compatibility with the reference implementation.
- JavaScript / foreign-function interop.

## Reality Check: Testing Against the Real Repo

Almost every module in the official repo is a top-level class with a factory
such as `class Foo usingPlatform: platform = ...`, and expects `platform` to
be a full object graph (collections, streams, mirrors, and more). With a
Rust-built minimal core library, Milestone 1 can:

- Use the entire repo as a **parser corpus** — every `.ns` file must lex and
  parse.
- **Execute** only our own test modules plus the few real modules whose
  platform needs are satisfied by the Milestone 1 core library.

Broad execution conformance against the real repo is a later-milestone
concern and is out of scope here.

### Corpus parse result (Milestone 1)

As of `93e165a`, `cargo test -p ns-syntax --test corpus` parses 210 of 211
corpus files (99.5%). Blacklisted files and reasons are in
`corpus/blacklist.toml`.

Typed-Newspeak declaration syntax is parsed but discarded (there is no
Milestone 1 type AST): `<Type>` annotations on parameters and slots, `^<Type>`
return annotations, `[T]` type parameters on class names and selectors, and
`where { ... }` constraint clauses. The eventual-send operator `<-:` is
likewise accepted and dropped.

The one remaining deferred construct is the negative-literal vs. subtraction
lexer ambiguity: source that writes `expr -1` for `expr - 1` (one file,
`NewspeakColorizationForSqueak.ns`). Revisiting the Milestone 1 lexer rule
(`a -42` lexes as a negative literal) is deferred to Milestone 2.

## Source Format

A Newspeak source file (`.ns`) has the shape:

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
    public class-side-method = ( ... )
)
```

Grammar productions are transcribed from `NewspeakGrammar.ns` (a combinator
grammar in the official repo). Key productions:

- `compilationUnit = languageId , classCategory , classDecl , eoi`
- `classHeader = accessModifier? , classReservedWord , classId ,
  messagePattern? , '=' , superclassClause? , '(' , classComment? ,
  slotDecls? , initExprs , ')'`
- `superclassClause = superclassPrefix? , unaryMsg , message?` where
  `superclassPrefix` is `outer <id>` | `self` | `super`
- `slotDecls = '|' slotDef* '|'` (sequential) or `'||' slotDef* '||'`
  (simultaneous); `slotDef` is `accessModifier? slotDecl (('=' | '::=')
  expression)? '.'` — `=` is an immutable slot, `::=` a mutable slot,
  neither means an uninitialized mutable slot
- `messagePattern = unarySelector | (binarySelector slotDecl) |
  (keyword slotDecl)+`
- `methodDecl = accessModifier? , messagePattern , '=' , '(' , codeBody ,
  ')'`
- `codeBody = (comment | literalMessageComment | whitespace)* ,
  temporaries? , statements`
- `block = '[' , blockParameters? , codeBody , ']'`;
  `blockParameters = (':' slotDecl)+ '|'`
- `primary = slotName | literal | block | '(' expression ')'`
- `literal = pattern | number | symbolConstant | characterConstant |
  string | tuple`; `tuple = '{' (expression sepOrTermBy '.') '}'`
- `message = keywordMsg | binaryMsg | unaryMsg`;
  `cascadeMsg = ';' (keywordMsg | binaryMsg | unaryMsg)`
- comments: `(* ... *)`, nestable; `(* :literalmessage: ... *)` is a
  literal-message comment

`NewspeakGrammar.ns` is vendored into the repo so the parser can be checked
against it during implementation.

## Architecture

### Crate Layout

```
D:\Workspace\ns-next\
  Cargo.toml                 # workspace manifest
  crates/
    ns-syntax/               # lexer, tokens, AST, parser, spans, diagnostics
    ns-runtime/              # resolver, object model, interpreter, builtins, platform
    ns-cli/                  # binary `ns-next`
  corpus/                    # vendored .ns snapshot for the parser corpus test
    PROVENANCE.md            # upstream repo URL + exact commit hash
    blacklist.toml           # known-unparseable files, each with a reason
  tests-fixtures/            # *.ns + matching *.expected for execution tests
  docs/superpowers/specs/
```

### `ns-syntax`

Responsibilities: turn source bytes into a `Module` AST with source spans,
and report lexer/parser diagnostics.

- `Span { start: u32, end: u32 }`, byte offsets into the source.
- `Token` / `TokenKind`: identifiers, keywords (`foo:`), binary selectors,
  reserved words (`class`), literals, punctuation, comments (retained for
  tooling but skipped by the parser), EOI.
- Lexer: hand-written, single pass. Handles nested `(* *)` comments,
  `:literalmessage:` comments, character constants (double-quote
  delimited, `"a"`, per the grammar's `characterConstant` — `"` is free
  for this because comments are `(* *)` and strings are `'...'`), string
  escapes, radix numbers.
- AST (`ast` module): `Module`, `ClassDecl`, `ClassHeader`,
  `SuperclassClause`, `SlotDecl { name, mutability, init: Option<Expr> }`,
  `MethodDecl { pattern, body }`, `MessagePattern`, `CodeBody { temps,
  statements }`, `Stmt` (`Expr`, `Return`), `Expr` (`Ident`, `Literal`,
  `Block`, `Send { receiver, selector, args }`, `Cascade`, `Assign`,
  `Array`), `Literal`. Every node carries a `Span`.
- Parser: hand-written recursive descent + Pratt-style precedence for
  message sends (unary > binary > keyword), following `NewspeakGrammar.ns`.
- Public API: `lex(&str) -> Result<Vec<Token>, Vec<Diagnostic>>`,
  `parse(&str) -> Result<Module, Vec<Diagnostic>>`.

`ns-syntax` has no dependency on `ns-runtime`.

### `ns-runtime`

Responsibilities: resolve names, build the runtime object model, and
execute.

#### Resolver

Input: `Module`. Output: `ResolvedModule` — the same tree with each
`Ident` annotated with a `Binding`, plus per-scope slot layout metadata.

`Binding` variants:

- `Local(u16)` — slot in the current activation frame.
- `OuterLocal { depth: u16, index: u16 }` — a captured method/block
  variable `depth` frames out.
- `SelfSlot(Symbol)` — send an accessor message to `self`.
- `OuterSlot { depth: u16, name: Symbol }` — accessor on an enclosing
  object `depth` links out along the `outer` chain.
- `LexicalClass(ClassId)` — a nested or enclosing class name used as a
  value.
- `ImplicitSelfSend(Symbol)` — a bare lowercase name treated as a unary
  send to `self`.

Resolution order for a name in a method body: method temps/params →
enclosing block temps/params (innermost out) → receiver slots (`self`) →
slots and nested-class names of lexically enclosing classes (`outer`
chain), outward to the top-level class → the top-level class's factory
parameters (e.g. `platform`).

- A lowercase name with no binding that could be a unary send to `self`
  becomes `ImplicitSelfSend`.
- An uppercase name with no binding is a resolution error carrying the
  offending span.

#### Object Model

- `Value = Nil | Bool(bool) | SmallInt(i64) | Float(f64) | Obj(ObjRef)`
  where `ObjRef = Rc<RefCell<Object>>`.
- `Object { class: ClassRef, identity_hash: u32, data: ObjectData }`. The
  identity hash is a **monotonic counter value assigned at allocation**, not
  derived from the `Rc` pointer address — required by
  `2026-09-04-ns-next-liveness-images-vm-design.md`'s stable-identity
  constraint, since a hash derived from the address would break the moment a
  later milestone's GC moves the object or `become:` swaps its identity.
- `ObjectData = Instance(Vec<Value>) | Str(String) | Sym(Symbol) |
  Array(Vec<Value>) | Closure(ClosureData) | Class(ClassData)`.
- `ClassData { name: Symbol, superclass: Option<ClassRef>, methods:
  HashMap<Symbol, Rc<MethodDecl>>, native_methods: HashMap<Symbol,
  NativeFn>, instance_slots: Vec<ResolvedSlot>, class_side:
  Option<ClassRef>, enclosing: Option<ObjRef>, factory: Option<MessagePattern> }`.
- `ClosureData { params, captured_frame: FrameRef, body, home: ObjRef }`
  — captures its defining frame for closure semantics and its home
  context for non-local return.

#### Source Retention and Provenance

Per the liveness/VM constraints spec, every class and method keeps enough
information to recover its exact source text and where it came from — needed
later for the debugger and for exporting an image back to `.ns` files, and
cheap to carry from M1 onward.

- Each loaded module keeps its full original source as one `Rc<str>`,
  attached to the `Module`/`ResolvedModule` (not duplicated per node).
- Every `ClassDecl` / `MethodDecl` already carries a `Span` (from `ns-syntax`).
  Source text for any node is recovered on demand as
  `span.text(&module_source)` — no per-node string copy.
- `SourceOrigin` — for M1, always `SourceOrigin::File(PathBuf)`, attached
  once per module. Modeled as an enum specifically so a later milestone can
  add `SourceOrigin::Debugger { at: Timestamp }` for code entered live,
  without changing the type other call sites depend on.

#### Activation Records (Frame)

Per the liveness/VM constraints spec, a Newspeak-level activation is never
represented only by Rust locals or the Rust call stack. Milestone 1 stays a
tree-walking interpreter (no bytecode compiler — that is a Milestone 2
concern), but every activation is still an explicit, heap-reachable `Frame`:

```
struct Frame {
    receiver: Value,
    args: Vec<Value>,
    temps: Vec<Value>,      // indexable by resolver slot index, growable
    pc: usize,              // index into the enclosing CodeBody.stmts —
                             // M1's "program counter" is a statement index,
                             // not a byte/bytecode offset
    sender: Option<FrameRef>,
    home: Option<FrameRef>, // the enclosing method activation, for blocks
    method: MethodRef,      // resolved fresh each call; never a baked-in
                             // pointer into a specific compiled body
}
```

The interpreter still uses Rust recursion for control flow (evaluating a
`Send` recurses into evaluating its receiver and arguments), but every
Newspeak-level call pushes a `Frame` onto an explicit, independently walkable
frame stack. This is what a later milestone's debugger and mirrors reflect
over, and what an image snapshot serializes — nothing about it changes when
M2 replaces the tree-walk with bytecode; only how a `Frame`'s `pc` advances
changes.

#### Dispatch

Look up the selector in the receiver class's `methods`, then
`native_methods`, then walk `superclass`. On a miss, send
`doesNotUnderstand:` with a message object; the default implementation
raises a runtime error carrying the selector and receiver.

#### Class Instantiation

1. The `superclassClause` expression is evaluated once, in the enclosing
   lexical scope, to yield the superclass (default `Object` when absent).
2. A class object (`ObjectData::Class`) is created, its `enclosing` set to
   the defining object, its method dictionaries populated from the AST.
3. Sending the factory keyword message (e.g. `usingPlatform:`) to the
   class object allocates an instance, binds the factory parameters,
   evaluates slot initializers in the instance's context in declaration
   order, runs `initExprs`, and returns the instance.

See "Open Questions Resolved During Implementation" below for why step 1 needs
no special-casing for the `= Object new (` idiom, or any other superclass
expression: it is always just "evaluate the expression, take the result's
class."

#### Core Library (native, in Rust)

`Object`, `Behavior`/`Class`, `Boolean`/`True`/`False`, `Integer` (`i64`,
overflow raises), `Float`, `String`, `Symbol`, `Character`, `Array`,
`OrderedCollection`/`List`, `Nil`, `Block`.

A minimal `platform` object exposes these classes plus a stdout stream
object (enough for `Transcript`-style output in tests).

### `ns-cli`

Binary `ns-next`. Subcommands:

- `ns-next run <file.ns> [--send <selector>]` — load the top-level class,
  instantiate it via its factory (passing the built-in `platform`), send
  the entry selector (default configurable), print the result / any
  stdout, exit non-zero on uncaught Newspeak error.
- `ns-next parse <file.ns> [--ast]` — parse only; print diagnostics, and
  the AST with `--ast`.
- `ns-next parse-corpus <dir>` — parse every `.ns` under `<dir>`, honor
  `corpus/blacklist.toml`, print a pass/fail summary, exit non-zero on any
  unexpected failure.

Arg parsing via `clap`.

## Data Flow

```
source bytes
  -> ns-syntax::lex        -> Vec<Token>
  -> ns-syntax::parse      -> Module (AST + spans)
  -> ns-runtime::resolve   -> ResolvedModule (bindings + slot layout)
  -> Interpreter::load     -> top-level class object
  -> CLI: instantiate via factory message, passing `platform`
  -> CLI: send entry message
  -> result value / stdout
```

## Error Handling

- Lexer / parser: collect `Diagnostic { span, message, severity }`;
  report all, do not panic on malformed input.
- Resolver: `ResolveError { span, kind }`; reported before execution.
- Runtime: a `NewspeakError` value carrying selector / receiver /
  message; uncaught errors propagate to the CLI, which prints a Newspeak
  stack trace (frame = selector + source span) and exits non-zero.
- Interpreter bugs (invariant violations) use `panic!` / `debug_assert!`
  and are distinct from `NewspeakError`.

## Testing

- `ns-syntax`: unit tests for the lexer and parser; AST snapshot tests
  via `insta`.
- **Parser corpus test**: `corpus/` holds every `.ns` from
  `newspeaklanguage/newspeak` at a fixed commit (recorded in
  `corpus/PROVENANCE.md`). A test runs `parse` on each file; failures not
  listed in `corpus/blacklist.toml` (with a stated reason) fail the
  suite.
- `ns-runtime`: unit tests for the resolver (each `Binding` variant) and
  the interpreter (dispatch, `super` sends, blocks, non-local return,
  cascades, slot mutation).
- **Execution integration tests**: `tests-fixtures/<name>.ns` +
  `tests-fixtures/<name>.expected`; a harness runs `ns-next run` and
  compares stdout.

## Vendored Corpus

The corpus is a **manual snapshot**, not a git subtree/submodule: copy the
`.ns` files at a known upstream commit and record the commit hash and
source URL in `corpus/PROVENANCE.md`. Refresh is a deliberate, reviewed
action. Upstream license is preserved alongside the files.

## Decisions

- **`i64` integers**, overflow raises a `NewspeakError` — bignums are a
  later milestone.
- **`Rc<RefCell<_>>` object graph**, cycles leak — a real GC is a later
  milestone.
- **Manual corpus snapshot** with a recorded commit hash — not a
  submodule.
- **Hand-written lexer and parser** — no parser-generator dependency;
  checked against `NewspeakGrammar.ns`.

## Open Questions Resolved During Implementation

- **`superclassClause` semantics (the `= Object new (` idiom) — RESOLVED.**
  The grammar (`NewspeakGrammar.ns:203`) defines
  `superclassClause = superclassPrefix opt , unaryMsg , message opt` — an
  ordinary message-send expression, nothing syntactically special (all 211
  corpus files already parse it under this rule). The corpus's own
  `BankAccount.ns` / `BankAccountExemplarDemo.ns` — despite the name — confirm
  there is no exemplar/prototype state propagation: their
  `(* :exemplar: BankAccount balance: 100 *)` comments are literal-message
  *tooling pragmas* (already skipped as ordinary trivia), unrelated to
  inheritance. The actual semantics are exactly what "Class Instantiation"
  above already states: **the superclass-clause expression is evaluated once,
  and the resulting value's `class` is the superclass.** `SomeClass new` is
  simply the idiomatic way to say "my superclass is `SomeClass`" — you never
  reference a class bare in this position, you evaluate to an instance and
  take its class. No special-casing needed in the resolver or interpreter.
- **Character-constant lexing edge case (`""""` for the quote character) —
  RESOLVED, already correct.** `ns-syntax`'s `lex_char_constant` consumes
  exactly one character between `"` delimiters regardless of what that
  character is (including `"` itself), matching
  `characterConstant = "\"" , charIncludingQuotesAndSpace , "\""`. Confirmed
  against a real corpus file: `corpus/modules/VCSSmalltalkSourceMirrors.ns`
  uses `""""` and is one of the 210 corpus files that already parse
  successfully. No further lexer work needed.
