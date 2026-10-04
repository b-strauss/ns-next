# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, bytecode compiler and register VM, and a versioned image format.
Rust 1.98.0, edition 2021, six crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`;
  `parse` refuses a source with errors, `parse_for_editor` answers a
  recovered module for the editor, its recovery bounded by a work budget
  (`docs/design/frontend.md`, "Recovery").
- `crates/ns-runtime` — resolver, object model, dispatch, the compiler (`compiler/`,
  checked against the reference lowering in `compiler/reference.rs`), the
  instructions (`bytecode.rs`, stored and run in the eight-byte encoding of
  `bytecode/compact.rs`), the VM (`vm.rs`) and the send machinery it calls
  (`interpreter/`: inline caches in `inline_cache.rs` in front of the lookups
  in `resolve.rs`, the one unwinder in `unwind.rs`, the VM mirror in
  `mirror.rs` and its leaves in `builtins/`), the core library
  (`core-library/Kernel.ns`, `PlatformFactory.ns`, `CollectionsFactory.ns`,
  booted by `interpreter/kernel.rs` and `interpreter/core_library.rs`),
  `LargeInteger` arithmetic (`arbitrary_integer/`, adapted
  from `num-bigint` and `num-integer`), the heap (`heap.rs`, one object space
  of variably sized objects in `ns-memory`'s reservation (`heap/space.rs`),
  a nursery of eden and two survivor spaces below the old space,
  each object its words -- header word, class word, fields, in road step 3's
  layout (`heap/header.rs`, `heap/object.rs`; the seam's value types in
  `heap/records.rs`, the per-heap tables of mixins, applications and compiled
  code in `heap/tables.rs`) -- named by handles that are 64-bit byte
  offsets; an activation stack reified on demand into frame objects; a
  Cheney scavenger with age-based tenuring (`heap/scavenge.rs`) beside a
  mark-and-slide collector, which compacts both generations into the old
  space, the mark bit in the header and the forwarding through a live-word
  bitmap (`heap/compaction.rs`), both run only at run-loop safepoints or
  between entries; the write barrier and the remembered set in `heap.rs`'s
  store accessors; `heap/value.rs`, a value as one
  tagged word, Spur's, and `Word`, the bare word a heap object stores; the
  step state `Mutator` in `interpreter/mod.rs`, held at rest by `World` in
  `heap/world.rs` and reached only through `Interpreter::enter`) and the image
  format (`image/`).
  `docs/design/README.md`, "The map", says which document covers each part.
- `crates/ns-cli` — the `ns-next` binary, which runs programs: `run`, `run-corpus`,
  `load`, `bench`, `build`. A program is one `.ns` file or a project of several
  modules: a directory whose `newspeak.json` names them, the current directory
  when no program is given (`project.rs`, `program.rs`). The file is described
  by `schemas/newspeak.schema.json`, which `crates/ns-cli/tests/project_schema.rs`
  holds the reader to and the JetBrains plugin bundles.
- `crates/ns-analyzer` — the `ns-next-analyzer` binary, everything that looks at
  Newspeak without running it: `lsp`, a language server for `.ns` files
  (diagnostics, semantic tokens, fold ranges, document symbols); `dump-tokens`,
  which the JetBrains plugin's lexer is checked against; `parse`, `parse-corpus`,
  `disasm` and `inspect`. It never runs a program: `disasm` compiles a module or
  decodes an image, `inspect` reads an image without loading it, and neither runs
  the program's code (the interpreter `disasm` bootstraps instantiates only the
  shipped core library). Its server dependencies
  (`lsp-server`, `lsp-types`) stay in this crate and never reach `ns-runtime` or
  `ns-next`; `serde_json` is shared by `ns-analyzer` and `ns-cli` (the project
  file is JSON) (`docs/design/editor-support.md`).
- `crates/ns-tooling` — what `ns-next` and `ns-next-analyzer` share: loading and
  resolving a source file, collecting `.ns` files, printing diagnostics. It is a crate
  of its own so the analyzer's server dependencies never reach `ns-next`; it depends
  on `ns-syntax` and `ns-runtime` only.
- `crates/ns-memory` — the memory layer (page-range reservation on Windows and
  POSIX). The only crate with `unsafe`; `ns-runtime`, `ns-tooling` and `ns-analyzer` forbid it.
  The heap's object space is its user (`docs/design/memory.md`, "The memory
  layer", "The object space").

`editors/jetbrains/` — the JetBrains plugin (Kotlin, Gradle wrapper inside): a
lexer mirroring `ns-syntax`'s, compared against it token by token over every
module (`LexerComparisonTest`), the connection to `ns-next-analyzer`, and a
structure view of its own over the server's document symbols.

`benchmarks/are-we-fast-yet/` — the fourteen Are We Fast Yet benchmarks,
Primordial Soup's Ton80 Richards and DeltaBlue, and their harness, one
project, verified by
`goldens/tool/awfy/` (`docs/design/execution.md`, "The Are We Fast Yet
benchmarks").

Read `docs/design/README.md` first — it maps the living design documents and says
how much of each aspect exists. `docs/PROGRESS.md` is the milestone chronology.
`docs/OPEN-WORK.md`'s "Next" names the next piece of work, and the rest of it
lists everything still to be done: remove an item when work closes it, add one
when a review, measurement or design leaves something open.

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
(`Literal::UnresolvedIdentifier`, and `Literal::BodyTooLarge` for a body past
the `u16` register ceiling), so a method never run never fails.

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
process: validate every id, tag, count and length before it indexes, allocates
or slices; bound any recursion the input controls; never add a pre-allocation
not bounded by `bounded_count`. `image::load` verifies structure and reconciles
mixins before any decoded code runs (`image-format.md`, "Verification"). The
decoder sizes every record from its own bytes in a first pass and allocates
nothing before it (`image-format.md`, "Two passes"), then fills each
placeholder in place, so no record is built beside its placeholder. The
decoder allocates its graph only after a tenure between entries: the
platform-path sends before it may collect at their safepoints like any run,
but the graph pass allocates old, after the last tenure, and never polls a
safepoint, so nothing collects while its placeholders exist and the peak
argument stands (`image-format.md`, "The image under generations"). The
transient peak of a rejected load is about 10.7x of the graph bytes plus ≤4x
of the symbol section in a release build, and about 16x of the graph bytes
plus ≤4x of the symbol section in a debug one (`memory.md`, "The
amplification bound of a rejected load", has the breakdown and what is not
counted; `decode_value`'s doc comment the derivation). The guarantee
covers decoding only: `ns-next load` runs the packaged application's code,
recursion bounded by the 100,000-activation depth limit plus its 1,000 reserve
(`execution.md`, "The depth limit"), and an image that names platform paths
first follows them through shipped code only.

**Every store into an existing object goes through `heap.rs`'s store
accessors and their write barrier.** A scavenge reads only the young
objects and the remembered old ones, so a store that writes a word into an
old object any other way leaves the young object it names unreachable to
the next scavenge -- moved or reclaimed, silently, at exit status 0. Only an
initializing store into an object just allocated skips the barrier, which
is why every old allocation outside the image decoder is born remembered;
besides those, only the collector's own rewrites (a scavenge, a
compaction, a move), which keep the remembered set themselves, and the
replacement of resolved futures, which runs only before a full collection
empties the nursery, write words without it.
`NS_NEXT_STRESS_SCAVENGE`'s verification of the remembered set, and a
debug build's poison over the nursery a scavenge vacated, are what find a
breach (`memory.md`, "Generations").

**A wire tag is assigned once and never renumbered or reused.** Every tag lives in
`crates/ns-runtime/src/image/opcodes.rs`, pinned by `tests/opcode_tags.rs`; a new
tag takes the next free number, a removed one retires its number. Renumbering
would let a stale artifact decode into a different, valid program.

**A handle means nothing against a `Heap` that did not mint it, nor across a
move.** An `ObjectReference`, `ClassReference` or `FrameReference` is a 64-bit
byte offset into its heap's object space, an `ActivationReference` an index
into its activation stack. Never hold one across the death of its
interpreter — the offset or index answers a different, valid object in
another heap. Reach the real `Heap` (a `&Heap`
parameter, or read through `interpreter.enter` while it lives); a reader of
compiled code alone takes the mixin, which holds no handle (`compiler.md`,
"Readers of compiled code take the mixin"). A host keeps a value across a run
only as a `Root`. The debug-only origin guard, move epochs and the stress modes
catch mistakes; a release build does not. A new Rust-side field holding a
handle or a stored `Word` needs its `Trace` impl, and a new pointer word in an
object its format's arm in `heap/object.rs`'s `pointer_ranges`, which marking
and forwarding both read (`memory.md`, "A handle is space-relative", "What
the collector traces", "The move stress mode").

**The brand is the compiler's, and needs no cast.** `World::enter` hands the
`Mutator` to a closure generic over `'gc`, in safe Rust. Never add a public way
to change a handle's brand (`heap::value`'s `pub(super)` `at_rest`/`branded` are
the only conversions; there is no `unbind`), and never give `Mutator` a way to
reach the `Interpreter`. A stored `Word` carries no brand, so `Heap::load` and
`Heap::store` are part of the seam and stay `pub(crate)`. Three `compile_fail`
doctests on `Interpreter::enter` pin it (`memory.md`, "The brand").

