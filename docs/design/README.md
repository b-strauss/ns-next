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
| Values, objects, mixins, classes, instantiation | [object-model.md](object-model.md) | Built | `become:` |
| Name resolution, lookup, access control, sends | [dispatch.md](dispatch.md) | Built | inline caches, designed in three steps: primitives as method-table entries (3a), the caches (3b), the dispatch id's place (3c) |
| AST to bytecode: instructions, lowering, inlining | [compiler.md](compiler.md) | Built | call-site ids per code family, as `u32` (3b) |
| Activations, the VM loop | [execution.md](execution.md) | Built, including the trampoline; nothing re-enters the loop from Rust, and the loop polls a safepoint | closure conversion, JIT tiers |
| Object lifetime, identity, collection | [memory.md](memory.md) | Partial — the arena, the activation stack, the safepoint, tracing, host roots, the stress mode and the lifetime brand are built; nothing reclaims an arena slot | a collector |
| Serialization, both artifacts | [image-format.md](image-format.md) | Built | suspend and resume |
| Builtins and the platform | [core-library.md](core-library.md) | Partial | natives as method-table entries with an enumerated primitive (3a), then the core library as Newspeak source |
| Debugger, mirrors, edit-and-continue, actors | [liveness-and-reflection.md](liveness-and-reflection.md) | Planned | — |

## Architecture

Three crates, with the name-resolution pass as the seam the bytecode compiler
plugs into:

```
crates/
  ns-syntax/    lexer, tokens, AST, spans, diagnostics
  ns-runtime/   resolver, object model, dispatch, compiler, VM, builtins, image
  ns-cli/       the `ns-next` binary
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
at [memory.md](memory.md) for what the heap guarantees and what it does not —
notably that nothing reclaims an arena slot yet — and at
[execution.md](execution.md), "Activations", for the stack.

`ns-syntax` does not depend on `ns-runtime`. Nothing under `crates/ns-syntax`
should change for runtime work.

### From source to a running program

```
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

## Gates

These are non-regression gates, not suggestions. Each has caught a real
regression.

```bash
cargo test --workspace          # includes the golden program outcomes
NS_NEXT_STRESS_MOVES=1 cargo test --workspace   # the same, moving everything at the run loop's safepoints
cargo clippy --workspace --all-targets   # must be warning-free, the naming lint included
cargo fmt --all                 # before every commit
cargo run -p ns-cli -- parse-corpus corpus/modules   # parsed 210, failed 0, expected-fail 1, stale 0
cargo run -p ns-cli -- run-corpus corpus/modules     # ran 6, failed 0, skipped 205 -- every row under both lowerings
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules   # the same report
```

The two stress runs are Git Bash syntax and do anything only in a debug build
([memory.md](memory.md), "Checking the roots before a collector exists: the
stress mode"). Clippy's gate includes `min_ident_chars` (`clippy.toml`),
which refuses identifiers of one or two characters: names are written out
(`CLAUDE.md`, "Conventions").

Never judge performance from a debug build; `cargo build --workspace --release`
is the one to measure, and every binary a measurement compares — the current
tree included — is built with the same explicit `RUSTFLAGS`, which pin
function alignment so that code placement cannot confound the comparison;
the shipped build is unpinned ([execution.md](execution.md), "What removing
the re-entries moved", the protocol).

## Deviations from the language specification

Places where the implementation deliberately differs from
`docs/references/newspeak-spec.pdf`. Recorded so a deviation is a decision
rather than an oversight.

| Deviation | Spec | Where it is explained |
|---|---|---|
| No reified `Class`/`Metaclass` hierarchy; a class has a lazily created metaclass carrying its class side | §3.2 | [object-model.md](object-model.md) |
| No formal "value object" notion, because there are no actors to need it | §3.1.1 | [liveness-and-reflection.md](liveness-and-reflection.md) |
| `self`, `super`, `outer`, `true`, `false`, `nil` are not lexer-reserved; they are recognized contextually | §4.1 | [frontend.md](frontend.md) |
| Integers are `i64`; overflow raises a Newspeak error rather than promoting | — | [core-library.md](core-library.md) |
| Lazy slots are eager: the parser drops `lazy` | §6.3.3 | [frontend.md](frontend.md) |
| A builtin class name no enclosing declaration declares loads the builtin class directly, rather than being a send to `Object`'s accessor for it, so a superclass cannot override it | §3.2 | [dispatch.md](dispatch.md) |

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
