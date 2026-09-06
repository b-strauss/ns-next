# ns-next — Newspeak Runtime, Milestone 1 Design

Status: implemented (Milestone 1 complete)
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

**Read `2026-09-04-1153-ns-next-liveness-images-vm-design.md` before implementing
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

**`newspeak-spec.pdf`** (Gilad Bracha's "Newspeak Programming Language Draft
Specification", v0.106, `docs/references/newspeak-spec.pdf`) is vendored
alongside the grammar. Where `NewspeakGrammar.ns` is authoritative for
*syntax*, this document is authoritative for *semantics* — name resolution
(§5.7-5.10), dispatch and access control (§5.5, §5.9, §6.3.1), and
instantiation (§6.1, §6.3) are specified there in more precise detail than
this design doc previously carried, and several corrections below are sourced
from it. It is explicitly a draft ("incomplete...everything is subject to
change... in some cases the implementation deviates from this specification")
— where the vendored corpus's actual parse/run behavior and the spec text
disagree, prefer matching the corpus, and note the discrepancy.

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

**Corrected against `newspeak-spec.pdf` §2.3.1 and §5.7-§5.10.** Newspeak's
real resolution algorithm is two-phase, not a single linear scope walk, and
collapses more cases into one mechanism than the previous revision of this
section modeled:

1. **Local names are static, resolved purely lexically**, walking the
   activation chain outward: current method/block temps and parameters, then
   each enclosing block/closure's temps and parameters, innermost first. No
   dispatch is involved — this is a direct frame-slot read.
2. **Everything else — method calls, slot reads, slot writes, and bare
   nested/enclosing class-name references — is a *send*, not a direct
   field access**, per §2.2.1 ("references to methods without arguments are
   not distinguished from references to slots... or to nested classes").
   Concretely (§5.7): search the enclosing **class declarations** (not
   activations — those were already covered by step 1) from innermost
   outward — depth `k=0` is the class immediately enclosing the current
   method, `k=1` its enclosing class, and so on — for the smallest `k` whose
   class (including everything it inherits) has a member named `s`. That
   fixes the target as an outer-send at depth `k`; `k=0` is what's
   colloquially called a self send (§5.8 defines `self m` as literally
   `outer C m` where `C` is the immediately-enclosing class). If no lexical
   level has anything named `s`, it still resolves as depth `0` — the
   runtime dispatch below then naturally produces `doesNotUnderstand:`.
   `super` sends (§5.10) are a *third*, unrelated mechanism — see "Dispatch"
   below.

`Binding` variants:

- `Local(u16)` — slot in the current activation frame. Static, no dispatch.
- `OuterLocal { depth: u16, index: u16 }` — a captured method/block
  variable `depth` activations out. Static, no dispatch.
- `LexicalSend { depth: u16 }` — replaces the previous revision's
  `SelfSlot` / `OuterSlot` / `ImplicitSelfSend`, which modeled these as three
  separate cases; per the spec they are one mechanism (an outer-send at
  depth `k`, §5.7-§5.9). Covers implicit-receiver method calls, slot reads,
  slot writes (via the write's setter-keyword selector — see "Setter sends"
  below), and references to nested-class accessor methods found at that
  lexical depth. Runtime dispatch is in "Dispatch" below.
- **No separate "class value" binding.** A nested/enclosing class name is
  *itself* referenced via the ordinary `LexicalSend` mechanism (§2.2.1: a
  reference to a nested class is not syntactically distinguished from a
  method or slot reference) — dispatch happens to land on that class's
  auto-generated accessor (§6.4). This includes the superclass clause's
  `receiver` part (§6.1 evaluates it "as if it occurred inside an instance
  method" of the enclosing class) — same mechanism, evaluated at a specific
  time (class-declaration time; see "Class Instantiation"), not a distinct
  binding kind.
- `SuperSend` — a `super m` send (§5.10). Not depth-based: resolved at
  runtime against the *current activation's* `current_class` (see
  "Activation Records" below), which need not be `self`'s own class.

**Setter sends.** `id:: e` (§5.4.3) desugars to sending the keyword message
`id: e` and yielding `e` as the expression's value (not the setter method's
own return value, which is `self` per §6.3.2). The resolver must treat the
`id` in an `Expr::Assign` node exactly as it would treat an implicit-receiver
keyword send to `id:` — full `LexicalSend` depth search, private-vs-virtual
dispatch, the works — not as a distinct "slot write" primitive. The frontend
AST already represents `id:: e` as `Expr::Assign { target, value }` (a
Milestone-1 parser simplification, `== x := v` per its own code comment) —
that shape is fine to keep; this is purely a note for how the resolver must
*interpret* it. Checked against `crates/ns-syntax/src/parser/expr.rs:290`:
the value is built via a full recursive `parse_expression()` call, so it
already captures a complete cascade/keyword `sendExpression` per §5.4.3's
grammar — no frontend change needed.

**AST recognition, checked against the shipped parser.** The frontend already
normalizes several send shapes the resolver must recognize:

- **Bare unary implicit send** (`foo`, no message following): parses as a
  plain `Expr::Ident` — not wrapped in `Send` at all. Per §5.4.3, a bare
  identifier used as a `primary` *is itself* an implicit-receiver unary send.
  So every standalone `Expr::Ident` the resolver encounters (whatever
  position it appears in) needs the full Local → OuterLocal → §5.7
  depth-search resolution, *unless* it is literally `self` or `super` — see
  below.
- **Implicit-receiver keyword send** (`foo: 1 bar: 2`, no explicit receiver):
  `crates/ns-syntax/src/parser/expr.rs:339-344` already synthesizes the
  receiver as `Expr::Ident { name: "self", span: Span::new(at, at) }` — a
  **zero-width span** at the selector's start. This is the resolver's
  discriminator: a `Send` whose receiver is `Ident{"self", span}` with
  `span.start == span.end` is a true implicit send (run the §5.7
  depth-search); one with a real, non-zero-width span is a genuine explicit
  `self` send (§5.8 — depth is unconditionally `0`, no search). Binary sends
  are never implicit (§5.4.3), so no equivalent synthesis exists for them.
- **`outer N m`**: parses as an ordinary two-hop unary chain — no dedicated
  AST shape (confirmed empirically against `corpus/modules/AccessModifierTesting.ns`,
  which uses this extensively; see "Documented Deviations" below). The
  resolver must pattern-match `Send { receiver: Send { receiver:
  Ident{"outer"}, message: MessageSend { selector: Unary(n), .. } }, message:
  {selector: s, ..} }` where `n` starts uppercase, treat the *whole* shape as
  one `outer N s` send (§5.9 — compile-time error if `n` does not name a
  lexically enclosing class), and never dispatch the inner "send `n` to
  `outer`" step as a real message.
- **`super m`**: receiver is `Expr::Ident{"super", ..}` with a real span —
  always explicit per grammar (no implicit form).

#### Object Model

- `Value = Nil | Bool(bool) | SmallInt(i64) | Float(f64) | Obj(ObjRef)`
  where `ObjRef = Rc<RefCell<Object>>`.
- `Object { class: ClassRef, identity_hash: u32, data: ObjectData }`. The
  identity hash is a **monotonic counter value assigned at allocation**, not
  derived from the `Rc` pointer address — required by
  `2026-09-04-1153-ns-next-liveness-images-vm-design.md`'s stable-identity
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
    current_class: ClassRef,// see below — NOT necessarily receiver's class
    method: MethodRef,      // resolved fresh each call; never a baked-in
                             // pointer into a specific compiled body
}
```

`current_class` is required by `newspeak-spec.pdf` §3.6 ("The current class...
is `definingClass(s, C)` where `s` is the selector of `p` and `C` is the class
of the current instance, unless explicitly specified otherwise") — it is the
class that actually **defines** the running method, not necessarily the
receiver's own concrete class, and it is what subsequent `self`/`outer`/
`super` sends inside that method resolve relative to. Set by the dispatch
step that creates the activation (see "Dispatch" below), never derived from
the receiver at read time — this is what makes chained `super` sends inside
inherited methods resolve correctly through a deep hierarchy.

The interpreter still uses Rust recursion for control flow (evaluating a
`Send` recurses into evaluating its receiver and arguments), but every
Newspeak-level call pushes a `Frame` onto an explicit, independently walkable
frame stack. This is what a later milestone's debugger and mirrors reflect
over, and what an image snapshot serializes — nothing about it changes when
M2 replaces the tree-walk with bytecode; only how a `Frame`'s `pc` advances
changes.

#### Dispatch

**Every send is a virtual call; there is no static dispatch to fall back
on.** §2.1: "A synchronous message send is like a virtual method call." §2.3
makes this a hard language rule: "Methods are always virtual (i.e., subject
to override)" — the opposite default from Java/C++, where a method is
non-virtual unless declared otherwise. This holds for ordinary sends
(`lookupPublic` walks `o`'s *actual runtime class*, never a statically known
type), for non-`private` `LexicalSend`s (`lookupProtected` walks the kth
enclosing object's actual runtime class), and even for `super` sends (a
runtime `definingClass` walk, not a fixed compile-time target). **The one
exception is `private`**, and the spec is precise about why: "the code could
be predetermined — but the receiver could not" (§5.9) — a private method
can't be overridden, so its body is fixed, but *which object* it runs on
still depends on the runtime kth-enclosing-object navigation. Practical
consequence for later milestones: since there is no static dispatch to fall
back on outside the `private` case, inline caches (`2026-09-04-1153-ns-next-liveness-images-vm-design.md`'s
call-site identity + dependency-keyed caches) are not an optional speedup —
they are the central mechanism that makes dispatch affordable at all.

**Corrected against `newspeak-spec.pdf` §5.5, §5.9, §5.10; access defaults
per §6.3.1.** The previous revision of this section used one plain
"walk `methods` then `superclass`" algorithm for every kind of send. The
spec actually defines *three* different lookup shapes, gated by access
modifiers — a real behavioral difference the M1 interpreter must implement,
not just an editorial nuance:

- **`lookupPublic(selector, class)`** — used for **ordinary** sends (an
  explicit, non-`self`/`super`/`outer` receiver, §5.5). Walk from `class` up
  the superclass chain. At each level: if the mixin declares `selector` as
  `public`, that is the match. If the mixin declares `selector` at all but as
  `protected` (or `private`), **the search stops there and fails** — a
  non-public declaration *shadows* a public implementation further up the
  chain; it does not merely hide the name from external callers while
  leaving an inherited public version reachable. Only when the mixin does
  not declare `selector` at all does the walk continue to the superclass.
- **`lookupProtected(selector, class)`** — used for the runtime half of a
  `LexicalSend` (§5.9) whose statically-determined target is **not**
  `private`. Same chain walk, but matches `public` **or** `protected`;
  a `private` declaration at an intermediate level is transparently skipped
  (the walk continues past it, unlike `lookupPublic`'s hard stop).
- **Direct, non-virtual dispatch** — a `LexicalSend` whose
  statically-determined target (at its resolved depth `k`) is declared
  `private` bypasses both lookups above: invoke exactly that method,
  ignoring the enclosing object's actual runtime class entirely. (Private
  members are never overridden, so there is nothing to look up.)
- **`SuperSend` (§5.10)** looks up `selector` starting at the superclass of
  the *current activation's* `current_class` (not `self`'s own class), via
  the plain "defined for" walk (§3.5) — `public` and `protected` both match
  at every level (no `lookupPublic`-style hard stop). On success, the new
  activation's `current_class` is set to `definingClass(selector,
  superclass)` — see "Activation Records" above for why this matters for
  chained `super` sends.
- **`doesNotUnderstand:` is exempt from every access check above.** `Object`'s
  own implementation must be `protected` (§5.5), so if its dispatch used
  `lookupPublic` an ordinary send could never reach it. DNU is resolved via
  the plain presence-only "defined for" relation (§3.5): the first
  `doesNotUnderstand:` found walking the receiver's class chain, regardless
  of access modifier.
- **Default access, when unmarked, is `protected`** (§6.3.1) — this is
  binding, not cosmetic: it determines whether a member participates in
  `lookupPublic` at all. A top-level class's own access is always `public`,
  regardless of what is written on it.

For the `LexicalSend` case as a whole: compute `o` = the `k`th enclosing
object of `self` with respect to the current class (§3.3 — walked over the
*actual* enclosing-object chain matched against mixin applications, not a
naive lexical-nesting count, so that subclassing across a nesting boundary
still resolves correctly), then apply the private/`lookupProtected` rule
above using `o`'s *actual runtime class*. This is what makes public/protected
`LexicalSend`s virtual (overridable in a subclass) while private ones are
not.

#### Class Instantiation

**Corrected against `newspeak-spec.pdf` §6.1 and §6.3.** An earlier revision
of this section concluded, from the grammar shape alone, that "the
superclass-clause expression is evaluated once, and the resulting value's
`class` is the superclass" — this conflated two things the spec keeps
separate, and is corrected here; see the retraction in "Open Questions
Resolved During Implementation" below.

A superclass clause `[prefix] receiver [message]` (e.g. `Object new`, or
`balance: b` in `= Object new (...)`, or `Bla bar` in `Bla bar = (...)`) has
its two parts evaluated at **different times**:

- The `receiver` part alone (`Object`) is "the class determined by the
  inheritance clause" (§6.1) — evaluated **once, when the class declaration
  itself is evaluated**, via the *same* name resolution as any other
  identifier (§5.7-§5.9; class names are not a separate namespace — `Object`
  is looked up exactly like a method or slot name would be). Its value fixes
  the actual superclass, directly — there is no intermediate "evaluate
  `Object new`, allocate an instance, take that instance's class" step.
- The trailing `message` part (`new`) is "the message determined by the
  inheritance clause" and is **not evaluated at class-declaration time**. It
  is deferred and evaluated fresh on **every instantiation** of the
  (sub)class, in the context of that instantiation's own
  instance-initializer activation — so the subclass's own factory parameters
  are in scope when it runs.

Instantiation, when the factory message is sent to a class object (§6.3):

1. **One** instance `o` is allocated — distinct identity, every slot declared
   by the class and its superclasses initially `nil`. There is *no* separate
   instance allocated per level of the inheritance chain: `o` is the same
   object threaded through every step below.
2. An instance-initializer activation `i` is created, with the same
   arguments as the factory message.
3. In the context of `i`, the class's superclass clause's *deferred message*
   is evaluated (this class's factory parameters are in scope) — the result
   is sent to the superclass's own instance initializer, invoked on the
   *same* `o`. This recurses up the chain to `Object`.
4. Once the superclass chain above has fully run, this class's own slot
   clauses execute on `o`, in declaration order.
5. This class's own `initExprs` statement sequence executes on `o`.
6. `i` returns `o` to the factory method, which returns it to the caller.

Step 3 is why a class body has no access to the factory's actual parameters
except through slots explicitly extracted in the initializer (§6.3) — only
`i` sees them, not the class body proper.

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

## Documented Deviations from the Language Specification (Milestone 1)

Places where M1's design deliberately simplifies `newspeak-spec.pdf`,
recorded here per this plan's Global Constraint that a grammar/spec
deviation gets noted rather than left implicit:

- **No metaclass architecture.** §3.2: every class is formally an instance of
  its own flat metaclass, itself a subclass of `Class`, an instance of
  `Metaclass`. M1's `ClassData.class_side: Option<ClassRef>` is a side-table
  standing in for this — class-side methods are stored directly rather than
  materialized as a real metaclass instance. No M1 program sends a message
  to a metaclass object itself, so this is unobservable for now; revisit if
  a later milestone needs metaclasses to be first-class values (mirrors
  almost certainly will).
- **No "value object" concept.** §3.1.1 defines a formal, deeply-immutable
  "value object" notion (numbers, strings, symbols, characters, booleans,
  module definitions, and any class inheriting `Value` whose slots are all
  immutable value objects) — the mechanism that lets an object cross an
  actor boundary by copy instead of by far reference (§3.7). M1 has no
  actors, so this is moot for now, but the Concurrency section of
  `2026-09-04-1153-ns-next-liveness-images-vm-design.md` should adopt this exact
  spec vocabulary and mechanism — not the looser "provably non-aliased or
  frozen data" phrasing it currently uses — once concurrency is implemented.
- **`self`, `super`, `outer`, `true`, `false`, `nil` are not lexer-reserved.**
  §4.1 lists all six as reserved words — illegal as ordinary identifiers. The
  shipped M1 lexer treats them as ordinary identifiers: `true`/`false`/`nil`
  are recognized as literals and `self`/`super`/`outer` are recognized
  contextually, by name, at the parser/resolver level (confirmed working —
  `self foo`, `super foo`, and general expression-level `outer N foo` all
  parse correctly today; `outer N foo` was checked directly against
  `corpus/modules/AccessModifierTesting.ns`, which uses it extensively, via
  `cargo run -p ns-cli -- parse`). This only under-rejects programs that
  illegally use these six names as ordinary identifiers; it does not affect
  any correct program, and no corpus file exercises the illegal form. Not
  worth fixing now — revisit only if it ever causes an observable problem.

## Open Questions Resolved During Implementation

- **`superclassClause` semantics (the `= Object new (` idiom) — RESOLVED,
  CORRECTING an earlier pass of this section.** An earlier revision, working
  from the grammar shape alone
  (`NewspeakGrammar.ns:203`: `superclassClause = superclassPrefix opt ,
  unaryMsg , message opt` — an ordinary message-send expression, nothing
  syntactically special, which is still correct and is why all 211 corpus
  files parse it), concluded: "the superclass-clause expression is evaluated
  once, and the resulting value's `class` is the superclass." **That
  conclusion was wrong in a load-bearing way.** Reading the actual language
  specification (`newspeak-spec.pdf` §6.1, §6.3 — now vendored in
  `corpus/reference/`) shows the *receiver* part (`Object`) and the
  *trailing message* part (`new`) of the clause are evaluated at different
  times, and there is no "allocate an instance, take its class" step at all
  — see "Class Instantiation" above for the corrected model in full. The
  corpus's own `BankAccount.ns` / `BankAccountExemplarDemo.ns` finding still
  stands unchanged: their `(* :exemplar: BankAccount balance: 100 *)`
  comments are literal-message *tooling pragmas* (already skipped as
  ordinary trivia), unrelated to inheritance — that part of the earlier
  analysis was correct and needed no fix.
- **Character-constant lexing edge case (`""""` for the quote character) —
  RESOLVED, already correct.** `ns-syntax`'s `lex_char_constant` consumes
  exactly one character between `"` delimiters regardless of what that
  character is (including `"` itself), matching
  `characterConstant = "\"" , charIncludingQuotesAndSpace , "\""`. Confirmed
  against a real corpus file: `corpus/modules/VCSSmalltalkSourceMirrors.ns`
  uses `""""` and is one of the 210 corpus files that already parse
  successfully. No further lexer work needed.
