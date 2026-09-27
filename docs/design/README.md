# ns-next — Design

`ns-next` is a native Rust runtime for [Newspeak](https://newspeaklanguage.org/):
lexer, parser, resolver, bytecode compiler and register VM, and a versioned image
format. Rust 1.98.0, edition 2021.

This folder is the **living description of how the system is designed and how much
of it exists**. It is updated in place. There is no series of dated design
documents to reconcile: when a design changes, the document covering that aspect
changes with it, and git carries the history.

## How to read these documents

Three conventions make a document's claims checkable rather than merely
plausible.

**Present tense means it exists.** If a document says the application cache is
keyed by the superclass's application, that is what the code does today. Nothing
is described in the present tense as a way of describing an intention.

**Unbuilt work is marked, not paraphrased.** A section describing something that
does not exist yet is headed `Planned: …` and says in its first line what would
change. Something built but incomplete opens with **Partial:** and names what is
missing. Planned work lives in the document whose mechanism it changes, not in a
separate roadmap, because that is where a reader needs it.

**Every document names its code.** Each opens with a status block listing the
modules it describes, so a claim can be checked by opening a file. A document
that cannot name its code is describing an intention, and should say so.

## The map

| Aspect | Document | Status | Planned next |
|---|---|---|---|
| Lexer, parser, AST | [frontend.md](frontend.md) | Built | — |
| Values, objects, mixins, classes, instantiation | [object-model.md](object-model.md) | Built, §6.2's mixin application included: each `<:` clause adds an ordinary class level, so lookup, `super`, `outer` and the caches need nothing new. §3.2's class hierarchy is reified: every class object's class is its metaclass, over `Class`, and every metaclass's class is `Metaclass`. All four of §3.1.1's value conditions are enforced, refused where an instance is made, and a module's root may name only `Object` or `Value`, or write `Object <: Value` | `become:` |
| Name resolution, lookup, access control, sends | [dispatch.md](dispatch.md) | Built, the inline caches included: primitives as method-table entries (3a) — since step 6 on the VM mirror's class alone, with kernel primitive methods run in `act` without an activation — per-heap caches with a global cache behind them (3b), the five refinements decided before 3c — flat metaclasses, the primary factory as a metaclass entry, the lexical global-cache key, unserialized call-site numbers and the cache's least-recently-used groups — and the dispatch id in the object header (3c); step 3 is complete; a §5.7 member set is now keyed by a real class identity, and a setter send skips every local that declares no setter (a parameter or an immutable local) | — |
| AST to bytecode: instructions, lowering, inlining | [compiler.md](compiler.md) | Built; call sites numbered per code family, the capture analysis and converted register layout closure conversion needs, an inlined fast path for the nine binary integer leaves, and a kernel mirror send's failure block lowered inline (`SendMirror`) | dropping `IntegerBinary`'s `Jump` |
| Activations, the VM loop | [execution.md](execution.md) | Built, including the trampoline; nothing re-enters the loop from Rust, the loop polls a safepoint, its sends go through the inline caches, and the optimized lowering converts closures — a captured variable is copied into the closure or lives in its scope's indirection vector, and only by-reference code keeps a returning frame's registers | JIT tiers |
| Object lifetime, identity, collection | [memory.md](memory.md) | Built — the arena, the activation stack, the safepoint, tracing, host roots, the stress modes, the lifetime brand and a mark-compact collector; a slot is reclaimed once nothing reaches it | generations |
| Serialization, both artifacts | [image-format.md](image-format.md) | Built | suspend and resume |
| Builtins and the platform | [core-library.md](core-library.md) | **Partial:** `Integer` answers `//` but not `/`. Built as Newspeak source over a VM mirror: every builtin method is Newspeak in `crates/ns-runtime/core-library/Kernel.ns` -- a forwarder to one of the mirror's `MIRROR` rows, which are the leaves, or a quick method or ordinary method that reaches no leaf, as all of `True`'s, `False`'s and the nil protocol's are. Its hierarchy is §3.2's and §3.1.1's — `Top`, then `Object`, then `Value` over the value classes with `Number` among them, and `Behavior` over `Class` and `Metaclass` — and `Value` derives `=`, `==` and `hash` from an instance's state. Every class object's class is its own metaclass, its class side applied to `Class`, and every metaclass's class is `Metaclass`; a class object is a value when its enclosing object is. `Object` carries §3.2's accessors for the predefined classes, so a bare `String` is a send answered without an activation. §8.6's exceptions are Newspeak over kernel-private activation objects: `Exception`, `Error`, `MessageNotUnderstood`, `Halt` and `Message`, reached through `platform kernel`, with ANSI's `on:do:`, `ensure:`, `ifCurtailed:`, `return:`, `retry`, `resume:` and `pass`; every VM error and every message nothing understands is signalled, and the depth limit keeps a reserve for signalling its own. The platform is an instance of the nested `Platform` of the shipped module `crates/ns-runtime/core-library/PlatformFactory.ns`, whose root is booted with the mirror and the kernel instance from the table of shipped modules (`interpreter/core_library.rs`) and never handed out (the factory pattern); its transcript, `collections` and `kernel` members are lazy slots, its module's classes are handed out by class accessors, and an image names all of them by their platform paths | The collections module (a Newspeak `List` in `CollectionsFactory.ns`, the mirror's `moduleNamed:`); real bodies for `whileTrue:`, `whileFalse:` and `do:` before the JIT tiers; settling `Integer>>/` and the numeric tower `Number` is the place for |
| Debugger, mirrors, edit-and-continue, actors | [liveness-and-reflection.md](liveness-and-reflection.md) | Planned | — |
| Editor support: the language server and the JetBrains plugin | [editor-support.md](editor-support.md) | Built — highlighting and errors as you type in every commercial JetBrains IDE from 2026.1.4; the server bundled for Windows x64 | formatting, navigation, more platforms |