**A running activation is an `ActivationReference`; only a `FrameReference` is
ever stored.** An activation's index is reused after its pop; a closure's
captured frame, a frame object's sender and home, and image records take a
`FrameReference`, the handle of a frame object in the object space
(`Heap::reify`; there is no separate store for frames). The run loop
pops every activation it runs on every exit. **Every instruction that reads or
writes another frame's registers by reference must answer its depth, never 0,
in `bytecode::outer_reach_of`, and its eight-byte form in
`bytecode::compact::narrow_outer_reach`**, or the frame drops its registers on
return and every read answers `nil` at exit status 0. Both matches are
exhaustive, so the compiler makes a new instruction decide; the round-trip
tests' `assert_derived_as_decoded` holds the two to each other
(`execution.md`, "Activations", "Closure conversion").

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
class** — `current_class`, then each class object's declaring-class field
(`Heap::class_declaring`). A level a `<:`
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
every send — today the class object's builtin field (`Heap::class_builtin`), a
primitive's payload check and a method's `MethodFastPath`. Add such a read and its per-send re-check goes with
it. A lazy slot's getter has no fast path (`Instruction::ReturnSlotIfFilled`
instead). The reference lowering runs every send uncached and is the oracle
(`dispatch.md`, "Primitive methods", "3b — The caches").

