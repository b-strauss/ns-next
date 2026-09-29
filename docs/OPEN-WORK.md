# ns-next — Open work

Everything that is still to be done, in one list. Each item says what is
open, why it matters, and where it is explained; the mechanism itself is
described in the design document named, under its `Planned:` heading where
it has one ([design/README.md](design/README.md), "How to read these
documents"). [PROGRESS.md](PROGRESS.md) records when things were done; this
file records what is not.

Keep it current: remove an item in the same batch that closes it, and add one
whenever a review, a measurement or a design discussion leaves something open.
An item's code references were checked against the tree on 2026-09-28.

## Next

**The eight-byte value**, road step 2's second plan (its first, large
integers, is built). Its plan is to be written from
[object-model.md](design/object-model.md), "Planned: an eight-byte value",
against what [core-library.md](design/core-library.md), "Large integers",
describes as built.

## The road to a high-performance VM

Decided with the user on 2026-09-29, in this order: nine numbered steps,
with the compact instruction encoding after road step 2 and real Newspeak bodies
for the control-flow loops between steps 7 and 8. Steps 1 to 6 give a very good
interpreter on a modern object representation; road step 8 reaches Cog's class,
road step 9 Strongtalk's and HotSpot's, measured against Java with the Are We
Fast Yet benchmarks (Newspeak versions in `prior-art/somns/core-lib/Benchmarks`)
and `prior-art/newspeak-benchmarks`.

- **Step 1, the memory layer -- built** (`crates/ns-memory`;
  [memory.md](design/memory.md), "The memory layer").
- **Step 2, first plan, large integers -- built**: `LargeInteger` for
  §5.1.1's unbounded integers, adapted from `num-bigint` and `num-integer`.
  [core-library.md](design/core-library.md), "Large integers".
- **Step 2, second plan, the eight-byte value**, on today's arena: one tagged
  64-bit word -- SmallInteger of 61 bits, SmallFloat64, the constants `nil`,
  `true` and `false`, and handles -- which moves the `SmallInteger`/
  `LargeInteger` boundary from 64 bits to 61. It removes the
  store-forwarding stall every `Value` in memory costs today.
  [object-model.md](design/object-model.md), "Planned: an eight-byte value".
- **After road step 2, the compact instruction encoding**, designed against the
  new value, from the dispatch findings of "Where the 3% goes".
  [execution.md](design/execution.md), "Planned: the compact instruction
  encoding".
- **Step 3, the object space.** One header word (the application id as the
  cache guard, the identity hash, format, size, GC bits, a `LargeInteger`'s
  sign) and a class word,
  the fields inline, variably sized objects in one VM-owned space with
  byte-offset handles; classes, closures and reified frames become heap
  objects and the frame arena goes; strings in two widths; `Symbol` under
  `String`; the application cache keyed by id. [memory.md](design/memory.md),
  "Planned: the object space"; [object-model.md](design/object-model.md),
  "Planned: the object layout"; [image-format.md](design/image-format.md),
  "Planned: the image under the new representation".
- **Step 4, generations**: eden and two survivor spaces below the old space,
  a copying scavenger with age-based tenuring, and a remembered bit and set
  behind the store accessors. [memory.md](design/memory.md), "Planned:
  generations".
- **Step 5, `become:` through forwarders**, after Spur.
  [object-model.md](design/object-model.md), "Planned: `become:` through
  forwarders".
- **Step 6, inline allocation in the interpreter**: a bump in eden for `new`,
  closures, boxed floats and `Array new:`, with headroom past the scavenge
  threshold so that collection stays at safepoints.
  [memory.md](design/memory.md), "Planned: inline allocation".
- **Step 7, an execution model for compiled code**: frames interpreted and
  compiled code share, a calling convention, GC maps for compiled frames, and
  deoptimization metadata, which the debugger needs as much as the optimizer.
- **Between steps 7 and 8, real Newspeak bodies for `whileTrue:`, `whileFalse:` and
  `do:`.** Making them Newspeak loops re-settles three semantics: `do:`
  iterates over a copy, `do:` with a non-block is a no-op, and `whileTrue:`
  with a non-block body re-runs the condition.
  [core-library.md](design/core-library.md), "Planned: real bodies for
  control flow and `do:`, before the baseline JIT".