## Architecture

Four crates, with the name-resolution pass as the seam the bytecode compiler
plugs into, and the JetBrains plugin beside them:

```
crates/
  ns-syntax/    lexer, tokens, AST, spans, diagnostics
  ns-runtime/   resolver, object model, dispatch, compiler, VM, the leaves, image;
                core-library/Kernel.ns, the core library in Newspeak
  ns-cli/       the `ns-next` binary
  ns-lsp/       the `ns-next-lsp` binary: the editor's language server
editors/
  jetbrains/    the JetBrains plugin: a Kotlin lexer mirroring ns-syntax's,
                highlighting, and the connection to ns-next-lsp over the
                native LSP API
```

Inside `ns-runtime`, one file cuts across every other: `heap.rs`. It owns the
object arena, the activation stack every running activation lives on, and the
frame arena an activation reaches only when a block captures it. `ObjectReference`,
`ClassReference` and `FrameReference` are opaque 32-bit handles into the arenas, and
`ActivationReference` into the stack; each one's inner index is private to that file,
so every read and write goes through a `Heap` accessor because the compiler
refuses anything else. That seam exists so the store behind a handle can be
replaced by changing one file rather than every caller; replacing
`Rc<RefCell<_>>` with the arena is the evidence that it works, since no call
site outside `heap.rs` and `types.rs` changed for either the object or the
frame conversion. The handle types are not hidden, on purpose: the two
activation handles are distinct types in every signature — a closure, a frame
and an image record take only a `FrameReference` — so storing an `ActivationReference` in
anything that outlives its call is a compile error. And every movable handle
carries the brand of the entry that minted it, a lifetime obtainable only
inside `Interpreter::enter`, which `heap/world.rs`'s `World::enter` gives in
safe Rust — the crate forbids `unsafe` code — so a handle held across a
safepoint, or kept by a host outside an entry, is a compile error too. Start
at [memory.md](memory.md) for what the heap guarantees — a mark-compact
collector reclaims a slot once nothing reaches it, only at a safepoint of the
run loop — and at [execution.md](execution.md), "Activations", for the stack.

