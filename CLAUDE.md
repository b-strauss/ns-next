# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, bytecode compiler and register VM, and a versioned image format.
Rust 1.98.0, edition 2021, four crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`.
- `crates/ns-runtime` — resolver, object model, dispatch, the compiler (`compiler/`,
  with the reference lowering it is checked against in `compiler/reference.rs`),
  the VM (`vm.rs`) and the send machinery it calls (`interpreter/`, the inline
  caches in `interpreter/inline_cache.rs` in front of the lookups in
  `interpreter/resolve.rs`), the core library -- `core-library/Kernel.ns`,
  Newspeak source the bootstrap compiles into the builtin classes
  (`interpreter/kernel.rs`), in §3.2's and §3.1.1's hierarchy (`Top`, then
  `Object`, then `Value` over the value classes and `Behavior` over `Class`
  and `Metaclass`; every class object's class is its own metaclass, its class
  side applied to `Class`, and every metaclass's class is `Metaclass`,
  `interpreter/metaclass.rs`), with §8.6's exception classes (`Exception`,
  `Error`, `MessageNotUnderstood`, `Halt` and `Message`, which programs reach
  through `platform kernel`, over kernel-private `Activation` objects;
  `on:do:`, `ensure:` and `ifCurtailed:` are marked `Block` methods, and the
  one unwinder, `interpreter/unwind.rs`, carries out every unwind), whose
  methods reach Rust only through the one VM
  mirror object and its leaves (`builtins/`, `interpreter/mirror.rs`); beside
  it `core-library/PlatformFactory.ns`, whose root holds the mirror and is
  never handed out, and whose nested `Platform` is the platform every program
  is handed, with lazy members and class accessors, booted from the table of
  shipped modules (`interpreter/core_library.rs`) -- and the image format
  (`image/`).
  Objects live in an arena owned by one `Heap` (`heap.rs`) and are addressed
  by 32-bit handles; activations live on an
  activation stack in the same `Heap` and are reified into the frame arena only
  when a block captures one. A mark-compact collector reclaims a slot once
  nothing reaches it, only at a safepoint of the run loop. The heap and
  everything else a step of the run loop uses are the step state, `Mutator`
  (`interpreter/mod.rs`), held at rest by `World` (`heap/world.rs`) and reached
  only through `Interpreter::enter`.
- `crates/ns-cli` — the `ns-next` binary: `parse`, `parse-corpus`, `run-corpus`, `run`,
  `snapshot`, `load`, `bench`, `disasm`, `inspect`, `build`.
- `crates/ns-lsp` — the `ns-next-lsp` binary: a language server for `.ns` files
  (lexer, parser and resolver errors, and semantic tokens from the resolver's
  bindings) and `dump-tokens`, which the JetBrains plugin's lexer is checked
  against. It never starts the VM. Its dependencies (`lsp-server`, `lsp-types`,
  `serde_json`) stay in this crate. See `docs/design/editor-support.md`.

`editors/jetbrains/` — the JetBrains plugin (Kotlin, Gradle wrapper inside): a
lexer mirroring `ns-syntax`'s, compared against it token by token over every
module (`LexerComparisonTest`), and the connection to `ns-next-lsp`.

Read `docs/design/README.md` first — it maps the living design documents and says
how much of each aspect exists. `docs/PROGRESS.md` is the milestone chronology
beside it; its "Where the next session starts" names the next piece of work.

## Invariants that are easy to break

These are not style preferences. Each one has already produced a real defect in this
repository; the reasoning behind them is in the design documents under
`docs/design/`.

**Program outcomes are pinned, not re-derived.** The reference lowering
(`docs/design/compiler.md`, "The reference lowering") is a second way to run
every program, and both are held to recorded answers: every
`goldens/<category>/*.ns` against the `status`, `stdout` and `stderr` recorded in
the `.toml` beside it (`docs/design/README.md`, "The goldens"), and `run-corpus`.
A change that alters one is a behaviour change: establish that the new output is right
before updating the golden, never update it because the code now produces it.

**`vm.rs` holds no dispatch logic.** Send, `doesNotUnderstand:`, lexical, super and
native dispatch, frame construction and `instantiate` live in `interpreter/`; the VM's
instruction loop calls them. Never copy dispatch logic, lookup logic or error text into
`vm.rs` or `compiler/`: the reference lowering and, later, a JIT must reach the same
dispatch and raise the same errors, and a second copy is how they would drift. If you
need something private there, widen it to `pub(crate)`. The inline caches live in
`interpreter/` too: a send in `vm.rs` hands the running activation's `feedback_base`, its
code's `family_size` and the instruction's `CallSite` to `interpreter/inline_cache.rs` and
does nothing else with them. Likewise `SendMirror`'s arm reads its registers, calls
`interpreter::mirror::send_inlined`, and jumps to the failure block or delivers what it was
handed; which leaf runs, and whether the receiver is the mirror, are decided there.

**Compiling an already-resolved module can never raise a `NewspeakError`.** Compilation
is a total, deterministic lowering; a compiler-invariant violation is `panic!` /
`debug_assert!`. A *program-level* error the language raises at evaluation time is
deferred to execution instead (see `Literal::MalformedInteger` and
`Literal::UnresolvedIdentifier`), so a method that is never run never fails.

**An expression's value register must still hold that value when its consumer executes.**
A named local's register is live storage. `lower_operand` copies it when anything lowered
afterwards can write it, and an assignment through a setter copies its value under the same
condition, even when the very next instruction reads it, because the setter send runs user
code before that consumer reads. **Which sends can write a local is a question about the
register, not about the send.** Since closure conversion, exactly two things write a named
local of a running activation under the optimized lowering: an assignment lowered into that
same body, including inside an inlined block, and -- for a local that an inlinable
conditional's block assigns -- the fallback closure that conditional's guard failure creates,
which any later send may run. A converted closure writes no register of another activation at
all: a captured written variable lives in its scope's indirection vector, whose read lands in
a scratch register, and a copied register is read-only. So the lowering asks
`a_send_can_write(register)` per register rather than treating every send as a write
(`docs/design/execution.md`, "Fewer protective copies"); the reference lowering reaches
everything by reference and copies every read of a local unconditionally, which is what makes
it the oracle for a mistake here. `lower_expression_for` skips the copy only
where its caller states that nothing reads the value (`ValueUse::Discarded`). A parameter's
or an immutable local's register is never copied, whatever else is lowered afterwards: no
assignment can bind to one (§3.6), so nothing but its own initialization ever writes it. This
rests entirely on the resolver passing a setter send by every parameter and every immutable
local (`ScopeStack::resolve_setter`); `lower_assign`'s `debug_assert!`s check every write
into a named local, whether it is the current frame's own register (`Binding::Local`, and
`Binding::OuterLocal` at `hops == 0`) or an enclosing frame's (`StoreOuterLocal`,
`hops != 0` -- the write from a fallback closure the copy protects against, checked
against the target frame's own `read_only_locals` table, which `lower_block_literal`
hands each nested `Lowerer` alongside its scope stack and its `Layout`). A vector slot needs
no such check: only a variable something writes is ever put in a vector. Getting this
wrong is silent wrong output with exit status 0, not a crash.

**An image is untrusted input.** No byte sequence may cause a panic or a process abort:
validate every id, tag, count and length before it indexes, allocates or slices, and
bound any recursion whose depth the input controls. `image::load` also runs two
structural verification passes, and reconciles every decoded mixin with the
bodies it carries, before any decoded code can run. See
`docs/design/image-format.md`'s "Verification" section for the scope of what they
do and do not guarantee. One thing to keep in mind while decoding: the decoder
allocates one placeholder object and reified frame per declared record before it
can validate any of them, and nothing collects during a load, because the decoder
never polls a safepoint, so a rejected image holds its placeholders until the load
finishes. That is a transient peak, not a
leak — `image::load` owns its `Interpreter` in a local, so every `Err` path drops
it and both arenas with it — but the peak is amplified against the bytes remaining
at the graph section, and both `bounded_count` calls divide the *same* remainder,
so one file maxes out both arenas rather than trading one against the other:
about **16.9x in a release build** (an `Object` is 56 bytes, which a release-build test
pins, against a 6-byte minimum record, a frame-arena entry 136 against 18) and about
22.2x in a debug
build, where both are larger because every handle carries its heap's id and its
move epoch. `bounded_count` is what keeps this proportional to
the input at all rather than to a `u32` — do not add a pre-allocation that is not
bounded that way. `docs/design/memory.md` has the breakdown.

That guarantee covers **decoding**. `ns-next load` on a stateless binary instantiates
and sends the recorded entry with no flag at all, so loading one *runs its code* --
with the same exposure as running the equivalent source. Recursion depth is bounded
by a counted limit of 100,000 activations -- an ordinary Newspeak error, signalled
and catchable, not a native stack overflow, since nothing re-enters the run loop
from Rust -- plus a reserve of 1,000 held only while an error raised near the
limit is being signalled, past which the run ends (see
`docs/design/execution.md`, "The depth limit"). A mutated binary that decodes
cleanly can still abort while executing -- a native indexing `arguments` it was not
given, for one -- and that is not a decoder defect. Loading a stateful image runs
shipped code before its graph decodes: each platform path it lists (at most 4,096
of at most 32 members) is followed by unary sends, each step a public slot getter,
nested-class accessor or pure class accessor (`^outer <Root> <Nested>`) of
`PlatformFactory.ns`, which can run a lazy member's initializer or a nested class's
superclass clause -- shipped code, never the image's.

**A wire tag is assigned once and never renumbered or reused.** Every instruction,
literal and graph-section tag is declared in `crates/ns-runtime/src/image/opcodes.rs`
and read from there by the encoder, the decoder and `inspect`'s structural walk alike;
`crates/ns-runtime/tests/opcode_tags.rs` pins each value. A new tag takes the next free
number, a removed one retires its number. This is stricter than the rest of the format
needs — images are disposable and `FORMAT_VERSION` is never bumped — because
renumbering fails in the one way the untrusted-input guarantee does not cover: a stale
artifact would decode into a *different, perfectly valid* program and run it, reporting
nothing. Retiring numbers means those bytes hit an unknown tag and are refused.

**A handle means nothing against a `Heap` that did not mint it.** An `ObjectReference`, a
`ClassReference`, a `FrameReference` and an `ActivationReference` are indices into one `Heap`'s arenas
and activation stack. Nothing outside the heap module can build a `Heap` to
stand in for one that is out of scope — `Heap::new` is private to it, and
`World` builds the one an interpreter has. Never hold a handle across the
death of the interpreter that allocated it: the index is in range in the other
heap too, so the read answers a **different, perfectly valid object** rather
than failing. Reach the real `Heap` instead:
`image::encode::encode_code_table` and `image::mixins::plan` take a `&Heap`
parameter for exactly this reason, and `ns-cli`'s `disasm` command
(`crates/ns-cli/src/commands/disasm.rs`) — which builds its own class object and
so has no parameter to take — reads through `interpreter.enter` while that
interpreter is still alive, before it goes out of scope. A
`debug_assertions`-only origin guard turns the mistake into a panic at the read,
so a debug-build test run catches it; a release build does not, which is why
this is an invariant and not a check to rely on.

A handle also means nothing across a move. A host keeps a value across a run
only as a `Root` (`Mutator::stash`/`fetch` inside an entry, `Interpreter::stash`
for an immediate, and every host entry takes and answers them), and the
compiler enforces it: every movable handle is branded with the entry that
minted it (`Interpreter::enter`), so one that leaves its entry does not
compile, and a value fetched from a root is good until that entry ends. In a
debug build every handle also carries its move epoch and every accessor checks
it, and `NS_NEXT_STRESS_MOVES=1` moves everything at the run loop's safepoints
— at every one while the traced heap is within `STRESS_MOVE_BUDGET` (1,000
entries), and paced by heap size above it (`interpreter/safepoint.rs`) —
and `NS_NEXT_STRESS_COLLECT=1` moves everything too, by collecting instead of
moving, on the same schedule — so the stress runs under "Working in this
repository", below, still catch a
handle the tracer (`trace.rs`) does not know about, which the brand cannot
see. Add a field that holds a handle to any runtime type, and its `Trace` impl
stops compiling until it says what to do with it.

**The brand is the compiler's, and needs no cast.** `heap/world.rs`'s
`World::enter` hands the at-rest `Mutator<'static>` to a closure generic over
`'gc` (`for<'gc> FnOnce(&mut Mutator<'gc>) -> R`), in safe Rust: the closure
is checked for every `'gc`, so it can neither assume the brand is `'static`
nor let a branded value out. The crate forbids unsafe code
(`#![forbid(unsafe_code)]`) and the brand needs none. What keeps it meaning
something: never add a public way to change a handle's brand — `heap.rs`'s
private `at_rest` and `branded`, one per direction, for the host root table,
are the only conversions, and there is no `unbind` — and never give `Mutator`
a way to reach the `Interpreter`,
which is what keeps a step (a decision, a native, a continuation) from
running the loop. The three `compile_fail` doctests on `Interpreter::enter`
pin the mistakes (`docs/design/memory.md`, "The brand").

**A running activation is an `ActivationReference`; only a `FrameReference` is ever
stored.** An `ActivationReference` indexes the `Heap`'s activation stack and is
valid only until its entry is popped -- the next activation at that depth
reuses the index, so a handle kept past its pop reads a different, perfectly
valid activation. `ClosureData`, `Frame` and every image record take a
`FrameReference` (get one with `Heap::reify`), so storing an `ActivationReference` is a
compile error; a `debug_assertions`-only generation check turns a stale
read into a panic. The run loop pops every activation it runs, on every exit,
error paths included (`vm::run`) -- `Interpreter::run_from_host`, the one way
a host's `send` or `instantiate` enters the loop, debug-asserts that the stack
is balanced. A reified frame is married to its
live entry and divorced on pop, and the divorce copies its registers out only
when something can still read them by reference: the reference lowering's
blocks, and the fallback closure an inlined conditional sends to a non-Boolean
receiver. Converted code copies what it captures into the closure, or shares
its scope's indirection vector, so it needs none of them. Which it is, is
decided per activation from the block's `outer_reach`
(`docs/design/execution.md`, "Activations", "Closure conversion"). **Every
instruction that reads or writes another frame's registers by reference must be
named in `bytecode::outer_reach_of`**, or `MakeClosure` leaves the creating
activation unmarked, its frame drops its registers on return, and every read
through it answers `nil` at exit status 0.

**An unwind target is a `FrameReference`.** A non-local return, an exception's
`return:`, `resume:` and `retry` (`Signal::Return`, `Signal::Restart`) and the
depth limit's reserve (`Mutator.depth_reserve`) name the frame of the activation
they aim at, never its `ActivationReference`, and the unwinder asks
`Heap::frame_activation` for the running activation afresh each time it carries
on (`interpreter/unwind.rs`). The reason is that code runs while an unwind is in
progress -- every `ensure:` and `ifCurtailed:` block it passes -- and an
activation that code pushes can take the stack index a popped target used. **A
release build does not check an activation's generation**, so an
`ActivationReference` kept across those blocks would land the return in whichever
activation reused the index, at exit status 0; a frame is divorced on its
activation's pop and married to nothing afterwards, so a target popped meanwhile
is found dead instead. `interpreter::unwind`'s
`a_target_survives_its_index_being_reused` and
`a_later_signal_at_the_same_index_does_not_inherit_the_reserve` pin it
(`docs/design/execution.md`, "Unwinding").

**Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` / `{:?}` an `EvaluationResult`.**
`ObjectReference`, `FrameReference` and `ActivationReference` each carry a hand-written `Debug` that prints
identity only, so `Debug` on a `Value::Object` no longer recurses — but it still renders
Rust internals such as `ObjectReference(#42)`, which is useless in a Newspeak-facing message
where `Heap::describe` answers "the String \"foo\"". `Signal` deliberately has no
`Debug` impl of its own, which means `EvaluationResult` (`Result<Value, Signal>`) has none
either: `Result::unwrap`/`expect` require `E: Debug`, so calling either — or writing
`{:?}` on an `EvaluationResult` — is a compile error, not a lapse a reviewer has to catch by
hand. The same holds for a `HostResult` (`Result<Root, HostSignal>`), the result a
host receives: `HostSignal`, `HostError` and `Root` have no `Debug`, so `unwrap` /
`expect` on one does not compile either. Tests unwrap through a local `expect_ok`
helper — copy the one in a neighbouring test file.

**A class object is per enclosing instance; behaviour is the `MixinApplication`, shared.**
`ClassData` carries identity and an `enclosing` object; method lookup, slots and layout
live on `MixinApplication`, one per (mixin, superclass application) pair. **The
application cache is keyed by the superclass's *application*, never by its class
object** — a nested superclass's class object is itself per-instance, so a class-object
key would never hit and every instance would mint its own application: wrong per §6.2,
and every call site megamorphic since no two instances would ever share behaviour.

**Compiled code lives on the mixin, and `Instruction::InitializeSlot`'s `own_index` is
mixin-relative.** One mixin backs every application of it, so each body is
compiled once, onto the mixin — a method's onto its `MixinMethod` — and
shared by every application; an absolute field index baked into that shared
code would be wrong the moment a second application inserts different
inherited slots ahead of it. The VM resolves `own_index` against the
executing activation's `slot_base` (`MixinApplication.own_slot_base` for
that level) at run time. Getting it wrong is silent wrong output under one
application and correct output under another; `vm_initializer.rs` pins both.

**`outer` walks the *lexical* declaration chain, never the receiver's dynamic class.**
The running activation's `current_class` seeds it and `ClassData::declaring_class`
continues it level by level. A method or nested-class accessor inherited across a
nesting boundary runs with the class that lexically declares it, not the receiver's own
class — using the receiver's class instead silently resolves `outer` to the wrong
enclosing object. A level a `<:` clause adds (§6.2) takes its enclosing object and
declaring class from the class the clause names, never from the declaration
applying it (`interpreter/nested.rs`, `build_applied_levels`).

**A `<:` clause's message runs with the applying class's factory arguments, just
before the initializer of the level it adds** — `ms`, `S`'s initializers, `m1`,
`M1`'s, …, then `C`'s own (§6.2; `docs/design/object-model.md`, "Mixin
application"). Evaluating every message on the way up, or with the level's own
arguments, gives a wrong order or wrong values at exit status 0;
`goldens/mixin/mixin_initializer_order.ns` pins it.

**The VM mirror is a capability.** Whoever holds the one VM mirror object can run every
leaf, so it reaches the shipped core-library modules and nothing else
(`docs/design/core-library.md`, "The VM mirror is the whole Rust boundary"). Six things
keep it there, and loosening any one of them lets a program claim every primitive: only
`resolver::resolve_kernel` records `Binding::MirrorSend`, the mark `SendMirror` and
primitive methods are compiled from, and only the shipped core-library modules --
`Kernel.ns`, and `PlatformFactory.ns` from `interpreter/core_library.rs`'s table -- are
resolved with it; a user module goes through `resolve`; the bootstrap checks that
`Kernel`'s one immutable `vmMirror` slot holds the mirror (`interpreter::kernel`), and
`PlatformFactory`'s too (`interpreter::core_library`), and no program reaches a shipped
module's root instance or root class, so none can make a second one over something else --
the platform handed to every program is an instance of `PlatformFactory`'s nested
`Platform`, whose factory is `new` and whose classes reach the mirror lexically, and
nothing a program can name answers either root; behind that, `PlatformFactory`'s nested
classes are private and its one public member is `newPlatform`, so even a holder of the
root could only make another platform (the factory pattern,
`docs/design/core-library.md`); the platform is never written to an image field by
field: the encoder names it, and every shipped instance or class it reaches, by its
platform path (`image/platform_paths.rs`), and refuses a shipped module instance or class
no path names; no program can name the mirror's class -- `Object` declares no
accessor for it or for `Top` (`interpreter/kernel.rs`'s test pins the accessor set, and the
recognizer in `compiler::primitive_method` refuses both), and the image decoder refuses builtin
class tag 14 wherever the graph names a class (`read_referenced_builtin_class`, which refuses
`Top`'s tag 15 the same way and for the same kind of reason -- §3.2 lets only `Object` name
it, so no source module can and no artifact may either); the encoder
refuses an image whose graph reaches the mirror, its class, `Top` or the metaclass of either
class; `decide_factory` refuses to instantiate the
mirror's class; and `SendMirror`, which runs a leaf with no lookup, is emitted only for a
`MirrorSend` and has no wire tag, so no artifact can carry one. Never add a way for a
program to obtain the mirror, its class or a `MirrorSend` binding.

**A cache records what the lookup decided; it never decides.** A send's lookup is
`resolve_*` (`interpreter/resolve.rs`), and `act` is the one place that runs what it
found; an inline-cache hit is `act` on the resolution `resolve` answered for the same
key, rebuilt from the entry, so it can do nothing a miss would not. An entry holds no
handle — a guard id, `Rc`s and numbers; the defining class is recomputed per hit — so
it survives a move and cannot reach another heap. Anything the key does not cover is
read afresh in `act` on every send, never trusted from the entry; today that is
`ClassData.builtin`, which `instantiate::decide_factory` reads off the receiver to
tell `Array` and `OrderedCollection` from an ordinary instance, a primitive's
payload check, which `builtins::execute` re-runs. A class object's own `class`
field is no such read: it is its metaclass, whose instances id is the guard, so
the key covers it. A kernel method's fast path is decided the same way:
the entry records a `Method`, and `act` reads the fast path off that method
(`MixinMethod.fast_path` -- a mirror row, `MethodFastPath::Mirror`, a builtin class
for one of `Object`'s accessors, `MethodFastPath::BuiltinClass`, or a quick method's
constant, receiver or argument, `MethodFastPath::Constant`, `Receiver`, `Parameter`,
`ValueOfParameter` and `CullParameter`, whose argument `act` checks for a closure on
every send) on every send, hit or
miss, never from anything the entry decided (`docs/design/dispatch.md`, "Primitive
methods"). A lazy slot's getter has no fast path: every variant's arm is inlined
into each send site of `vm::run` and taxes every method send, so the getter answers
a filled slot from its own activation (`Instruction::ReturnSlotIfFilled`). Add such a read and its per-send
re-check goes with it. The reference lowering runs every send uncached, so it
is the oracle a cache defect shows against (`docs/design/dispatch.md`, "3b — The
caches").

## Working in this repository

**The gates are staggered.** Everything below still gates the work; what
changed is how often. Running all of it before every commit means a batch of
three small changes runs the suite twelve times, which is why the per-commit
gate is the first group and the rest runs once, at the end of a stretch of
work, before it is handed on. Nothing here is optional: a commit that never
saw the second group is not finished work, it is work in progress.

The suites run under `cargo-nextest` (`cargo install cargo-nextest --locked`,
once per machine; a tool, not a dependency of any crate). It runs every test
of every binary in parallel, one process each, where `cargo test` runs the
binaries one after another; the goldens are one test each
(`crates/ns-cli/tests/run_goldens.rs`, `golden_round_trips.rs`, on
`libtest-mimic`), so they spread across the cores too. A stress mode measured
219 s under nextest against 947 s under `cargo test`, and 141 s once debug
builds compile at `opt-level = 1` (`Cargo.toml`, `[profile.dev]`), which keeps
`debug_assertions` and so every stress mode. Nextest runs no
doctests, and three `compile_fail` doctests pin the brand on
`Interpreter::enter`, so `cargo test --doc` runs beside it.

```bash
# --- before every commit ---
cargo fmt --all
# must be warning-free
cargo clippy --workspace --all-targets
# the full suite, including every golden under both lowerings; the editor's
# language server has its own group below
cargo nextest run --workspace --exclude ns-lsp
cargo test --workspace --exclude ns-lsp --doc
# non-regression gate: must report "parsed 211, failed 0, expected-fail 0, stale 0"
cargo run -p ns-cli -- parse-corpus corpus/modules

# --- before every commit that touches crates/ns-lsp or crates/ns-syntax ---
cargo nextest run -p ns-lsp

# --- before every commit that touches editors/jetbrains, crates/ns-lsp or crates/ns-syntax ---
./editors/jetbrains/gradlew -p editors/jetbrains check

# --- once at the end of a batch of work, before handing it on ---
# the same suite under the three stress modes (debug builds only; Git Bash
# syntax): MOVES moves every object at the run loop's safepoints, CACHES
# vacates every inline cache at every safepoint, COLLECT collects at every
# safepoint. All three at once: each run ends in a long tail of a few
# depth-limit tests that leaves most cores idle, and the tails overlap
# (225 s against about 6 minutes one after another). Each writes its own log
# under the git-ignored .temp/, and every mode that fails is named at the end.
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
cargo nextest run -p ns-lsp
# non-regression gate: must report "ran 6, failed 0, skipped 205"
cargo run -p ns-cli -- run-corpus corpus/modules
# the same report under each stress mode
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules

# --- once at the end of a batch of work (downloads several IDEs) ---
./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin

# build every crate
cargo build --workspace
```

`cargo clippy --workspace --all-targets` still compiles `ns-lsp` on every commit,
so a change to the resolver or to `ns-syntax` that breaks the API the server uses
fails at the commit even when its tests only run at the end of the batch. A change
to `crates/ns-syntax/src/lexer.rs` must be mirrored in
`editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt`
in the same commit, or the comparison fails.

Never judge performance from a debug build — `cargo build --workspace --release` is the
one to measure; a release build is also the only one that runs the size tests
`a_signal_fits_in_the_niche_of_its_one_unboxed_error` and the `Object` size test
(`cargo nextest run --workspace --release`). Every binary a measurement compares — the current tree included — is
built with the same explicit flags, which pin every function start to a 64-byte
boundary: in PowerShell,
`$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`
before the build and `Remove-Item Env:RUSTFLAGS` after it (`RUSTFLAGS` replaces the
configuration's flags, so `+crt-static` must be repeated; a Linux or macOS build carries
the alignment flag alone). Step 2c measured identical machine code 1–2% apart through a
64-byte phase shift of every hot function; pinned, that phase shift is gone. The pin is
not the whole story, though: `bench_block.ns` has since measured **16% apart between two
builds of the same tree** with the pin in place, each build stable to about 1%, so for
that source one build per commit measures the build rather than the commit — build
several of each tree and pool them, or the comparison is noise. The shipped build does
not carry the pin: it costs about 110 KB (5.6%) and
measured no faster. `docs/design/execution.md`'s measurement protocol ("What removing the
re-entries moved") spells it out. `vm::run`'s per-instruction step sits near LLVM's
inlining threshold and is forced inline; before timing a change to a send's path, check
with `dumpbin /disasm` (from the Visual Studio Build Tools, not on `PATH`) on a release
build with debug information that no `vm::run::closure` became a function of its own
(`docs/design/execution.md`, "What the inline caches moved").

A file that legitimately cannot parse yet belongs in `corpus/blacklist.toml`. A corpus
module that runs end to end belongs in `corpus/runnable.toml`, with the entry selector
to send and the output it produces; `run-corpus` runs every listed module from source
and as a built binary, under both lowerings, and fails if any of the four differs from
the recorded output. Nothing under `crates/ns-syntax` should change for runtime work.

`PlatformFactory.ns` is widened only on demand: an accessor exists because a module on
`corpus/runnable.toml` needs it, and the whitelist entry that demanded it is named in the
accessor's comment. A capability nothing on that list asks for has no test that would
catch it rotting.

Add no new external dependency without a reason that survives review — the image format
is hand-rolled on purpose.

**Search the whole specification before concluding it is silent.**
`docs/references/newspeak-spec.md` is the specification transcribed from
`newspeak-spec.pdf` for exactly this: a rule is often stated in a section whose
heading looks unrelated — §3.2 decides how the primary factory is found, from a page
about metaclasses — and reading only the topical section is how a rule gets missed. The
PDF stays the authority and is what a citation names; the Markdown is how to find the
passage (`docs/references/PROVENANCE.md`).

**Ground a design decision in prior art before proposing options.** `prior-art/` holds
seven reference implementations as submodules. Four answer "what does Newspeak do here":
the canonical Newspeak on Squeak, its cross-language benchmark suite, Ryan Macnak's
Primordial Soup (a Newspeak VM in C++, with its own bytecode set, image format and
collector) and Stefan Marr's SOMns (a Newspeak on Truffle/Graal). Two are there because
the design documents name them as models for this runtime's own representation: `nova/`
for the arena of typed handles, which ns-next now has (its `GcScope`/`NoGcScope` root
discipline was weighed and rejected for a gc-arena-style lifetime brand, `memory.md`);
`opensmalltalk-vm/` for Cog's activations on a stack, reified on demand, which ns-next
now has, and its divorce of whole stack pages, which suspension and the debugger still
need (`execution.md`). One is there for the optimizing end still ahead: `strongtalk/`,
the type-feedback Smalltalk VM that became HotSpot, for inline caches in the bytecode,
polymorphic inline caches, recompilation along recorded receiver types and
deoptimization back into interpreted activations -- and, through Gilad Bracha's
optional types and first-class mixins, an ancestor of Newspeak itself.

When a question is "how should this work", read how a working system answered it and
cite that, rather than reasoning from first principles alone. Nothing under
`prior-art/` is compiled, tested or linked, and no gate depends on it; a plain clone
does not even fetch it (`git submodule update --init <name>` — by name, since
`opensmalltalk-vm` alone is ~255 MB). Copying code out of any of them is a licensing
decision, not a technical one, and `nova` is MPL-2.0, the most restrictive of the seven.
See `prior-art/PROVENANCE.md`, which also explains why `newspeaklanguage/newspeak`
appears both there and under `corpus/`, and why neither copy may be deleted.

## Conventions

- All code, identifiers, comments, doc comments, commit messages and developer
  documentation are in English.
- Classic terminology: `blacklist`, `whitelist`, `sane` — not their newer replacements.
- **Names are written out.** No identifier of one or two characters, and no
  truncated word -- the whole word, in every kind of name the workspace
  declares (locals, parameters, fields, functions, types, variants, constants)
  and in every segment of a compound one: a `Mutator` is `mutator`, an index
  `index`, an error `error`; `interpreter`, not `interp`; `program_counter`,
  not `pc`; `arguments`, `parameters`, `temporaries`, `expression`,
  `statement`, `declaration`, `identifier`, `instruction`, `initializer`,
  `position`, `directory`; `argument_count`, not `num_args`;
  `ObjectReference`, not `ObjRef`. The exceptions: type parameters and
  lifetimes keep Rust's conventions (`T`, `'a`, `'gc`); `ok`, `id`, `min`,
  `max`, `temp`, `And` and `Or` are words; Rust keywords and names the
  workspace does not declare stay as Rust, std and other crates spell them
  (`str`, `len()`, `fmt`, `cfg`, `Rc`, `map_err`, `Err`, `args()` on a
  `Command`); and names bound to a file
  or an interface keep that name (the module file `vm`, the `disasm` command
  and its module, grammar groups `g1`/`g2`). Clippy's
  `min_ident_chars` (`clippy.toml`, threshold 2 -- it is inclusive) refuses
  the one- and two-character names; code samples in plans follow the same
  rule, since implementers copy them.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`).
- Design documents live in `docs/design/` and are living: update the document for
  the aspect that changed rather than adding a dated file that supersedes it.
  Implementation plans live in `docs/superpowers/plans/`, are named
  `YYYY-MM-DD-HHMM-<name>.md` so several from one day sort chronologically, and
  are execution records — left as written once complete.
- A Newspeak module written inline in a test is an `indoc! {"…"}` (or
  `formatdoc! {"…"}` for a template), its lines indented with spaces along with
  the Rust around it and the closing `"}` on a line of its own -- not a
  one-line literal of `\n` and `\t` escapes. `indoc` is a dev-dependency only.
- Comments should explain the decision, not restate the mechanics. The reason a check
  exists — what it prevents — is the part a later reader cannot reconstruct.
- **Object, frame and activation access goes through
  `crates/ns-runtime/src/heap.rs`, not directly through a handle.** `ObjectReference`,
  `ClassReference`, `FrameReference` and `ActivationReference` are defined in `heap.rs` and their
  inner index is private to that module, so the compiler refuses every read of
  one from anywhere else — there is no grep gate to remember and no way to
  widen a field to reach past an accessor. If an accessor you need is missing,
  add it there. The accessors copy out rather than lend out on purpose; see
  `docs/design/memory.md`, "The seam".
