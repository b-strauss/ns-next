# The specification of ns-next

ns-next gets a specification of its own: the language it runs, the
run-time semantics, the bytecode, the image format and the invariants the
implementation keeps, written in Markdown, rendered as an HTML book, and
bound to the tests that check each rule. This document records what was
decided and in which order it is built. The first batch is built: the
book under `docs/spec/`, its title page and introduction followed by an
informal overview of the language that states no rule
(`docs/spec/overview.md`), `cargo xtask spec`, which checks its rule
identifiers, the tests' references to them and its links and renders it,
the commit gate's step, the rule of `CLAUDE.md` that a change keeps the
book with it, and the language's lexical structure and grammar, each
rule with its tests, the special characters, the reserved words and the
words with tokens of their own as tables the lexer's and the parser's
tests hold to the code, and the appendix of the differences from
Bracha's Newspeak. The parts after the grammar say what they will cover
and are not written yet.

## Why

The rules ns-next keeps are spread over the design documents, the
invariants of `CLAUDE.md` and the code, each told in prose beside its
reasoning, its prior art and its mechanism. A question such as what an
install does to a removed class's objects had to be answered from five
documents at once, and every invariant of `CLAUDE.md` has already produced
a real defect. Bracha's specification (`docs/references/newspeak-spec.pdf`)
leaves parts open or marked *Unimplemented*, and ns-next has already
decided syntax it departs from ([README.md](README.md), "Deviations from
the language specification"). The specification states the rules once,
normatively; the design documents keep the reasoning and the mechanism and
point at the rules (decided with the user on 2026-11-23, before the
complete install, [liveness-and-reflection.md](liveness-and-reflection.md),
"Planned: the complete install", goes on).

## The decisions

Each decided with the user on 2026-11-23.

1. **A self-contained specification of ns-next, Bracha's document its
   source.** It defines the language as ns-next implements it, derived
   from Bracha's document where that holds and completed where it is
   silent; Bracha's document is no longer the measure, so the principle
   that a deviation from the specification is a defect
   ([README.md](README.md), "Principles") refers to this one once it
   stands. An appendix lists where it differs from Bracha's Newspeak and
   why.
2. **Five parts.** (1) The language: its lexical structure, its grammar
   and its semantics, modules, their identity and metadata included. (2)
   The run-time semantics as abstract operations: lookup and access,
   instantiation, slot access, `outer`, unwinding and exceptions,
   `become:`, the install and the mirrors, the VM mirror as a capability,
   the depth limit and what a program observes of memory. (3) The
   bytecode: the instructions, their encoding, each one's register
   effects, and the rules the decoder checks. (4) The image format: its
   structure, sections and tags, its validation and its bounds. (5)
   Separately, the VM's invariants: the internal rules no program sees
   whose breach is silent wrong output -- the write barrier, handles and
   the brand, what the collector traces, the rules of machine code --
   each naming the rule of parts 1 to 4 it protects.
3. **For the language and the run-time semantics, every current deviation
   is decided on its own.** It is adopted as a normative rule, or marked
   in the text as a gap, as Bracha marks what is *Unimplemented*; what was
   never discussed stays a marked gap and nothing is invented for it. For
   the formats the current state is the contract.
4. **Prose in fixed forms.** The grammar is a PEG over tokens, with a
   lexical grammar beneath it; the semantics are named abstract
   operations in numbered steps, as ECMA-262 writes them; each instruction
   has a page in the frame of the JVM specification's chapter 6 --
   operands, encoding, the registers read and written, outer reach, jump
   targets, effect, errors; the image is structure tables with numbered
   validation rules.
5. **Markdown, rendered by mdBook, each rule named as the Rust Reference
   names its rules.** The source is Markdown under `docs/spec/`, a chapter
   per part and the appendix, readable on GitHub as it is. Every normative
   rule is preceded by an identifier line, `r[runtime.lookup.public]`, as
   the Rust Reference's `mdbook-spec` has it
   ([its dev guide](https://rust-lang.github.io/reference/dev-guide/tooling/mdbook-spec.html)).
   A preprocessor of `xtask`'s own, in Rust, checks the identifiers --
   unique, well formed -- turns them into anchors and links, and collects
   the tests that name a rule: a golden in a line of its `.toml`, a Rust
   test in a comment, as a test of `rust-lang/rust` names its rule in a
   `//@ reference:` header. The book shows each rule's tests, and an
   appendix the rules no test names yet. The grammar, the instruction
   table and the tag table stand in marked, machine-readable blocks that
   Rust tests hold to the code -- `opcodes.rs` to the tag table among
   them. `cargo xtask spec` builds the HTML into `target/spec/`, to be
   opened in a browser. mdBook is a development tool, installed as
   `cargo-nextest` is, and reaches no binary. The alternatives weighed
   were Sphinx (WebAssembly, Python, the Ferrocene Language
   Specification), ecmarkup (ECMA-262) and Bikeshed (W3C, WHATWG), and,
   before them, an executable model in Lean 4 written in Verso, dropped
   because the toolchain it would take on is more than the specification
   needs now: Verso's precompiled libraries do not link on Windows at
   v4.36.0.
6. **Every commit checks it.** The identifiers, the tests' references to
   them and the machine-readable tables are checked by Rust tests in the
   default `cargo nextest run`, and the commit gate builds the book, so a
   broken link or block fails at once. A commit that changes observable
   behaviour, a format or an invariant changes the specification's rule
   for it in the same commit, for every part already written.
7. **The specification is living.** It describes ns-next as it is at
   every commit, as the design documents do, and has no versions or
   editions of its own: a rule no longer true is rewritten, never kept
   beside its successor (decided with the user on 2026-11-23).

## The order of building it

Semantics first (decided with the user on 2026-11-23), each its own plan,
branch and end-of-batch gates:

1. **The foundation and the syntax**, built: mdBook, the preprocessor, the checks
   of identifiers and references, the rule in `CLAUDE.md`, the book's
   skeleton of five parts and the appendix; and the lexical structure and
   the grammar, the syntax deviations decided one by one, the parser's
   tests and the syntax goldens naming their rules.
2. **The language and the run-time semantics**, the other deviations
   decided; the install as it is.
3. **The image format**: the structure, the tags, the validation rules.
4. **The bytecode**: the instruction table and the register effects.
5. **The VM invariants**, and the design documents and `CLAUDE.md`
   pointing at the rules by identifier.

The complete install ([liveness-and-reflection.md](liveness-and-reflection.md),
"Planned: the complete install") resumes after the fifth, and changes the
specification as it changes the runtime.

## The second batch's deviations

Each of [README.md](README.md)'s remaining deviations is decided with the
user before the second batch's plan is written; the plan carries each
decision into the book -- a rule of ours in the appendix, or Bracha's
rule with ns-next's behaviour marked a gap -- and into the design
document it belongs to. Decided so far, on 2026-11-23 and 2026-11-24:

1. **Non-integral literals (Bracha §5.1.1).** `2.5` stays a binary
   `Float`: a rule of ours, in the appendix. An exact decimal is a class
   of its own, `Decimal`, reached through a protected accessor on
   `Object` as `Float` and `Fraction` are (Bracha §3.2), with a literal
   of its own: a decimal number, then `s` and an optional scale --
   `2.50s` (scale 2, from the digits written), `2.5s2`, `3s2` (`3.00`),
   `-0.1s` -- as ANSI Smalltalk's scaled decimal writes it; no radix. An
   `s` followed by anything but digits is not the literal's (`2.5sec` is
   `2.5` and the send `sec`). Class and literal are a gap until a batch
   builds them; the meaning of a scale below the digits written
   (`3.14159s2`) is decided in that batch's plan. Alternatives weighed:
   Bracha's `FixedPointNumber` for every non-integral literal, an exact
   `Fraction` as Primordial Soup and Bracha's parser make it
   (`NewspeakPredictiveParsing.ns`, `scanMaybeFraction:`), and the
   suffixes `m` (C#) and `M` (Clojure, which collides with a radix's
   digits).
2. **String literals and Unicode normal form C (Bracha §5.1.5).** A string
   literal holds the code points its source text holds: a rule of ours,
   in the appendix. No language normalizes its literals, nor do
   Bracha's implementations; normalizing only literals leaves every
   string built at run time as it was, which is where decomposed text
   comes from, and costs about 127 KB of tables
   (`unicode-normalization`, measured 2026-11-24). A normalization method
   of the library, if wanted, is a decision of its own.
3. **Tuple literals (Bracha §5.1.7).** `{e1. e2}` is an `ImmutableArray`,
   shallowly immutable, as Bracha's rule says: a gap until the
   collections below are built. With it the collections get read-only
   and value variants, in Kotlin's direction rather than Java's -- a
   class that cannot write has no writing method, rather than one that
   fails -- built from mixins:
   - **The mixins**, protocol without slots, so that they apply to
     `Object` and to `Value` alike: `Collection` (`do:`, `size`,
     `collect:`, `select:`, `inject:into:` …), `OrderedCollection` (a
     defined order: `first`, `last`, `reverseDo:`, `reversed`,
     `do:separatedBy:`), `IndexedCollection` (`at:`, `indexOf:`,
     `copyFrom:to:` …, reading only), `MutableIndexedCollection`
     (`at:put:`, `replaceFrom:to:with:startingAt:`, `sort:`, which leave
     `IndexedCollection`), `GrowableCollection` (`add:`, `addAll:`,
     `remove:` …), `KeyedCollection` (`at:`, `includesKey:`, `keysDo:` …)
     and `MutableKeyedCollection` (`at:put:`, `removeKey:` …).
     `addFirst:`, `addLast:`, `removeFirst` and `removeLast` stay
     `List`'s own.
   - **The classes**: `Array`, `ByteArray`, `WeakArray` and `TypedArray`
     are `Object <: Collection <: OrderedCollection <: IndexedCollection
     <: MutableIndexedCollection`; `List` adds `GrowableCollection`;
     `ImmutableArray` stops at `IndexedCollection`, and `ValueArray` is
     the same on `Value`, holding values only. `Map` is `Object <:
     Collection <: OrderedCollection <: KeyedCollection <:
     MutableKeyedCollection` (`IdentityMap` stays `Map`'s subclass),
     `ImmutableMap` stops at `KeyedCollection`, `ValueMap` is the same on
     `Value`. `Set` is `Object <: Collection <: OrderedCollection <:
     GrowableCollection`, `ImmutableSet` stops at `OrderedCollection`,
     `ValueSet` is the same on `Value`. The views `asReadOnly` answers,
     forwarding to their original and copying nothing, are
     `ReadOnlyArray` (over `Array`, `List`, `ByteArray` and a
     `TypedArray`), `ReadOnlyMap` and `ReadOnlySet`, each with only the
     reading mixins.
   - **The conversions**: `asArray` a mutable copy, as today;
     `asImmutable` a shallowly immutable copy; `asValueArray`,
     `asValueMap`, `asValueSet` a deeply immutable one, failing on an
     element that is not a value (not `asValue`, a `ValueHolder`'s
     selector in Smalltalk); `asReadOnly` a view.
   - **`String` and `Symbol` stay outside the hierarchy**: what a
     string's element is -- a code point, a one-character string, a
     grapheme -- is not one answer, and `indexOf:` finds a substring
     there. `codePoints` answers a `ValueArray` of `Integer`s.
   - **Where they live**: `ImmutableArray`, `ValueArray`, `ReadOnlyArray`,
     `Decimal` and the seven mixins in the kernel; `ImmutableArray` and
     `Decimal`, the classes of literals, get an accessor on `Object`
     (Bracha §3.2); the mixins are handed out by `platform collections`,
     as `Collection` is today; `List`, `Map`, `IdentityMap` and `Set`
     stay in `CollectionsFactory.ns` as they are, and their immutable,
     value and read-only variants are reached only as the answers of
     those conversions.
   The names `Array`, `List`, `Map` and `Set` stay mutable, as Primordial
   Soup's API has them. Alternatives weighed: a mutable `Array` as our
   rule, and the prefixes `ReadOnly` and `Frozen` and the mixin names
   `ExtensibleCollection` (ANSI Smalltalk) and `SequencedCollection`
   (Java 21's JEP 431).
4. **Asynchronous sends and actors (Bracha §5.6, §3.7).** Bracha's rules
   go into the book -- a send `<-:` answers a promise at once, the message
   is delivered in a later turn of its actor, even to the current one, in
   E-Order, and the promise is fulfilled or broken when the turn ends --
   and are a gap: until actors are built, an asynchronous send that runs
   signals an error naming the gap, rather than running synchronously and
   answering the value as it does today, which a program relying on the
   order gets wrong at exit status 0. It fails when it runs, not when it
   compiles, so that a module with `<-:` on a path never taken runs on.
   Nothing of the repository's own sends `<-:`; twelve corpus modules
   do, and only parse. How a turn waits for a promise is open for the
   batch that builds actors (`docs/OPEN-WORK.md`): callbacks and promise
   pipelining as Bracha and E have it, `await` as Swift's actors have it
   (a reentrant actor whose state may change at each `await`), or a
   parked stack as Go's goroutines and Java's virtual threads have it (no
   colouring, but deadlocks if the actor waits, or invisible
   interleaving if it does not) -- each buildable on the activations
   ns-next already suspends into frame objects, which a running-world
   image could then carry.

## Open

- Whether an executable oracle checks the specification later -- a naive
  reference interpreter in Rust, or a mechanized model -- decided once the
  prose stands and shows where it was ambiguous.
