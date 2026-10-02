# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, bytecode compiler and register VM, and a versioned image format.
Rust 1.98.0, edition 2021, five crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`.
- `crates/ns-runtime` — resolver, object model, dispatch, the compiler (`compiler/`,
  checked against the reference lowering in `compiler/reference.rs`), the VM
  (`vm.rs`) and the send machinery it calls (`interpreter/`: inline caches in
  `inline_cache.rs` in front of the lookups in `resolve.rs`, the one unwinder in
  `unwind.rs`, the VM mirror in `mirror.rs` and its leaves in `builtins/`), the
  core library (`core-library/Kernel.ns`, `PlatformFactory.ns`,
  `CollectionsFactory.ns`, booted by `interpreter/kernel.rs` and
  `interpreter/core_library.rs`), `LargeInteger` arithmetic (`arbitrary_integer/`,
  adapted from `num-bigint` and `num-integer`), the heap (`heap.rs`, an arena of
  32-bit handles, an activation stack reified on demand, a mark-compact
  collector run only at run-loop safepoints; `heap/value.rs`, a value as one
  tagged word, Spur's, and `Word`, the bare word a heap object stores; the
  step state `Mutator` in `interpreter/mod.rs`, held at rest by `World` in
  `heap/world.rs` and reached only through `Interpreter::enter`) and the image
  format (`image/`).
  `docs/design/README.md`, "The map", says which document covers each part.
- `crates/ns-cli` — the `ns-next` binary: `parse`, `parse-corpus`, `run-corpus`, `run`,
  `load`, `bench`, `disasm`, `inspect`, `build`. A program is one `.ns` file or a
  JSON project file of several modules (`project.rs`, `program.rs`).
- `crates/ns-lsp` — the `ns-next-lsp` binary: a language server for `.ns` files
  (diagnostics, semantic tokens) and `dump-tokens`, which the JetBrains plugin's
  lexer is checked against. It never starts the VM; its server dependencies
  (`lsp-server`, `lsp-types`) stay in this crate and never reach `ns-runtime`,
  and `serde_json` is shared by `ns-lsp` and `ns-cli` (the project file is JSON)
  (`docs/design/editor-support.md`).
- `crates/ns-memory` — the memory layer (page-range reservation on Windows and
  POSIX). The only crate with `unsafe`; `ns-runtime` and `ns-lsp` forbid it.
  Nothing uses it yet (`docs/design/memory.md`, "The memory layer").

`editors/jetbrains/` — the JetBrains plugin (Kotlin, Gradle wrapper inside): a
lexer mirroring `ns-syntax`'s, compared against it token by token over every
module (`LexerComparisonTest`), and the connection to `ns-next-lsp`.

`benchmarks/are-we-fast-yet/` — the fourteen Are We Fast Yet benchmarks,
Primordial Soup's Ton80 Richards and DeltaBlue, and their harness, one
project, verified by
`goldens/tool/awfy/` (`docs/design/execution.md`, "The Are We Fast Yet
benchmarks").

Read `docs/design/README.md` first — it maps the living design documents and says
how much of each aspect exists. `docs/PROGRESS.md` is the milestone chronology;
its "Where the next session starts" names the next piece of work.
`docs/OPEN-WORK.md` lists everything still to be done: remove an item when work
closes it, add one when a review, measurement or design leaves something open.

## Invariants that are easy to break

Each has already produced a real defect. The rule is stated here; the mechanism
and its history are in the design document named with it. Read that section
before changing code the rule governs.

**Program outcomes are pinned, not re-derived.** Every `goldens/**/*.ns`
is held to the `status`, `stdout` and `stderr` in the `.toml` beside it, under
both the optimized and the reference lowering, and `run-corpus` likewise. A
change that alters one is a behaviour change: establish that the new output is
right before updating the golden, never because the code now produces it
(`docs/design/README.md`, "The goldens"; `compiler.md`, "The reference lowering").

**`vm.rs` holds no dispatch logic.** Send, `doesNotUnderstand:`, lexical, super
and native dispatch, frame construction, `instantiate` and the inline caches live
in `interpreter/`; the VM's loop only calls them, so the reference lowering and a
later JIT reach the same dispatch and raise the same errors. Never copy dispatch
logic, lookup logic or error text into `vm.rs` or `compiler/`; widen to
`pub(crate)` instead. A send hands `feedback_base`, `family_size` and the
`CallSite` to `interpreter/inline_cache.rs` and does nothing else with them;
`SendMirror`'s arm calls `interpreter::mirror::send_inlined`, which decides the
leaf.

**Compiling an already-resolved module can never raise a `NewspeakError`.** It is
a total, deterministic lowering; a compiler-invariant violation is `panic!` /
`debug_assert!`. A program-level error is deferred to execution
(`Literal::UnresolvedIdentifier`), so a method never run never fails.

**An expression's value register must still hold that value when its consumer
executes.** A named local's register is live storage; `lower_operand` copies it,
and an assignment through a setter copies its value, whenever something lowered
afterwards can write that register — including when a setter send runs user code
before the very next instruction. Whether something can write it is a question
about the register (`a_send_can_write(register)`), not the send: only an
assignment in the same body (inlined blocks included) and the fallback closure
of an inlinable conditional that assigns the local can. Parameters and immutable
locals are never copied; that rests on `ScopeStack::resolve_setter` and is
checked by `lower_assign`'s `debug_assert!`s against `read_only_locals`. Getting
it wrong is silent wrong output at exit status 0 (`execution.md`, "Fewer
protective copies").

**An image is untrusted input.** No byte sequence may panic or abort the
process: validate every id, tag, count and length before it indexes, allocates or
slices; bound any recursion the input controls; never add a pre-allocation not
bounded by `bounded_count`. `image::load` verifies structure and reconciles
mixins before any decoded code runs (`image-format.md`, "Verification"). The
transient peak of a rejected load is about 16.0x the graph section's bytes in a
release build, 23.1x with the integers and floats it allocates (debug 20.0x and
28.9x; `memory.md` has the breakdown and what is not counted). The guarantee covers decoding only:
`ns-next load` runs the packaged application's code, recursion bounded by the
100,000-activation depth limit plus its 1,000 reserve (`execution.md`, "The
depth limit"), and an image that names platform paths first follows them
through shipped code only.

**A wire tag is assigned once and never renumbered or reused.** Every tag lives in
`crates/ns-runtime/src/image/opcodes.rs`, pinned by `tests/opcode_tags.rs`; a new
tag takes the next free number, a removed one retires its number. Renumbering
would let a stale artifact decode into a different, valid program.

**A handle means nothing against a `Heap` that did not mint it, nor across a
move.** Never hold an `ObjectReference`, `ClassReference`, `FrameReference` or
`ActivationReference` across the death of its interpreter — the index answers a
different, valid object in another heap. Reach the real `Heap` (a `&Heap`
parameter, or read through `interpreter.enter` while it lives, as `disasm` does).
A host keeps a value across a run only as a `Root`. The debug-only origin guard,
move epochs and the stress modes catch mistakes; a release build does not. A new
field holding a handle or a stored `Word` needs its `Trace` impl (`memory.md`, "A
handle is arena-relative", "The move stress mode").

**The brand is the compiler's, and needs no cast.** `World::enter` hands the
`Mutator` to a closure generic over `'gc`, in safe Rust. Never add a public way
to change a handle's brand (`heap::value`'s `pub(super)` `at_rest`/`branded` are
the only conversions; there is no `unbind`), and never give `Mutator` a way to
reach the `Interpreter`. Three `compile_fail` doctests on `Interpreter::enter`
pin it (`memory.md`, "The brand").