`ns-syntax` does not depend on `ns-runtime`. Nothing under `crates/ns-syntax`
should change for runtime work.

### From source to a running program

```
Interpreter::bootstrap          the builtin classes, the VM mirror, and
                                core-library/Kernel.ns compiled into them
                                and instantiated with the mirror
source bytes
  -> ns-syntax::lex             Vec<Token>
  -> ns-syntax::parse           Module (AST + spans)
  -> ns-runtime::resolve        ResolvedModule (bindings + slot layout)
  -> Interpreter::load_module   the module's root class object, its code
                                compiled onto the declarations: one
                                CompiledCode per body, on its mixin, by
                                the interpreter's lowering -- optimized,
                                or the reference lowering it is checked
                                against
  -> instantiate                the primary factory, then each level's initializer
  -> the entry send             whatever the CLI was asked to send
```

The two serialization paths branch off this: `snapshot` serializes the world
after instantiation, `load` rebuilds one without running the front end at all.
See [image-format.md](image-format.md).

### The `ns-next` binary

`parse`, `parse-corpus`, `run-corpus`, `run`, `snapshot`, `load`, `bench`,
`disasm`, `inspect`, `build` — one module each under
`crates/ns-cli/src/commands/`. `main.rs` holds only the argument parser, the
dispatch to them and the thread every command runs on — a thread of its own
because the symbol interner is thread-local, spawned with the stack the front
end needs ([execution.md](execution.md), "The depth limit"); loading a source
file, and instantiating the root class and sending the entry, which several
commands share, live in `source.rs` and `entry.rs`.

## The goldens

`goldens/` is this runtime's recorded behaviour. A golden is a `.ns` program
and a `.toml` beside it holding the outcome that program must produce:
`status`, `stdout` and `stderr`, each defaulting to the success case (`0`,
empty, empty), so an ordinary golden's file is just its `stdout` and one that
raises writes its `status` and `stderr`. Both lowerings must produce it, byte
for byte and exit status included (`crates/ns-cli/tests/run_goldens.rs`).
That runner is a `harness = false` binary on `libtest-mimic`, one test per
golden per lowering named by its path -- `golden::optimized::<category>/<name>`
and `golden::reference::<category>/<name>`, so nextest runs the two lowerings
of a golden in parallel rather than one trial running both in sequence -- and
in a debug build `golden_with_a_two_entry_global_cache::<category>/<name>` as
well -- so the suite lists each golden, reports every failure with the expected
and actual stream, runs them in parallel, and runs one or a family by name
(`cargo nextest run -p ns-cli --test run_goldens lazy`); a skipped golden is
listed as skipped, and `--run-ignored all` runs it. The snapshot and
build/load round trips are the same, one test per golden
(`crates/ns-cli/tests/golden_round_trips.rs`).
Every golden that is not skipped, the ones recorded as raising included, also
goes through the snapshot and the build/load round trip, which hold an image's
and a binary's three streams against what `run` answers on the source.

A fourth key, `skipped`, carries why the runtime does not answer this one
yet -- `skipped = "not implemented yet: simultaneous slot clauses initialize
sequentially, so second is nil when first is computed"`. The sweeps pass it
over and count it; everything
else about it still holds, including the pairing check, so a `skipped`
cannot switch a half-written golden past discovery. The other three keys
still record what the program *should* answer, which is the point: a gap
gets its golden the day it is found, stating the whole expectation, and
closing the gap is deleting one line. A reason is required, and a blank
one fails. A skipped golden is an ignored test, counted in the suite's
summary; `cargo nextest list -p ns-cli --run-ignored only` names them.