- **Step 8, a baseline JIT**, Cog's class, for x64 and arm64.
- **Step 9, an optimizing JIT** with type feedback, inlining and
  deoptimization, Strongtalk's and HotSpot's class.

Steps 7 to 9: [execution.md](design/execution.md), "Planned: JIT tiers".

**To settle when planning**, details the design leaves to each step's plan,
to be decided against the code:

- *Road step 3:* where the recorded guard of an activation lives -- a
  field of the activation stack's entry, and, if a reified frame is to carry
  it for road step 7's suspend and resume, an eleventh frame-object field
  (104 bytes rather than the about 96 the decode amplification figure uses
  in [image-format.md](design/image-format.md)); the bit widths for packing
  a class object's applied clause -- mixin id, clause index and builtin tag
  -- into one `SmallInteger`; the three reserved guard values (`NONE`,
  `VACANT` and the forwarder's) and their numbers; whether the Rust-side
  table of value-class refusals is append-only or reclaimed under live
  editing ([object-model.md](design/object-model.md), "Planned: the object
  layout").


## Planned: performance and representation

- **`IntegerBinary` without its `Jump`.** A successful inlined integer
  operation executes an unconditional `Jump` past its fallback send
  (`compiler/lower.rs:1747`), two dispatches on the hot path. Invert the
  branch sense so it jumps on success and falls into the send; that retires
  instruction tag 33 and takes the next free tag, 40.
  [compiler.md](design/compiler.md), "Planned: inverting the branch sense to
  drop the `Jump`".
- **Tail calls, skipped but kept** (§3.6). A tail send skips the sender in
  the continuation while the skipped activations stay reachable for the
  debugger; an `on:do:` activation is never skipped; how much history to keep
  is undecided. Not designed. [execution.md](design/execution.md), "Planned:
  tail calls, skipped but kept".
- **`do:`'s per-element argument vector.** Each element's block activation
  takes a one-element `StepArguments::Owned(vec![item])`
  (`builtins/collections.rs:267`); a one-value variant changes `Step`'s
  layout, which measured as a 4–8% risk. Moot once `do:` is Newspeak
  (between steps 7 and 8 of the road).
- **`Mutator.builtin_classes` as a fixed-size array** instead of a `Vec`
  (`interpreter/mod.rs:255`). `builtin_class(kind)` carries a bounds check, and
  the fast-path arm puts it and its cold panic block at every site `act` is
  inlined into; an array would return most of `vm::run`'s growth from 6,507 to
  6,626 instructions as headroom under LLVM's inlining threshold. It needs the
  bootstrap restructured: `World::new` must initialise the field before any
  class object exists, and `build_builtin_classes` builds them through the
  `Mutator` itself. A forged placeholder handle, `Option` and `MaybeUninit`
  were rejected: each breaks a handle invariant, puts the check back, or needs
  `unsafe`. [execution.md](design/execution.md), "What the
  class accessors cost".
- **The value check once per activation, not per store.** `InitializeSlot`
  runs `check_value_slot` on every store of an object value (`vm.rs`,
  `interpreter/instantiate.rs`); deciding it once per activation is the known
  recovery for `bench_new2.ns`'s 3.6% (below). [execution.md](design/execution.md),
  "What `Top`, `Value` and `Number` cost".
- **Cascades through the integer fast path.** `lower_cascade` emits every
  message with `emit_ordinary_send_with_receiver`, so a cascade's binary
  integer sends never get `IntegerBinary` (`compiler/lower.rs:1514`).
- **Register reuse within a statement, and liveness across statements**, both
  permitted and not done. [compiler.md](design/compiler.md).
- **A `Factory` entry carrying the bases of its instantiation bodies**, to
  drop the family-map probe per initializer push (`inline_cache.rs`'s
  `base_for`); its cost was never isolated. [dispatch.md](design/dispatch.md),
  "Code families".
- **Guarding a private lexical site on N's mixin** rather than its
  application — kept open until a program has such a site (a mixin under
  several superclasses) to measure; the per-mixin id it wants comes with the
  object space (step 3 of the road). [dispatch.md](design/dispatch.md),
  "Decisions".
- **Divide-and-conquer decimal conversion.** `printString` of a
  `LargeInteger` divides by `10^19` once per nineteen digits
  (`arbitrary_integer/conversion.rs`, `to_decimal`), so an integer of *n*
  digits prints in time quadratic in *n*; `num-bigint`'s divide-and-conquer
  radix conversion, not taken, splits by a power of the radix and recurses.
  Parsing a literal is quadratic the same way (`from_radix_digits`).
  [core-library.md](design/core-library.md), "Large integers".
- **Burnikel-Ziegler division copies the divisor at every block and level**
  (`arbitrary_integer/division.rs`: `divide_blocks` passes `divisor.to_vec()`
  per block, `divide_two_blocks_by_one` clones both divisor halves and
  `divide_three_halves_by_two` the high one per call), as upstream
  `num-bigint` does. Lending the slices instead removes
  an allocation and a copy per recursive step; unmeasured.
  [core-library.md](design/core-library.md), "Large integers".
- **`Heap::large_integer` copies a `LargeInteger`'s digits out on every
  leaf send**, for the receiver and for an argument alike
  (`builtins/numbers.rs`: `execute_large_integer`, `integer_operand`; and
  `builtins/value.rs`'s derived equality and hash), as every heap accessor
  copies out ([memory.md](design/memory.md), "The seam"). For `<`, `=`
  and `hash`, and `printString` of a short integer, the copy (an allocation
  and a pass over every digit) dominates the cost, since the operation
  itself reads the digits once. Road step 3, the object space, removes it
  by keeping the digits inline in the object, where a leaf can read them
  in place.
  [core-library.md](design/core-library.md), "Large integers".

## Planned: language and library

- **The numeric tower and `Integer>>/`.** `Integer` answers `//` but not `/`
  (only `Float` has `/`, `Kernel.ns:544`); `1 = 1.0` and `1.0 = 1` both answer
  false; `Number` (`Kernel.ns:463`) carries no behaviour. Settling it means
  `Number`'s shared behaviour, `Fraction`, cross-type comparison and a
  matching hash (`3 asFloat hash = 3 hash`). Decided only that it must be
  settled; `num-rational` is the reference for `Fraction` and for converting a
  `Float` to a fraction exactly. [core-library.md](design/core-library.md),
  "Gap: an integer has no `/`".
- **`\\`, the remainder of `//`.** `Kernel.ns` declares `//`, rounded toward
  negative infinity, on `Integer` and `LargeInteger`, but not the floor
  modulo Primordial Soup pairs with it ("The remainder of this division is
  given by `\\`", `prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1958`),
  so `7 \\ -2` ends in `doesNotUnderstand:`. It needs a leaf per integer
  class, a remainder whose sign follows the divisor's (`num-integer`'s
  `mod_floor`; `ArbitraryInteger::floor_divide` already computes it), and
  goldens; nothing on `corpus/runnable.toml` asks for it yet.
  [core-library.md](design/core-library.md), "Large integers".
- **Pattern literals lowered to §5.1.9's `Pattern` sends** (`Pattern
  wildcard`, `Pattern literal:`, `Pattern keywords:patterns:`) instead of
  **nil**; needs a pattern AST in place of the kept source text and a library
  `Pattern` class. [frontend.md](design/frontend.md), "What is parsed and
  dropped".
- **Chains (`:|`, §5.4.4) and object literals (§5.1.10)**, both marked
  *Unimplemented* by the specification; object literals need the activation as
  enclosing object and a syntax choice. [frontend.md](design/frontend.md),
  "Possible future extensions".
- **The module namespace on the mirror.** `moduleNamed:` is built; the table
  of shipped modules is still the host's, a table entry has no "shipped" flag,
  and the file-system fallback for user modules does not exist — all three
  come with the first user module. Static module linking through it is decided,
  not built. [core-library.md](design/core-library.md), "Planned: the module
  namespace on the mirror".
- **Library classes on demand**, each when a module on `corpus/runnable.toml`
  asks for it: collections beyond `List` (`Map`, `IdentityMap`,
  `IdentitySet`, `WeakArray`, `Collection`); exceptions beyond §8.6's core
  (`ExceptionSet`, `Notification`, `Warning`, `outer`, `retryUsing:`,
  `signalerContext`, `Object>>error:`, finer `Error` subclasses); a mirror
  `slot name` leaf for reflection; an embedded compiled kernel image if the
  kernel grows. [core-library.md](design/core-library.md).

## Planned: images, liveness and reflection

- **Suspend and resume.** Needs a yield path out of `vm::run`, a re-entry
  that continues a decoded frame, whole-stack reify and divorce, and an
  encoding for every `Continuation` variant (`WhileLoop`, `DoLoop`,
  `NestedClassCreation`, `Instantiation`, `Unwind`, `VmErrorSignalled`,
  `Future`) and for `depth_reserve`. A dormant decoder gap must be closed
  first: `verify_graph` never cross-checks a frame's `home` against its `kind`
  (`image/decode.rs`). [image-format.md](design/image-format.md), "Planned:
  suspend and resume".
- **The debugger**: suspend, inspect, edit-and-continue at statement
  boundaries, restart frame, breakpoints; `defaultAction` is where it plugs in
  (a test-only `resume_unhandled_exceptions` hook stands in today). Blocked on
  the yield path above. [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **Mirrors and `MirrorBuilder`**, on the activation objects (`ActivationMirror`)
  and the reified instance-of relation, both of which exist; a `MirrorBuilder`
  must seal the VM mirror's class. Depends on `become:` (step 5 of the road).
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "Mirrors and
  MirrorBuilder"; [dispatch.md](design/dispatch.md).
- **Live editing of mixins.** A `Mixin` has no interior mutability, so an edit
  re-applies a fresh mixin and existing instances keep the old code, where
  §6.2 requires otherwise: interior mutability or mixin versioning. The
  application cache grows without bound; it can become weak once the object
  space keys it by id (step 3 of the road). `Interpreter::invalidate_caches`
  exists but nothing but a
  test calls it; scoped invalidation comes with the first mutation.
  `SourceOrigin` has only `File(PathBuf)`; code entered in the debugger wants a
  variant. [liveness-and-reflection.md](design/liveness-and-reflection.md);
  [memory.md](design/memory.md), "Weak references".
- **Actors and real concurrency**: an M:N scheduler, per-actor heaps as
  snapshot units, promises for `<-:`, and copying value objects across an
  actor boundary. The treatment of in-flight cross-actor promises at snapshot
  time still needs design. [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **A decompiler** for exporting an image back to `.ns`, and the other image
  questions below. [image-format.md](design/image-format.md), "Open questions".

## Planned: editor support

- **Formatting**, through `textDocument/formatting`, which needs comments
  kept in `ns-syntax`'s AST as tokens or trivia, and an `ns-next fmt` command.
- **Navigation** — go to definition, find usages, structure view — which
  needs a fault-tolerant parser in `ns-syntax`; that would also keep semantic
  tokens alive through syntax errors.
- **More platforms**: a CI build for the other five `bin/<os>-<arch>/`
  binaries of the language server; the repository has no CI.
- Publication on the JetBrains Marketplace stays possible and is not done.

[editor-support.md](design/editor-support.md), "Planned later".

## Deviations from the specification

The full table, with the reasoning, is [design/README.md](design/README.md),
"Deviations from the language specification". Four answer a **correct
program wrongly at exit status 0**:

- A non-integral literal is a binary `f64`, not an exact `FixedPointNumber`
  (§5.1.1).
- An asynchronous send (`<-:`) runs synchronously and answers the value, not a
  promise (§5.6). Set aside by the user until there are actors; the first
  candidate of a planned section-by-section semantic audit of the
  specification, like the syntax one.
- A pattern literal evaluates to **nil** (§5.1.9); see "Pattern literals"
  above.
- A string literal keeps its source's code points instead of their Unicode
  normal form C (§5.1.5), so a decomposed and a precomposed `é` differ.
  Nothing in the workspace normalizes; closing it means normalizing where
  source text becomes a string. It is not on the road yet. The Unicode
  normalization data is to come from the `unicode-normalization` crate
  (noted with the user on 2026-09-29 for when it is taken up), which as a new
  dependency is confirmed then. [frontend.md](design/frontend.md), "A string
  literal is not normalized to NFC".

The others: no character literals and no
`Character` (§5.1.4); a modifier needs no whitespace after it (§6.3.3,
§6.3.4); `outer N` without a message answers the enclosing object (§5.9);
value objects are never copied across an actor boundary (§3.1.1).

## Open design questions

- **The class side's own scope.** Instance-side methods and class-side
  declarations share one member set; canonical Newspeak scopes the class side
  apart (`compileClassSide:`). The specification is silent; left as is until a
  program depends on it. [dispatch.md](design/dispatch.md), "Open: the class
  side's scope".
- **A private slot of a mixin applied twice in one chain.** Which
  application's slot a lower method reads has no pinned answer; §5.9 does not
  say. Settle it from the specification and SOMns, then add a golden beside
  `goldens/mixin/mixin_same_mixin_applied_twice_in_one_chain.ns`.
- **How a replaced kernel method is compiled.** Its primitive-method mark and
  `SendMirror` rest on `Binding::MirrorSend`, which only shipped source gets;
  recording it for an edit would hand the edit the whole Rust boundary.
  [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **Whether a debugger forces `program_counter` at every instruction.**
  [execution.md](design/execution.md), "Where `program_counter` is stored".
- **The reference lowering's future.** It is the oracle today; the user said
  (2026-09-19) it should eventually be removed, with the goldens carrying the
  tests. [compiler.md](design/compiler.md), "The reference lowering".
- **Image format.** Whether an image should record its entry (a self-starting
  `load`); what a multi-module artifact looks like; whether upstream strips
  source at packaging; how an object holding external state (an open file or
  socket) is written — decided when the first such module arrives; and entry
  selector validation, sound only while no cross-module inheritance exists.
  [image-format.md](design/image-format.md), "Open questions";
  [core-library.md](design/core-library.md), "Open for later".
- **How much history a tail call keeps**, and **data parallelism over
  immutable data** as a later exception to the actor model.

## Unexplained measurements and unmeasured decisions

- **About 1 ms per round of `bench_block.ns`'s collector cost is
  unexplained.** The pause accounts for about 1.4 ms of the 2.46 ms the
  collector added per round; on the four other collecting sources the pause
  *exceeds* the measured difference, on `bench_block.ns` it falls 1.05 ms
  short. What sets `bench_block.ns` apart: 269 collections against 26 to 107
  at a similar live count, and the only populated frame arena. Three cheap
  experiments, none run: a fixture whose blocks capture nothing (separates
  the frame arena from everything else); varying `COLLECTION_FLOOR`
  (separates a cost that tracks the collection count from one that tracks
  the pause); and `COLLECTION_FLOOR` raised above the whole allocation, so it
  never collects but still pays the trigger's comparison. Not run either:
  pre-sizing the arenas with `with_capacity`, and whether the max-over-median
  latency gap persists under the collector. The allocator measurement puts
  16.9% of `bench_block.ns`'s samples in the Windows heap and measures it
  15.6% faster under mimalloc ([memory.md](design/memory.md), "Payloads
  belong to the Rust allocator, and what that costs"), but whether that is
  this residual was not isolated. The object space (step 3 of the road)
  will remove both the frame arena and the per-payload frees, so its collector
  will be measured afresh.
- **`bench_block.ns` 4.4% slower since the value conditions.** Not isolated.
  Hypothesis (a), the seeded symbol, is refuted; (b), the extra `Value`-level
  class object, and (c), hardware counters, are to run.
  [execution.md](design/execution.md), "What the value conditions cost".
- **`bench_new2.ns` 3.6% slower since `Top`, `Value` and `Number`.** A real
  regression with a known recovery (see "The value check once per
  activation"). [execution.md](design/execution.md), "What `Top`, `Value` and
  `Number` cost".
- **`bench_loop.ns` and `bench_block.ns` 1.4–1.9% slower after removing the
  re-entries.** Code placement suspected, not verified.
  [execution.md](design/execution.md), "What removing the re-entries moved".
- **`bench_newarg.ns` 2.5% and `bench_new.ns` 2.4% slower without primitive
  methods**, and four instantiating sources +0.5% to +1.4% after mixin
  application; neither isolated. [execution.md](design/execution.md), "What
  the core library's move cost" ("What is left unexplained") and "What mixin
  application cost".
- **`bench_do.ns` shifts without a cause** on code a change does not execute:
  2.8% / 3.3% slower on the flat-metaclass and factory change, although its
  loop neither instantiates, nor sends to a class object, nor reaches a
  megamorphic site — the fourth recorded uniform shift on unexecuted code,
  which the function-alignment pin does not remove. A fifth instance wants one
  focused investigation rather than another footnote.
  [execution.md](design/execution.md), "What the step before 3c moved".
- **The global cache's replacement rule** (least-recently-used groups, on
  Strongtalk's precedent) is unmeasured; `bench_megamorphic.ns` measures the
  probe only. Needs a program with cache pressure.
  [dispatch.md](design/dispatch.md).
- **`vm::run`'s growth** from the `fast_path` arm and from `vm::unwind`
  becoming a loop (6,486 to 6,561 instructions) was never timed on its own.
- **Whether batching two heap accessor calls still pays.**
  [memory.md](design/memory.md), "Known rough edges in the seam".
- **`==` on immediates in a hot loop** pays two dispatches; no benchmark sends
  `=` or `==`. [core-library.md](design/core-library.md).
- `bench_block.ns` measures 16% apart between two builds of the same tree —
  a standing protocol caveat, not a defect: pool several builds per tree.

## Defects that change a correct program's output

- **`Symbol new` and `String new` answer an instance with no text.**
  `decide_factory` (`interpreter/instantiate.rs`) special-cases only the VM
  mirror's class and `Array`, so both reach `decide_instantiation` and make a
  slotless instance of the class: `String new size` does not understand
  `size`, and `Symbol new == #foo` answers `true`, since a value's `==` is its
  derived equality, which finds two slotless instances of one class equal. Instantiating
  `Symbol` should be refused, as `Mutator::new_symbol` is meant to be the only
  way to make one, and `String new` should answer an empty string; the object
  layout (road step 3) settles both, with goldens
  ([object-model.md](design/object-model.md), "Planned: the object layout").

## Known defects and gaps

None of these changes a correct program's output; each is wrong but safe, or
reachable only from a crafted artifact.

- **`ns-cli build` accepts a slot accessor as the entry selector.**
  `root_class_answers` (`crates/ns-cli/src/commands/build.rs:54`) asks
  `mixin.methods.contains_key`, and a slot's getter and setter are method-table
  entries (`MethodEntry::SlotAccessor`), although its doc comment says a setter
  must not be accepted. Refuse accessor entries and add a test to
  `crates/ns-cli/tests/build_load.rs`.
- **A `^` into an initializer that is still running** ends the run as
  "internal: non-local return escaped the entry send" instead of a Newspeak
  error. Pinned by `tests/trampoline.rs`'s
  `a_return_into_a_running_initializer_escapes_the_run`.
  [execution.md](design/execution.md), "Activations".
- **`whileTrue:` and `whileFalse:` index `arguments[0]` unchecked**
  (`builtins/block.rs:43,48`), and the decoder does not check a send's
  argument count against its selector, so a crafted artifact can panic while
  executing — not a decoder defect, but a check away from none.
- **A decoded image's last initializer is not held to answering `self`**, so
  a crafted image's `new` answers whatever it answers. `verify_mixins` could
  refuse it.
- **A ready-made instance decoded from an image is not re-checked** against
  §3.1.1's value conditions (a decoded class is). Also unchecked: a value class
  nothing instantiates, agreement of `=` and `hash`, and global uniqueness of
  identity (two equal `String`s are distinct objects).
  [core-library.md](design/core-library.md), "What is still unchecked".
- **Load does not check that a mixin level sits directly above a class of its
  applying row**; the run-time arity guard turns a crafted mismatch into a
  Newspeak error. [image-format.md](design/image-format.md).
- **A root class's `<:` levels keep no identity hash across a snapshot**
  (their metaclasses do); not observable today, since no program can obtain a
  level. A `HASHED_*` tag naming a level by position closes it.
  [image-format.md](design/image-format.md), "Gap: a root class's `<:` levels
  are tag-named but have no entry in that table".
- **A crafted image can pair a register-less converted frame with a
  by-reference closure, and an `own_index` bound is unchecked** — wrong but
  safe output, accepted; it stays safe because every access is bounds-checked
  per object, today per `Vec` and in the planned object space against the
  object's own word count ([memory.md](design/memory.md), "Planned: the
  object space"). The register-less pairing will disappear with road step 3,
where every reified frame will carry its full register file on the wire.
[image-format.md](design/image-format.md).
- **The memory bound under a crafted artifact** is proportional but large,
  about 129 × (limit + reserve) stack entries: `MAX_BLOCK_NESTING` is 128
  (`image/decode.rs`), and the encoder does not refuse deeper nesting.
- **A list's reservation in the decoder is not counted in the load's
  peak.** `reserve_for` (`image/decode.rs:1062`) caps a capacity at the
  bytes remaining, not at the bytes its elements need, so `decode_values`
  (`image/decode.rs:2401`) reserves up to 16 bytes of `Value` per remaining
  byte for one record that claims a huge count -- about 16x more on top of
  the documented placeholder figure, about 32x in a release build in all
  and up to about 37x (derived, not measured) with decoded `LargeInteger`
  values. The same holds for every other caller whose element is larger
  in memory than on the wire: a closure's copied values
  (`image/decode.rs:2467`) and a frame's arguments and temporaries (through
  `decode_values`, `image/decode.rs:2515`), a body's instructions
  (`image/decode.rs:1742`) and an instruction's register operands
  (`decode_registers`, `image/decode.rs:1771`). The fix belongs in
  `reserve_for` itself, for every caller at once: a cap in bytes rather
  than elements, `count.min(reader.remaining() / size_of::<T>())`, so no
  reservation exceeds the bytes still unread. Capping by an element's
  minimum wire size instead changes nothing for a `Value` list, since the
  smallest value is one byte (`VALUE_NIL`). The arenas' `Vec` doubling
  slack is not counted either.
  [memory.md](design/memory.md), "The interim cost, and the one question
  still open".
- **`Heap::traced_size` counts the canonical symbol table's empty slots**
  (`heap.rs`), which never shrink; worth revisiting once many heaps live in one
  process.
- **Type arguments on a `<:` clause's receiver are dropped**, like every
  Typed-Newspeak annotation; no golden pins the drop.

## Missing tests

- `fresh_range` refusing a range past `u32`, and a decoded closure's base
  write-back in isolation (`interpreter/inline_cache.rs`).
- A `^` aimed at a superclass clause's own activation (needs a crafted image).
- The selector and receiver of the instantiation refusals
  (`tests/value_conditions.rs` checks the message only), and a subclass
  inheriting a root clause's refusal.
- A crafted-input test for `HASHED_METACLASS`, and the encoder's refusal of a
  class record without a metaclass (`image/encode.rs`).
- A `doesNotUnderstand:` overrunning the depth limit's reserve
  (`tests/exceptions.rs` has the error and `pass` cases).
- `tests/collector.rs`'s give-back test proves little and runs in debug
  builds only.
- A fixture where a wrong inlined-depth correction would land on a frame with
  a same-index local.
- A unit test pinning `IntegerBinary`'s two patched jump targets (only the
  `insta` snapshots do); the aliasing test in `compiler/lower.rs` hardcodes
  register 0.
- Generated or fuzzed programs run under both lowerings and compared run
  against run; several `ns-runtime` integration test files
  (`tests/value_conditions.rs`, `tests/image.rs`, others) run under one
  lowering only.
- **The release-only size tests** (`an_object_is_the_size_the_amplification_figure_assumes`,
  `a_signal_fits_in_the_niche_of_its_one_unboxed_error`) run in no gate
  `CLAUDE.md` lists; `cargo nextest run --workspace --release` runs them.
- Editor: the IDE path has no automated test (checked by hand under
  `runIde`), and `dump-tokens` on an unreadable file, a block comment over a
  whole-line selection and TODO markers through the project-wide index are
  untested. [editor-support.md](design/editor-support.md), "Known gaps".

## Code quality follow-ups

Small, deliberately left, none affecting an outcome.

- `bytecode.rs:537`: `outer_reach_of` ends in `_ => 0`, so "every instruction
  that reaches another frame's registers is named" rests on prose; an
  exhaustive match would make the compiler enforce it.
- `interpreter/inline_cache.rs`: `send_super` reads the current class and its
  superclass before it knows whether the send is cached; `send_lexical`
  computes N twice on a miss.
- `interpreter/mod.rs`: `catches_return` clones a `FrameKind`
  (`heap.activation_kind`).
- `builtins/block.rs`: a native `whileTrue:`/`whileFalse:` loop checks its
  body closure on every iteration.
- `vm.rs`: `send` and `deliver` carry `#[allow(clippy::too_many_arguments)]`.
- `image/inspect.rs:460` bounds the tag-named hash count with a minimum record
  of 1 byte where the decoder uses `MIN_TAG_NAMED_HASH_BYTES` (5).
- `image/decode.rs`: `DecodeError::CodeArityMismatch`'s comments justify it by
  `read_local` indexing `arguments`, which it no longer does (it reads the flat
  register window); the check is still needed.
- `compiler/capture.rs`: `Captures::keys` is test-only.
- `heap.rs`: `closure_home` indexes the frame arena without checking the
  captured frame's handle origin, as every neighbouring accessor does;
  `closure_activation` answers a parameter count that is only its body's own
  `argument_count`, which the caller already holds.
- `image/inspect.rs`: `read_closure_record` bounds its pre-allocation with
  `min(1024)` rather than `reserve_for`, as the rest of the walk does.
- A fallback closure (an inlined conditional's, sent to a non-Boolean)
  reloads its scope's indirection vector on every access.
- `mixin.rs`: `MixinApplicationClause` repeats `SuperclassClause`'s fields.
- `interpreter/nested.rs`: `after_clause` looks the application up twice; the
  not-a-class error names a clause by position.
- `image/encode.rs`: `encode_class_tag` scans the root levels twice;
  `image/decode.rs`'s `build_root_class` maps every error to
  `NotAClassReference`.
- `builtins/value.rs`: `derived_equality` still pushes a class pair per node
  that is discarded when already assumed; the hash of a value class object,
  from its mixin's name alone, collides for same-named nested classes under
  different enclosing values; every `<:` level gets a metaclass no program can
  send to, part of the eager cost.
- `core-library/Kernel.ns`: `defaultAction`'s `ifFail: [nil]` is unreachable;
  `searching` and `reachedDefault` stay true after a search.
- `builtins/activation.rs:157`: `activation_leaf` has an `unreachable!` arm;
  a restart sets the target's program counter twice (`interpreter/unwind.rs`,
  then `vm.rs`).
- `tests/image.rs:9463`: `a_level_applying_the_mirrors_or_tops_mixin_is_refused`
  has a stale name.
- `heap.rs`: accessor names grew by accretion (`class_of_object` beside
  `class_application`, `array_at` beside `instance_field`, verb-second names
  such as `set_frame_program_counter`), and the `heap` parameter sits first in
  some signatures and last in others. [memory.md](design/memory.md), "Known
  rough edges in the seam".
- `image::encode_code_table` and `image::mixins::plan` take a `ClassReference`
  and a `&Heap` only to reach a mixin, and `disasm` builds a class object for
  the same reason. [compiler.md](design/compiler.md), "Follow-up: entry points
  that reach a class object only to find its mixin".
- Editor: a name declared on both class sides with different kinds is
  coloured by the first declaration; a malformed LSP notification is logged
  and ignored; the platform-directory mapping exists twice (`build.gradle.kts`'s
  `hostPlatform` and `plugin.xml`). [editor-support.md](design/editor-support.md),
  "Known gaps".

## Goldens and documentation

- `goldens/spec/spec_6_3_factory_is_a_class_side_member.ns` has no rationale
  comment citing §6.3.
- `goldens/spec/spec_6_3_3_a_lazy_slot_is_initialized_on_first_use.toml`
  records trailing spaces in `stdout` that an editor could strip silently.
- Golden proposals from the 2026-09-18 audits judged marginal and not built:
  §5.1.8 closure value rules, §5.1.5 doubled quote, §5.1.7 empty and immutable
  tuples, §6.3.4 binary message pattern, §6.3.1 an unmodified method being
  protected, §4.3 inert metadata comments, §8.1 optional category string.
- Two "Converted representation" citations break across lines
  (`docs/design/compiler.md`, `docs/design/execution.md`), which a search for
  the heading misses.