**A running activation is an `ActivationReference`; only a `FrameReference` is
ever stored.** An activation's index is reused after its pop; `ClosureData`,
`Frame` and image records take a `FrameReference` (`Heap::reify`). The run loop
pops every activation it runs on every exit. **Every instruction that reads or
writes another frame's registers by reference must be named in
`bytecode::outer_reach_of`**, or the frame drops its registers on return and
every read answers `nil` at exit status 0 (`execution.md`, "Activations",
"Closure conversion").

**An unwind target is a `FrameReference`.** Non-local return, `return:`,
`resume:`, `retry` and the depth reserve name the target's frame; the unwinder
asks `Heap::frame_activation` afresh each time it carries on, because `ensure:`
and `ifCurtailed:` blocks run meanwhile and can reuse the index. Release builds
do not check generations (`execution.md`, "Unwinding").

**Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` / `{:?}` an
`EvaluationResult`.** Use `Heap::describe` in Newspeak-facing text. `Signal`,
`HostSignal`, `HostError` and `Root` have no `Debug` on purpose, so these do not
compile; tests use a local `expect_ok` helper — copy a neighbour's.

**A class object is per enclosing instance; behaviour is the
`MixinApplication`, shared.** The application cache is keyed by the
superclass's *application*, never by its class object, or every instance mints
its own application (`object-model.md`, "Three layers, not one").

**Compiled code lives on the mixin, and `Instruction::InitializeSlot`'s
`own_index` is mixin-relative**, resolved against the activation's `slot_base`
at run time, because one mixin's code is shared by every application of it;
`vm_initializer.rs` pins it (`compiler.md`).

**`outer` walks the *lexical* declaration chain, never the receiver's dynamic
class** — `current_class`, then `ClassData::declaring_class`. A level a `<:`
clause adds takes its enclosing object and declaring class from the class the
clause names (`interpreter/nested.rs`, `build_applied_levels`).

**A `<:` clause's message runs with the applying class's factory arguments, just
before the initializer of the level it adds** (§6.2; `object-model.md`, "Mixin
application"; pinned by `goldens/language/class/mixin/mixin_initializer_order.ns`).

**The VM mirror is a capability.** Whoever holds it runs every leaf, so only the
shipped core-library modules reach it. Six locks keep it there; never loosen one
and never add a way for a program to obtain the mirror, its class or a
`MirrorSend` binding: (1) only `resolver::resolve_kernel` records
`Binding::MirrorSend`, and only the shipped modules are resolved with it; (2)
the bootstrap checks the mirror slots of `Kernel` and `PlatformFactory`, and no
program reaches a shipped root (the factory pattern); (3) the encoder names
shipped instances by platform path (`image/platform_paths.rs`) and refuses one
no path names; (4) no program can name the mirror's class or `Top`, and the
decoder refuses builtin class tags 14 and 15 (`read_referenced_builtin_class`);
(5) the encoder refuses a graph reaching the mirror, its class, `Top` or their
metaclasses, and `decide_factory` refuses to instantiate the mirror's class;
(6) `SendMirror` has no wire tag (`core-library.md`, "The VM mirror is the
whole Rust boundary").

**A cache records what the lookup decided; it never decides.** An inline-cache
hit is `act` on the resolution `resolve_*` answered for the same key; an entry
holds no handle. Anything the key does not cover is read afresh in `act` on
every send — today `ClassData.builtin`, a primitive's payload check and a
method's `MethodFastPath`. Add such a read and its per-send re-check goes with
it. A lazy slot's getter has no fast path (`Instruction::ReturnSlotIfFilled`
instead). The reference lowering runs every send uncached and is the oracle
(`dispatch.md`, "Primitive methods", "3b — The caches").

## Working in this repository

The suites run under `cargo-nextest` (`cargo install cargo-nextest --locked`).
Before every commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets          # must be warning-free
cargo nextest run --workspace --exclude ns-lsp  # every golden, both lowerings
cargo test --workspace --exclude ns-lsp --doc   # nextest runs no doctests
cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
```