| Directory | What belongs in it |
|---|---|
| `bench/` | the measurement protocol's own sources, the ones `ns-next bench` times |
| `cache/` | an outcome an inline cache could change, each added before any cache existed |
| `capture/` | what a block captures: copied variables, indirection vectors, the fallback closures of an inlined conditional |
| `class-side/` | class sides and metaclasses: an implicit class side, `outer` from one, class-side `super` reaching `Class`, a class side's `doesNotUnderstand:`, the class of a class and of a metaclass, their names, and the refusal to instantiate a class object |
| `errors/` | a program that raises, where the error text and the exit status are the point — among them one golden per kind of module root the root rule refuses |
| `exceptions/` | §8.6's exceptions: signalling, handling, `return:`, `retry`, `resume:` and `pass`, the handler environment, `ensure:` and `ifCurtailed:` on every way out, VM errors and `doesNotUnderstand:` caught, and the depth limit's reserve |
| `kernel/` | the core library in `Kernel.ns`: what a user subclass of a builtin inherits, the one outcome the move defined, a recursion through kernel methods the two lowerings count alike, and §3.1.1's value semantics — a value's derived equality and hash, its lack of identity, and every one of its value conditions, those decided at class creation included, each refused at instantiation, and a class object that is a value because its enclosing object is |
| `language/` | the core language — arithmetic, blocks, closures, slots, control flow |
| `limits/` | the activation limit: what still runs at 1000 deep, and what overflows |
| `mixin/` | §6.2's mixin application: lookup and `super` through the applied levels, `outer` from an applied mixin, the order of several `<:` clauses, the initializer order, the level's factory selector, the errors a clause raises, and a module that declares itself a value with `<: Value` |
| `nesting/` | nested classes: `outer`, redeclaration, overriding, nested `super` |
| `spec/` | one named rule of `newspeak-spec.pdf`, its section in the file name |

A golden lives in a category directory, one level down, and the set is found
by reading `goldens/` and then each category — no list, no count, so adding
one needs no edit anywhere. Anything at another depth is refused by name
rather than skipped, because a golden nothing discovers is not a golden at
all. `every_golden_is_a_complete_pair` covers the other half: an orphan on
either side is a golden that stopped being discovered because one half was
renamed, moved or re-extended. Deleting both halves together is not caught,
and shows up in a diff as two deletions.

A golden's rationale — why that outcome is the right one, and which
specification section it comes from — is a `(* ... *)` comment at the top of
the `.ns`, under the category string, so that it sits with the program it
explains rather than beside it. The `.toml` holds the recorded answer and
nothing else.

## Gates

These are non-regression gates, not suggestions. Each has caught a real
regression.

```bash
cargo nextest run --workspace --exclude ns-lsp   # includes every golden, under both lowerings
cargo test --workspace --exclude ns-lsp --doc    # the doctests, which nextest does not run
NS_NEXT_STRESS_MOVES=1 cargo nextest run --workspace --exclude ns-lsp   # the same, moving everything at the run loop's safepoints
NS_NEXT_STRESS_CACHES=1 cargo nextest run --workspace --exclude ns-lsp  # the same, vacating every inline cache at every safepoint
NS_NEXT_STRESS_COLLECT=1 cargo nextest run --workspace --exclude ns-lsp # the same, collecting at every safepoint
# (CLAUDE.md's end-of-batch gate runs the three stress modes at once, each into its own log)
cargo nextest run -p ns-lsp             # the editor's language server: never starts the VM, so excluded above
cargo clippy --workspace --all-targets   # must be warning-free, the naming lint included
cargo fmt --all                 # before every commit
cargo run -p ns-cli -- parse-corpus corpus/modules   # parsed 210, failed 0, expected-fail 1, stale 0
cargo run -p ns-cli -- run-corpus corpus/modules     # ran 6, failed 0, skipped 205 -- every row under both lowerings
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules   # the same report
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules  # the same report
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules # the same report
./editors/jetbrains/gradlew -p editors/jetbrains check   # the plugin's own gate, per commit that touches it
./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin   # once at the end of a batch: downloads several IDEs
```