**A chain of messages is walked link by link.** `a b + c …` is a left-nested
`Send` as deep as the chain is long, and every chain length is valid
Newspeak, so no depth guard bounds it: never recurse along a receiver —
walk `Send::receiver_chain`; `Send` drops itself iteratively. The compiler
still recurses (`OPEN-WORK.md`) (`frontend.md`, "Recovery", rule 7).

## Working in this repository

The suites run under `cargo-nextest` (`cargo install cargo-nextest --locked`).
Before every commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets          # must be warning-free
cargo nextest run --workspace                   # every golden, both lowerings
cargo test --workspace --doc                    # nextest runs no doctests
cargo run -p ns-analyzer -- parse-corpus corpus/modules   # "parsed 211, failed 0"
```

Plus, per commit touching them:

- `crates/ns-memory` → `cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu`
  and the same for `aarch64-apple-darwin`
- `editors/jetbrains`, `crates/ns-analyzer`, `crates/ns-syntax` or `schemas/` →
  `./editors/jetbrains/gradlew -p editors/jetbrains check` (the plugin bundles
  `schemas/newspeak.schema.json`, so a change to it reaches the plugin; when
  unsure whether a change reaches the plugin, run it)

`.config/nextest.toml`'s default profile leaves out the tests that run the Are
We Fast Yet benchmarks at full size; they run only on demand, when
benchmarking, and once at the end of a branch, never under a stress mode:
`cargo nextest run -p ns-cli --profile benchmarks` (`execution.md`, "The Are
We Fast Yet benchmarks").

The end-of-batch gates (the five stress modes: moves, caches, collection,
scavenges and every instruction wide; `run-corpus` plain and under
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
Only a release build runs the size tests (`a_handle_and_an_optional_handle_are_one_word`,
`a_resolution_stays_the_size_the_hot_path_was_measured_at` and
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
  `crates/ns-runtime/src/heap.rs`.** The handles' inner index is private to the
  `heap` module (`heap.rs` and its submodules, `heap/value.rs` among them, which
  puts the index into a word); add a missing accessor in `heap.rs`. Accessors
  copy out rather than lend out (`memory.md`, "The seam").