Plus, per commit touching them:

- `crates/ns-lsp` or `crates/ns-syntax` → `cargo nextest run -p ns-lsp`
- `crates/ns-memory` → `cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu`
  and the same for `aarch64-apple-darwin`
- `editors/jetbrains`, `crates/ns-lsp` or `crates/ns-syntax` →
  `./editors/jetbrains/gradlew -p editors/jetbrains check` (when unsure whether
  a change reaches the plugin, run it)

`.config/nextest.toml`'s default profile leaves out the tests that run the Are
We Fast Yet benchmarks at full size; they run only on demand, when
benchmarking, and once at the end of a branch, never under a stress mode:
`cargo nextest run -p ns-cli --profile benchmarks` (`execution.md`, "The Are
We Fast Yet benchmarks").

The end-of-batch gates (the three stress modes, `run-corpus` plain and under
stress, the benchmarks profile, `verifyPlugin`) are in the `finishing-a-batch`
skill. A commit that
never saw them is work in progress, not finished work; a subagent implementing
part of a batch is told this staggering explicitly.

A commit that changes only Markdown (check `git diff --stat`) runs no gates. A
change to Rust, a test, a fixture, a golden or `Cargo.toml` runs them however
small.

A change to `crates/ns-syntax/src/lexer.rs` is mirrored in
`editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt`
in the same commit. Nothing under `crates/ns-syntax` should change for runtime work.

Never judge performance from a debug build (the `measuring-performance` skill).
Only a release build runs the size tests (the `Object` size and
`a_signal_fits_in_the_niche_of_its_one_unboxed_error`):
`cargo nextest run --workspace --release`.

Every corpus module must parse; there is no list of exceptions. A module that
runs end to end belongs in `corpus/runnable.toml` with the unary selector its
generated application sends it (`send`) and its output. `PlatformFactory.ns`
and `CollectionsFactory.ns` are widened only on demand: an accessor exists
because a `runnable.toml` module or a program under `benchmarks/` needs it,
named in the accessor's comment.

Add no new external dependency without a reason that survives review — the image
format is hand-rolled on purpose.

**Search the whole specification before concluding it is silent.**
`docs/references/newspeak-spec.md` transcribes `newspeak-spec.pdf`; a rule often
sits in a section whose heading looks unrelated. Cite the PDF. The older papers
are background only; where the specification is silent, the canonical
implementation decides (`docs/references/PROVENANCE.md`).

**Design decisions follow the principles in `docs/design/README.md`**: a
deviation from the specification is a defect; no valid construct is carved out
because it is rare; performance decides among semantics-preserving options,
judged by the finished system; a guarantee is made by arranging Newspeak classes
before adding a runtime rule; `unsafe` behind a safe abstraction is acceptable.
Ground each decision in `prior-art/` (the `making-design-decisions` skill;
`prior-art/PROVENANCE.md`). Copying code out of `prior-art/` is a licensing
decision, made so far only for `num-bigint` and `num-integer`
(`arbitrary_integer/`), SOMns and Primordial Soup (`benchmarks/are-we-fast-yet/`),
and Primordial Soup again for `CollectionsFactory.ns`'s `List`, `Map` and `Set`.

## Conventions

- All code, identifiers, comments, commit messages and developer documentation
  are in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- **Names are written out.** No identifier of one or two characters and no
  truncated word, in every kind of name and every segment of a compound:
  `mutator`, `index`, `error`, `interpreter` (not `interp`), `program_counter`
  (not `pc`), `arguments`, `argument_count` (not `num_args`), `ObjectReference`
  (not `ObjRef`). Exceptions: type parameters and lifetimes (`T`, `'a`, `'gc`);
  the words `ok`, `id`, `min`, `max`, `temp`, `And`, `Or`; names the workspace
  does not declare (`str`, `len()`, `fmt`, `Rc`, `Err`); and names bound to a
  file or interface (`vm`, `disasm`, `g1`/`g2`). Clippy's `min_ident_chars`
  enforces the short ones. Code samples in plans follow the same rule.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`).
- Design documents in `docs/design/` are living: update the document for the
  aspect rather than adding a dated one. Plans live in
  `docs/superpowers/plans/` as `YYYY-MM-DD-HHMM-<name>.md` and are left as
  written once complete.
- Documentation other than a plan describes the current state — `PROGRESS.md`
  and `OPEN-WORK.md` included. A statement no longer true is deleted or
  rewritten, never labelled historical. A reason still true stays.
- Before a branch is merged, a fresh session must be able to start from the
  documentation alone (the `finishing-a-batch` skill checks it).
- A Newspeak module inline in a test is an `indoc! {"…"}` (or `formatdoc!`),
  indented with spaces with the Rust around it, the closing `"}` on its own
  line — not a one-line literal of `\n` escapes.
- Comments explain the decision — what a check prevents — not the mechanics.
- **A public member of a shipped core-library module is documented.** Every
  `public` method, slot and class of `Kernel.ns`, `PlatformFactory.ns` and
  `CollectionsFactory.ns` has a `(* … *)` comment directly above it, or shares
  one with the family it belongs to (`+ - *`, the comparisons, the class
  accessors on `Object`). It describes what the member does as it stands: what
  it answers, what it takes, and which Error it signals and when — one to four
  lines, no more. It names no other implementation. A comment that explains a
  decision, as the rule above asks, goes after it.
- **Object, frame and activation access goes through
  `crates/ns-runtime/src/heap.rs`.** The handles' inner index is private to it
  and its submodule `heap/value.rs`; add a missing accessor there. Accessors copy out rather than lend out
  (`memory.md`, "The seam").