The three stress modes, six gate invocations, are Git Bash syntax and do
anything only in a debug build ([memory.md](memory.md), "The move stress
mode", "The gates the collector needs";
[dispatch.md](dispatch.md), "Invalidation"). Clippy's gate
includes `min_ident_chars` (`clippy.toml`), which refuses identifiers of one
or two characters: names are written out
(`CLAUDE.md`, "Conventions").

Never judge performance from a debug build; `cargo build --workspace --release`
is the one to measure, and every binary a measurement compares — the current
tree included — is built with the same explicit `RUSTFLAGS`, which pin
function alignment so that code placement cannot confound the comparison;
the shipped build is unpinned ([execution.md](execution.md), "What removing
the re-entries moved", the protocol).

## Deviations from the language specification

Places where the implementation differs from
`docs/references/newspeak-spec.pdf`. Recorded so a deviation is a decision
rather than an oversight -- and, where a row says so, to keep a defect nobody
has had time to fix from being a surprise.

Most rows only under-reject or narrow what can be expressed. Three do worse:
the sequential simultaneous slot clauses, the binary float behind a
non-integral literal and the synchronous asynchronous send each answer a
**correct** program **wrongly, at exit status 0**. Read all three before
trusting a program's output.

| Deviation | Spec | Where it is explained |
|---|---|---|
| Value objects exist and §3.1.1's second bullet is enforced, but nothing copies one across an actor boundary | §3.1.1 | [liveness-and-reflection.md](liveness-and-reflection.md) |
| Integers are `i64`; overflow raises a Newspeak error rather than promoting, where "integers are not limited in size" | §5.1.1 | [core-library.md](core-library.md) |
| A non-integral literal is a binary `f64`, not the exact-decimal `FixedPointNumber` the section names | §5.1.1 | [core-library.md](core-library.md) |
| There are no character literals and no `Character` class: `"c"` is a one-character string, as the section itself announces and §5.1.5's `"σ"` form states; a double-quoted string has no escape, and a quote in one is written `"""`, not the section's `""""` | §5.1.4 | [frontend.md](frontend.md) |
| Simultaneous slot clauses `\|\| ... \|\|` initialize sequentially, so mutual recursion between them sees **nil** | §6.3.2 | [frontend.md](frontend.md) |
| An asynchronous send runs synchronously and answers the value rather than a promise; there are no actors | §5.6 | [frontend.md](frontend.md) |
| The class category string is required, where the grammar (`corpus/reference/NewspeakGrammar.ns:202`, `classCategory = string opt.`) and the section make it optional | §8.1 | [frontend.md](frontend.md) |
| `main:args:` receives **nil** for `args:` rather than a list of the environment's arguments; there is no command-line passthrough into Newspeak | §8.4 | [image-format.md](image-format.md) |

One of these has a golden under `goldens/spec/`, recorded as `skipped`:
the sequential simultaneous slot clauses. It states what the specification
requires the program to answer, so closing the gap is deleting the
`skipped` line, and the expectation written the day the gap was found is
what then has to hold. The
asynchronous send has none: §5.6 says the result is "a promise that is
immediately returned to the sender", and with no promise to observe there
is no answer to record that would not be invented.

A `skipped` golden is not always a row here. `goldens/spec/` also records
rules the runtime simply does not implement yet, rather than places where a
decision was taken to differ: a parameter and a local of the same name
compile (§3.6). It carries its own `skipped` reason.

## What lives elsewhere

- **`CLAUDE.md`** — the invariants that are easy to break, each of which has
  already produced a real defect. Load-bearing for anyone working in the
  repository; the reasoning behind each one is in these documents.
- **`docs/PROGRESS.md`** — milestone chronology: what was completed and when,
  and, under "Where the next session starts", what comes next and in which
  order. These documents describe the design; that one records the schedule.
- **`docs/superpowers/plans/`** — implementation plans, dated. A plan is an
  execution record of finished work and is legitimately historical, so it is not
  a living document and is left as written.
- **`docs/references/`** — the primary sources, with `PROVENANCE.md` recording
  what each one is and why it is here.
