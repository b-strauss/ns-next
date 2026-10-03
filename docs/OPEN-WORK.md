# ns-next — Open work

Everything that is still to be done, in one list. Each item says what is
open, why it matters, and where it is explained; the mechanism itself is
described in the design document named, under its `Planned:` heading where
it has one ([design/README.md](design/README.md), "How to read these
documents"). [PROGRESS.md](PROGRESS.md) records when things were done; this
file records what is not.

Keep it current: remove an item in the same batch that closes it, and add one
whenever a review, a measurement or a design discussion leaves something open.
Code references of the items dated before 2026-09-30 were checked against the tree on 2026-09-28; those of the items dated 2026-09-30 to 2026-10-04 when they were written.

## Next

**Road step 4, generations** ("Step 4, generations" below): eden and two
survivor spaces at the low end of the object space's reservation, below the
old space; a copying scavenger with age-based tenuring; a remembered bit and
set behind the store accessors; large objects allocated old; and a load
that tenures before the decoder allocates ([memory.md](design/memory.md),
"Planned: generations"; [image-format.md](design/image-format.md),
"Planned: the image under generations"), with the mark-compact of the
object space as the old space's collector. Its design is in those sections;
it has no implementation plan yet, so the next session starts by writing
one. Everything before it on the road is built: road step 2's eight-byte
value, the compact instruction encoding, road step 3a, the object
representation, and road step 3b, the object space
([memory.md](design/memory.md), "The object space"), which put every
object -- a header word, a class word and its fields -- in one space in
`ns-memory`'s reservation, named by a 64-bit byte offset. Each has its
entry in [PROGRESS.md](PROGRESS.md); the whole road is below.

**Measuring.** The protocol is [execution.md](design/execution.md)'s "What
removing the re-entries moved"; every compared binary, the current tree
included, is built with the same explicit `RUSTFLAGS` (`CLAUDE.md`,
"Working in this repository"). Four of the eight measurement sources are
goldens under `goldens/tool/bench/`; the other four -- `bench_new.ns`,
`bench_new2.ns`, `bench_newarg.ns` and `bench_do.ns` -- are not in the tree
and must be written out from [execution.md](design/execution.md), "What
spec-conformant lookup moved", never reconstructed from a prose
description: a reconstruction runs and prints the right answer while
measuring something else. The Are We Fast Yet benchmarks are in the tree
and measured as [execution.md](design/execution.md), "The Are We Fast Yet
benchmarks", describes.

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
- **Before step 2's second plan, the Are We Fast Yet benchmarks** (decided
  with the user on 2026-09-29, so that every later step is measured on real
  programs rather than only on the microbenchmarks under `goldens/tool/bench/`).
  The Newspeak versions in `prior-art/somns/core-lib/Benchmarks` come into
  `benchmarks/are-we-fast-yet/`, each a module of the harness's project,
  verified under both lowerings by `goldens/tool/awfy/` and measured as
  [execution.md](design/execution.md), "The Are We Fast Yet benchmarks",
  describes. Each one widens the platform on demand, as `CLAUDE.md` asks,
  so the Newspeak side grows with them. Stage 1 -- Sieve, Permute, Queens,
  Towers, Bounce, Storage and List, with the harness -- is built, and
  follows Primordial Soup's protocol and module shape (decided with the
  user on 2026-09-30, for stage 1). Stage 2 (Mandelbrot, NBody, Richards,
  and Primordial Soup's Ton80 Richards beside them) is built and follows
  Primordial Soup's protocol, as stage 1 does (decided with the user on
  2026-09-30). Stage 3 is built: DeltaBlue, Havlak, CD and Json, and
  Primordial Soup's DeltaBlue as `DeltaBlueTon80`, in Primordial Soup's
  protocol (decided with the user on 2026-10-01).

  Each benchmark file carries a header naming its upstream, and
  `THIRD-PARTY-NOTICES.md` has SOMns's and Primordial Soup's entries;
  `Mandelbrot.ns`, `Havlak.ns` and `CD.ns` keep their upstream files' own
  licence texts in their headers.

  The baseline over all sixteen -- the fourteen Are We Fast Yet benchmarks
  and Primordial Soup's Richards and DeltaBlue -- is recorded in
  [execution.md](design/execution.md), "The Are We Fast Yet benchmarks":
  three pinned release builds of `1792df5`, pooled, as that document's
  measurement protocol asks (decided with the user on 2026-09-30).
- **Step 2, second plan, the eight-byte value -- built**, on the arena:
  one tagged 64-bit word -- a `SmallInteger` of 61 bits, a `SmallFloat64`,
  the constants `nil`, `true` and `false`, and handles -- with every other
  number a `LargeInteger` or a boxed `Float`.
  [object-model.md](design/object-model.md), "Values and objects".
- **After road step 2, the compact instruction encoding -- built**,
  designed against the one-word value from the dispatch findings of "Where
  the 3% goes": one eight-byte slot per instruction, side tables for the
  rest, measured before it was built (designed with the user on
  2026-10-02, built the same day).
  [execution.md](design/execution.md), "The compact instruction
  encoding".
- **Step 3, the object space -- built.** One header word (the application
  id as the cache guard, the identity hash, format, size, GC bits, a `LargeInteger`'s
  sign) and a class word, the fields inline, variably sized objects in one
  VM-owned space with byte-offset handles. Built in two plans (decided with
  the user on 2026-10-03), as road step 2's value was built on the arena
  first. **3a, the representation -- built** (2026-10-03,
  [its plan](superpowers/plans/2026-10-03-1338-ns-next-object-representation.md)):
  the final layout of an object's words, then on the arena; every object
  kind a format, class objects, closures and reified frames among them, and
  the frame arena gone; strings and symbols
  byte- and 32-bit-indexed objects, `Symbol` under `String`; per-heap ids
  for mixins, applications and code, the application cache keyed by them;
  the image's frames object records, sized in a first pass before anything
  is allocated. [object-model.md](design/object-model.md), "The object
  layout"; [image-format.md](design/image-format.md), "Frames are object
  records", "Two passes". **3b, the space -- built** (2026-10-03,
  [its plan](superpowers/plans/2026-10-03-1934-ns-next-object-space.md)):
  the arena replaced by one space in `ns-memory`'s reservation, handles
  64-bit byte offsets, bump allocation writing each object in place, the
  overflow word, the mark bit in the header and sliding compaction through
  a live-word bitmap. [memory.md](design/memory.md), "The object space";
  [execution.md](design/execution.md), "What the object space moved".
- **Step 4, generations**: eden and two survivor spaces below the old space,
  a copying scavenger with age-based tenuring, and a remembered bit and set
  behind the store accessors. [memory.md](design/memory.md), "Planned:
  generations".
- **Step 5, `become:` through forwarders**, after Spur. It brings the
  recorded guard of an activation and the slot-access check with it, since
  `become:` is the first way a running activation's receiver can change
  shape (decided with the user on 2026-10-03).
  [object-model.md](design/object-model.md), "Planned: `become:` through
  forwarders", "Planned with road step 5: what a slot access checks".
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

## Planned: performance and representation

- **A threaded run loop through `become`, measured.** A loop whose every
  instruction is a function ending in `become` (Rust's explicit tail call)
  would keep `program_counter`, the code and the register base in machine
  registers across instructions and give every instruction its own indirect
  jump -- the threaded dispatch Strongtalk's and Cog's interpreters have.
  `become` is experimental in Rust 1.98.0 (checked 2026-10-02), so the
  question is first how much it would gain: an experiment on a throwaway
  copy, on nightly, switching only the dispatch of the eight-byte encoding's
  loop to `become` and measuring it against the `match` loop. Whether the
  project then moves to nightly is a decision of its own, taken on those
  numbers (decided with the user on 2026-10-02 to note it, not to plan it).
  [execution.md](design/execution.md), "The compact instruction
  encoding".
- **A debug build's no-access protection of vacated pages**, for when
  generated code dereferences addresses itself (road step 8, the baseline
  JIT). The heap protects no page today: its accessors lend only the words
  below the space's top, so a stale offset into the tail a compaction
  vacated already finds no word, and one below the top is caught by the
  debug build's epoch check. Generated code that loads through base plus
  handle bypasses the slice, and protecting what a compaction or, under
  generations, a scavenge vacates is what turns its stale load into a fault
  (deferred with the user on 2026-10-03; `ns-memory` already supplies
  `Protection::NoAccess`). [memory.md](design/memory.md), "The object
  space", "The memory layer".
- **`IntegerBinary` with an immediate operand.** `i + 1` loads the `1` into
  a register before the inlined operation reads it, one more dispatch on
  every iteration of a counting loop; a form that carries a small integer
  operand in the slot would drop it: a compiler change and one more
  eight-byte form of the compact instruction encoding (noted with the user
  on 2026-10-02).
  [compiler.md](design/compiler.md), "Arithmetic inlining".
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
- **Inlining `to:do:` and `to:by:do:`.** Primordial Soup's compiler inlines
  both (`:inlineBytecode:`,
  `prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:2106`;
  `tryInlineToDo:` in `NewspeakCompilation.ns`); ours sends them to
  `Number`, whose Newspeak loop costs one activation for the send and a
  `value:` send and a block activation per iteration. An inlined form
  would need a guard on the receiver and the send as its fallback, as the
  inlined conditionals have. Whether to inline
  is decided by measuring the Are We Fast Yet benchmarks, which loop with
  both. It relates to "Between steps 7 and 8, real Newspeak bodies for
  `whileTrue:`, `whileFalse:` and `do:`" above.
  [core-library.md](design/core-library.md), "What exists".
- **A float literal outside the immediate range allocates each time it
  runs.** `LoadLiteral` makes a fresh boxed `Float` through
  `Mutator::new_float` on every execution of a literal such as `1.0e300`
  (`vm.rs`, `Literal::Float`), as it makes a fresh `LargeInteger` of a
  literal beyond 61 bits. Correct, since a value has no identity; hoisting
  the object, or keeping one per literal, would save the allocation.
  Unmeasured; noted 2026-10-02. [object-model.md](design/object-model.md),
  "Values and objects" ("Literals").
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
  several superclasses) to measure; the per-mixin id it wants, `MixinId`,
  exists since road step 3a. [dispatch.md](design/dispatch.md),
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
- **`LargeInteger` arithmetic copies its operands' digits out.** `=`, `hash`,
  the comparisons and the sign tests read a `LargeInteger`'s digits in place
  (`Heap::with_large_integer`), in `builtins/numbers.rs` and the derived
  equality and hash of `builtins/value.rs`, since road step 3a keeps the
  digits inline in the object. Arithmetic, `printString` and the other leaves
  that compute still take each operand through `Heap::large_integer`
  (`execute_large_integer`, `integer_operand`), an allocation and a pass over
  every digit before the operation, which for a short integer can dominate
  it. Lending the operands to `arbitrary_integer/`, which already works on
  `&[u64]`, would remove the copy; the result is still allocated through
  `Mutator::new_integer`. Unmeasured. [core-library.md](design/core-library.md),
  "Large integers"; [memory.md](design/memory.md), "Known rough edges in the
  seam".
- **`Object>>==` reads the heap for every two distinct objects.** The
  identity leaf (`builtins/object_class.rs`, `identical`) gives a boxed
  `Float` bit identity, so `==` of two distinct handles asks the heap
  whether both are boxed floats before it answers `false`. Spur and
  Primordial Soup compare a boxed float by its object identity and read
  nothing; giving bit identity up for that is a design question, which a
  measurement of what the read costs a program should inform. Unmeasured;
  noted 2026-10-02. [object-model.md](design/object-model.md), "Values and
  objects".

## Planned: language and library

- **The numeric tower and `Integer>>/`.** `Integer` answers `//` but not `/`
  (only `Float` has `/`, `Kernel.ns:578`); `1 = 1.0` and `1.0 = 1` both answer
  false; `Number` (`Kernel.ns:464`) carries only `to:do:` and `to:by:do:`.
  Settling it means
  `Number`'s shared behaviour, `Fraction`, cross-type comparison and a
  matching hash (`3 asFloat hash = 3 hash`). Decided only that it must be
  settled; `num-rational` is the reference for `Fraction` and for converting a
  `Float` to a fraction exactly. [core-library.md](design/core-library.md),
  "Gap: an integer has no `/`".
- **`//` with a `Float` operand.** Primordial Soup answers `7 // 2.5` and
  `7.5 // 2` as a floored `Float` (`vm/primitives.cc`, `Number_div`, whose
  `IS_FLOAT_OP` branch answers `floor(left / right)`); ours raises a type
  mismatch for the first and `doesNotUnderstand:` for the second, since
  `Float` has no `//`. `\\` is not a gap: Primordial Soup's `Number_mod` has
  no `Float` branch and its Newspeak fallback signals an `ArgumentError`
  (`newspeak/KernelForPrimordialSoup.ns:1988`), as ours refuses a `Float`.
  The on-demand rule adds `//` when a benchmark or runnable module sends
  it. [core-library.md](design/core-library.md), "Gap: an integer has no
  `/`".
- **`isNaN`.** `Number>>isNaN` answering `false` and `Float>>isNaN` as a
  leaf over `f64::is_nan`; Primordial Soup's `^(self = self) not` no longer
  works, since a NaN equals a NaN here. A NaN stays an ordinary `Float`
  (a class of its own was set aside). Noted with the user on 2026-10-01;
  added when a program sends it. [core-library.md](design/core-library.md),
  "A NaN equals a NaN".
- **`Float>>printString` of a large or small magnitude.** A `Float` prints
  as Rust's `Display` writes it, the shortest digits that read back as the
  same double and never an exponent, so `1.0e303 printString` is 304
  digits. Whether it should print an exponent past some magnitude is to be
  checked against Primordial Soup's float printing. Noted 2026-10-02.
  [core-library.md](design/core-library.md), "Large integers" (`asFloat`).
- **Failure blocks and Primordial Soup's error classes.** Every kernel
  forwarder's failure block is `ifFail: [^super <selector> …]`, reached only
  for a receiver of the wrong shape, while Primordial Soup's methods signal
  `ArgumentError` when the primitive fails (e.g.
  `newspeak/KernelForPrimordialSoup.ns:1599-1602` for `<<`). Our leaves raise
  their errors themselves, so whether the Newspeak side should signal
  Primordial Soup's error classes is open.
  [core-library.md](design/core-library.md), "The shape of a kernel method".
- **A program's exit status.** A program cannot set its exit status: a run
  exits 0 unless an error is uncaught, whatever `main:args:` answers. §8.4
  is silent. Primordial Soup exits through an explicit `exit:` primitive
  (`prior-art/primordialsoup/newspeak/Actors.ns:588`,
  `prior-art/primordialsoup/newspeak/TestRunner.ns:50`) and ignores
  `main:args:`'s answer; SOMns takes the answer as the status
  (`prior-art/somns/src/som/vm/ObjectSystem.java:557`) and also has
  `system exit:` (`prior-art/somns/core-lib/System.ns:26`). The user
  decided on 2026-09-30 that this is an item of its own, outside the Are We
  Fast Yet benchmarks. The recommendation given then
  was an explicit `platform system exit:` rather than the answer; that is
  not yet decided. Also open: whether pending `ensure:` blocks run
  (Primordial Soup's primitive ends the process at once); that a host
  embedding the interpreter in its own process needs an unwind to the entry
  rather than `std::process::exit`; and the range of the status. No design
  document covers it yet; the decision to keep it apart is in the plan
  [ns-next-are-we-fast-yet-stage-1](superpowers/plans/2026-09-30-1353-ns-next-are-we-fast-yet-stage-1.md),
  "What stays out".
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
  or a program under `benchmarks/` asks for it: collections beyond `List`,
  `Map` and `Set` (`IdentityMap`, `IdentitySet`, `WeakArray`,
  `Collection`); exceptions beyond §8.6's core
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
  application cache grows without bound. Road step 3a keys it by id and
  keeps it strong, and reclaims no mixin id, application index or guard id,
  since nothing drops a mixin before live editing; the reuse rules of
  [object-model.md](design/object-model.md), "An application has two ids",
  and a weak application cache come with the first edit (decided with the
  user on 2026-10-03). `Interpreter::invalidate_caches`
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
  attached to AST nodes -- their spans are kept, beside the AST
  (`parse_for_editor`) -- and an `ns-next fmt` command.
- **Navigation** — go to definition, find usages, structure view —
  `definition`, `references` and `documentSymbol` in the server, on the
  fault-tolerant parse that exists ([frontend.md](design/frontend.md),
  "Recovery").
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
  `goldens/language/class/mixin/mixin_same_mixin_applied_twice_in_one_chain.ns`.
- **How a replaced kernel method is compiled.** Its primitive-method mark and
  `SendMirror` rest on `Binding::MirrorSend`, which only shipped source gets;
  recording it for an edit would hand the edit the whole Rust boundary.
  [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **Whether a debugger forces `program_counter` at every instruction.**
  [execution.md](design/execution.md), "Where `program_counter` is stored".
- **The reference lowering's future.** It is the oracle today; the user said
  (2026-09-19) it should eventually be removed, with the goldens carrying the
  tests. [compiler.md](design/compiler.md), "The reference lowering".
- **Image format.** Whether upstream strips source at packaging; how an
  object holding external state (an open file or socket) is written —
  decided when the first such module arrives.
  [image-format.md](design/image-format.md), "Open questions";
  [core-library.md](design/core-library.md), "Open for later".
- **Whether `Object>>==` compares two `LargeInteger`s by state.** The
  identity leaf (`builtins/object_class.rs`, `identical`) decides by
  `ValueKind` first and compares two boxed `Float`s by their bits, so two
  allocations of one float are identical, but two `LargeInteger`s by object
  identity, so two allocations of one integer beyond 61 bits are not. A
  program meets the leaf only by sending `Object`'s `==` directly: a number
  reaches `==` through `Value>>==`, which is `^self = other` and compares
  by state. Whether a `LargeInteger` pair should compare by state in the
  leaf too, as a value (§3.1.1) and as the boxed float does, is open.
  Noted 2026-10-02. [core-library.md](design/core-library.md), "Large
  integers".
- **How much history a tail call keeps**, and **data parallelism over
  immutable data** as a later exception to the actor model.
- **Whether an image carries a lazy slot's value.** §6.3.3 intends that lazy
  slots "need never be persisted or synchronized": they are reset to
  **nil** and recomputed on demand. Primordial Soup's `transient` slot, the
  same getter under its own keyword, is skipped by its serializer
  (`newspeak/Serialization.ns`, line 645), which is what lets its `Map` and
  `Set` rebuild their hash table after a load (`newspeak/Collections.ns`,
  `hashtable_ ::= rehash`). ns-next's image writes a lazy slot's value like
  any other slot's. A `Map` or `Set` whose table was filled before a
  snapshot would then answer wrongly after the load wherever a key's hash
  does not survive it, as a `Symbol`'s does not
  ([core-library.md](design/core-library.md), "What exists"). `Map` and
  `Set` exist ([core-library.md](design/core-library.md), "The collections
  module (plan 2)"), but nothing reaches this yet: no image holds one, and
  the benchmarks build theirs at run time. `Map`'s and `Set`'s `rehash`,
  which their lazy `hashtable_` runs when the slot is nil, therefore runs
  today only after growth; the path that rebuilds a table after a load has
  no test, and gets one when this item is settled. Noted with the user on
  2026-10-01, while planning the Are We Fast Yet benchmarks' stage 3.

## Unexplained measurements and unmeasured decisions

- **About 1 ms per round of `bench_block.ns`'s collector cost is
  unexplained.** The pause accounts for about 1.4 ms of the 2.46 ms the
  collector added per round; on the four other collecting sources the pause
  *exceeds* the measured difference, on `bench_block.ns` it falls 1.05 ms
  short. What sets `bench_block.ns` apart: 269 collections against 26 to 107
  at a similar live count, and, when measured, the only populated frame
  arena. Three cheap experiments, none run: a fixture whose blocks capture nothing (separates
  the reified frames from everything else); varying `COLLECTION_FLOOR`
  (separates a cost that tracks the collection count from one that tracks
  the pause); and `COLLECTION_FLOOR` raised above the whole allocation, so it
  never collects but still pays the trigger's comparison. Not run either:
  whether the max-over-median latency gap persists under the collector. The
  allocator measurement put 16.9% of `bench_block.ns`'s samples in the
  Windows heap ([memory.md](design/memory.md), "What owning the memory
  gave"); the object space removed the per-object allocations and frees,
  and `bench_block.ns` now makes 229 collections over `bench --iterations
  10`, 12.0 ms of pause in all, but the residual was not measured afresh.
- **Mandelbrot 2.5%, `bench_loop.ns` 1.9% and NBody 1.5% slower after the
  object space**, each with separated ranges against the tree road step 3b
  started from, `d3e9f6f`, while seventeen of the twenty-four sources got
  faster. None of the three was profiled after the inlining fix that
  left List, DeltaBlueTon80 and Richards no longer separated from the base;
  the 64-bit handle alone
  measured 0.9% to 9.9% slower on every source. Whether road steps 3a and 3b
  together are level with `d0e8b08`, road step 3a's base, was not measured:
  each was measured against its own base in its own sitting. The first
  candidates to profile: `Heap::set_pointer_word`, still outlined (as on
  master), and the extra zero test `Value::as_object` and `Value::kind`
  pay since a handle is a `NonZeroU64`.
  [execution.md](design/execution.md), "What the object space moved",
  "What the object representation moved".
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
- **The memory the compact instruction encoding saves is counted, not
  measured.** The forms compared put the eight-byte slots at 189 KB plus
  small tables against 1.13 MB for the stored enum over the benchmark
  programs' 23,635 instructions; no working set was measured before and
  after. Noted 2026-10-02. [execution.md](design/execution.md), "What the
  compact instruction encoding moved".

## Defects that change a correct program's output

- **A started `Stopwatch` carried across a snapshot reads a wrong time in
  another process.** Its clock counts from an origin taken once per process
  (`platform.rs`, `CLOCK_ORIGIN`), where Primordial Soup's primitive reads
  the operating system's per-boot monotonic clock, so a start written by
  one process and read against another's origin gives a wrong elapsed time,
  negative whenever the loading process is younger than the start was, at
  exit status 0. Options, none decided: read the operating system's
  monotonic clock as Primordial Soup does (right across processes of one
  boot, still wrong across a reboot or another machine); rebase a started
  stopwatch on load, so it reads on from the elapsed time it had when
  written; or record the writing process's clock reading in the image and
  offset the loading process's origin by it, which rebases every reading
  at once without a rule for `Stopwatch`. Pinned today only within one
  process (`tests/image.rs`, `a_stopwatch_is_a_record_of_its_path_named_class`).
  [core-library.md](design/core-library.md), "The platform".

## Known defects and gaps

None of these changes a correct program's output; each is wrong but safe, or
reachable only from a crafted artifact, except the first three, which a
program can reach and which end the process, and three a program meets as a
gap: `ns-next run`'s panic on a closed pipe, `Object>>printString`'s one
text and `to:by:do:`'s zero step.

- **`<<` and `Array new:` reserve a program-chosen size with
  `try_reserve_exact`, which refuses an impossible size on Windows but on a
  system that overcommits (Linux) can succeed**, and the process is then
  OOM-killed while the zeros are written, where a program-chosen size is
  meant never to abort the process. Neither leaf can tell an affordable size
  from one the system will accept and then kill for.
  `crates/ns-runtime/src/builtins/collections.rs`, `allocate`, and
  `builtins/numbers.rs`, `shift_left`, which reaches
  `ArbitraryInteger::shift_left`
  (`crates/ns-runtime/src/arbitrary_integer/mod.rs`), where the digits are
  reserved;
  [core-library.md](design/core-library.md), "Large integers".
- **Running out of memory aborts the process; there is no Newspeak-level
  out-of-memory error.** A commit the operating system refuses, or the
  object space's reservation used up (64 GiB on a 64-bit target), ends in
  `std::alloc::handle_alloc_error` (`heap.rs`, `out_of_memory`), as a failed
  `Vec` push does -- decided with the user on 2026-10-03, so that allocation
  stays infallible and no hot path carries a `Result`. An error a program
  can handle, signalled before the space is exhausted, is not designed.
  [memory.md](design/memory.md), "The object space".
- **Writing an image of more than about four billion objects aborts
  instead of refusing.** The encoder writes object ids and counts through
  `u32_field` (`crates/ns-runtime/src/image/encode.rs`), which asserts that
  a count fits 32 bits; while a handle was a 32-bit index no heap could pass
  it, but a 64-bit offset no longer caps the object count, so a heap past
  `u32::MAX` objects is now reachable and `build` aborts on it. Refusing it
  with an encode error, or widening the ids on the wire, would close it.
  [image-format.md](design/image-format.md), "The instruction encoding"
  ("A count past its field refuses the image").
- **`load --print-source` of a project prints every module's source with
  nothing between them** (`crates/ns-cli/src/commands/load.rs`), so a reader
  cannot tell where one module ends; a boundary line naming each module's
  origin would fix it. [image-format.md](design/image-format.md), "Reading
  an artifact".
- **`disasm` of an artifact reaches module 0 only**: `--class` is rooted at
  the application's root class, so a project's other modules, the
  manifest's included, have no path (`crates/ns-cli/src/commands/disasm.rs`);
  `inspect --full` disassembles every body. A module selector, by name or
  index, would close it. [image-format.md](design/image-format.md),
  "Reading an artifact".
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
- **An `own_index` bound is unchecked** — wrong but safe output, accepted; it
  stays safe because every access is bounds-checked per object, against the
  object's own word count in the space ([memory.md](design/memory.md), "The
  object space").
  [image-format.md](design/image-format.md), "Verification".
- **The memory bound under a crafted artifact** is proportional but large,
  about 129 × (limit + reserve) stack entries: `MAX_BLOCK_NESTING` is 128
  (`image/decode.rs`), and the encoder does not refuse deeper nesting.
- **A list's reservation in the decoder is capped in elements, not bytes.**
  `reserve_for` (`image/decode.rs`) caps a capacity at the bytes remaining,
  not at the bytes its elements need, so a list whose element is larger in
  memory than on the wire can reserve several times the bytes still unread:
  the image's roots (`decode_values`, 8 bytes of `Value` a byte in a release
  build and 16 in a debug one -- the root-list term of the stated bound,
  [memory.md](design/memory.md), "The amplification bound of a rejected
  load"), and, before the graph section, a body's instructions and an
  instruction's register operands (`decode_registers`), which that bound
  does not count. The graph's records reserve nothing: every record is
  filled into its placeholder in place. The fix belongs in `reserve_for`
  itself, for every caller at once: a cap in bytes rather than elements,
  `count.min(reader.remaining() / size_of::<T>())`, so no reservation
  exceeds the bytes still unread. A decoder table's `Vec` doubling slack is
  not counted either.
- **`Heap::traced_size` counts the canonical symbol table's empty slots**
  (`heap.rs`), which never shrink; worth revisiting once many heaps live in one
  process.
- **Type arguments on a `<:` clause's receiver are dropped**, like every
  Typed-Newspeak annotation; no golden pins the drop.
- **The image encoder does not refuse a module class handed twice** among
  the modules it writes; the artifact it writes then fails to load. No
  command hands it one: `program.rs` hands each module once.
- **A project file with an empty `root`** is accepted by `read_project` and
  fails later, as reading a directory (`crates/ns-cli/src/project.rs`).
- **`ns-next run` panics when its stdout pipe closes early** (`ns-next run
  … | head`): the transcript's `print!` reports "failed printing to
  stdout" (os error 232 on Windows) instead of ending quietly
  (`crates/ns-runtime/src/platform.rs`). Found 2026-10-01 while checking
  the Json port.
- **`Object>>printString` answers `an Object` for every object whose class
  does not override it**, whatever its class: the leaf
  (`crates/ns-runtime/src/builtins/object_class.rs`, `PrintString`) reads
  nothing off the receiver, where Primordial Soup's answers `'instance of
  '` followed by the class's name. A program that prints its own objects
  cannot tell them apart. Noted 2026-10-01.
- **`Number>>to:by:do:` with a step of 0 never ends from a receiver at
  most `stop`**: `Kernel.ns`'s body, Primordial Soup's, counts up for any
  step that is not negative and has no guard against 0, so `1 to: 5 by: 0
  do:` loops forever, while `5 to: 1 by: 0 do:` never runs its block; the
  method's comment says only that a zero step never ends. Noted
  2026-10-01.
  [core-library.md](design/core-library.md), "What exists".
- **A `.ns` root declaring `packageUsing:`** is refused with the message
  meant for a factory that takes the platform, rather than one pointing at a
  project file (`program::instantiate_application`); no test takes that
  branch.
- **Editor recovery: past the depth guard every other method is dropped.**
  After about 75 consecutive unclosed methods the speculative member parses
  reach `MAX_DEPTH`, and from there every other method is lost; nothing is
  invented. [frontend.md](design/frontend.md), "Recovery". Noted 2026-10-04.
- **Editor recovery: the member-start scan of an erroneous statement is
  quadratic** in the statement's length (`member_holding_error` parses a
  member speculatively at each position up to the first error); a clean
  statement is never scanned. Noted 2026-10-04.
- **Editor recovery: `class Module () (` gives two diagnostics for its one
  missing `=`** -- a missing message selector, then the missing `=`. Noted
  2026-10-04.
- **Editor recovery: one token can be described two ways at one span**,
  "found the start of the next member" by a diagnostic under a re-parse's
  bound and "found Bar" (the token's kind) by another. Noted 2026-10-04.

## Missing tests

- Road step 3a's (noted 2026-10-03): the application cache's key in its
  three cases (a root, two superclasses, sharing); the resolver's push of
  the clause-count error, of which only the pure function is tested; a
  direct pin of `Format::Class` and `Format::Closure` on a class object and
  a closure; a heap-level test of an odd 32-bit string longer than one
  unit; a closure's code id and feedback base surviving a move and a
  collection; a closure record whose captured frame names a record that is
  no frame; and the assertion that the largest builtin tag packs into a
  non-negative clause, which holds only while `BuiltinClass::ALL` has fewer
  than 32 entries.
- `Reservation::prefix` growing back after `protect(ReadWrite)` restores a
  no-access range (`crates/ns-memory/src/lib.rs`).
- `pointer_ranges`' test (`heap/object.rs`) has no block shorter than its
  format expects (the clamp) and no `IndexablePointers` case.
- The decoder's refusal of a wrongly sized class placeholder (the
  `DecodeError` at `image/decode.rs`'s `fill_placeholder_class` call): no
  valid first pass reaches it; the heap-level test pins the contract.
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
- **The release-only size tests** (`a_handle_and_an_optional_handle_are_one_word`,
  `a_resolution_stays_the_size_the_hot_path_was_measured_at`,
  `a_signal_fits_in_the_niche_of_its_one_unboxed_error`) run in no gate
  `CLAUDE.md` lists; `cargo nextest run --workspace --release` runs them.
- Editor: the IDE path has no automated test (checked by hand under
  `runIde`), and `dump-tokens` on an unreadable file, a block comment over a
  whole-line selection and TODO markers through the project-wide index are
  untested. [editor-support.md](design/editor-support.md), "Known gaps".
- The packaged artifact (2026-09-30): the refusal of a module set of which
  only some carry source (`modules::encode_modules`); a back-reference into
  an earlier tree, or a child shared between trees, in the mixin forest; a
  load of an empty mixin table, which answers `ModuleCountMismatch` 1 against
  0; `image::package`'s refusal of a shipped class or instance other than
  the platform; the version word in `build_load.rs`'s corruption sweep;
  `load --print-source` of an image `image::snapshot` wrote; `run-corpus`
  refusing a row whose `send` is not unary; and that the CLI prints nothing
  when an image's platform path is refused.
  `a_module_shadowed_by_an_object_member_is_refused`
  (`crates/ns-cli/tests/run.rs`) asserts an empty stdout although its root
  has no initializer that could print, and
  `two_modules_draw_the_same_identity_hashes_under_run_and_load`'s comment
  claims it pins module order, which a review judged its fixture probably
  cannot tell apart from a load that merely draws no new hash.

## Code quality follow-ups

Small, deliberately left, none affecting an outcome.

- A fallback closure (an inlined conditional's, sent to a non-Boolean)
  reloads its scope's indirection vector (`LoadOuterLocal`, then
  `LoadVector`/`StoreVector`) on every access. The vector register is
  written once, by the frame's `MakeVector` at instruction 0, so one load
  per activation would do; but it must dominate every use, so the fallback
  body would need to know before it is lowered which outer vectors it
  reaches (a pre-scan), a register for each in its by-reference `Layout`,
  and a prologue outside every statement range -- not a simple change. The
  cost falls only on a fallback closure's run, as "Fallback blocks" in
  [execution.md](design/execution.md) intends: the common path pays nothing.
- `builtins/value.rs`: the hash of a value class object, from its mixin's
  name alone, collides for same-named nested classes under different
  enclosing values. A collision breaks no contract, and folding in the
  enclosing value is not plainly safe: the hash walk would unfold it, and
  any node there that declines (an `Array` slot, `Kernel`'s `vmMirror`)
  declines the whole hash, which `Behavior>>hash` then answers by identity,
  so two equal class objects would hash apart. Distinguishing them needs a
  walk that tolerates a decline below a class object -- a design decision.
- Every `<:` level gets a metaclass (`Mutator::new_applied_level` calls
  `attach_metaclass`) that no program sends to, part of the eager cost of
  building levels. Removing it is a design point, not a clean-up: the
  metaclass takes part in class-object equality (`class_state_pairs`) and in
  what the image encoder walks.
- Leftovers of road step 3a's code reviews, none affecting an outcome
  (noted 2026-10-03): two doc comments cite `object-model.md`'s retired
  heading "Planned: the object layout", now "The object layout"
  (`heap/tables.rs`'s module doc, `mixin.rs` above `CLAUSE_INDEX_LIMIT`);
  test names that outlived what they name
  (`class_data_is_reachable_through_object_data` in `types.rs`,
  `an_identity_hash_at_the_ceiling_is_refused_rather_than_overflowing_the_counter`
  in `tests/image.rs`); stale comments in `image/decode.rs` (`describe` at
  the refusal's recompute; the eight-words-a-byte check, which no
  well-formed record approaches, to be called a backstop) and
  `interpreter/instantiate.rs` ("only the cold error path clones it");
  `closure_activation` reading the closure twice on the hottest closure
  path; and `pointer_ranges` clamping silently to a block shorter than its
  format expects, where a `debug_assert!` would be louder.
- Every collection and every move rebuilds the inverse symbol map
  (`Heap::symbol_of`) through a temporary `Vec` of the live symbols, drained
  from the map and re-inserted under their forwarded offsets (noted
  2026-10-03). Walking `canonical_symbols` directly would avoid it, but needs
  a way to make a `Symbol` from its table index, which `Symbol` does not
  offer.
- Strings go through `CodePoints` where the packed words could serve
  directly (noted 2026-10-03): `Heap::string_concatenation`, the `,` leaf,
  builds three -- each operand unpacked, then the result -- before the
  result is packed again, and `Heap::str_value`, which every caller that
  wants a String's or a Symbol's text as UTF-8 goes through, unpacks into
  `CodePoints` before converting. `,` is on Json's path, which road step 3a's
  measurement shows 14.0% slower with separated ranges
  ([execution.md](design/execution.md), "What the object
  representation moved"); packing straight from the two `PackedString`s, and UTF-8 straight
  from one, would leave out the copies.

## Goldens and documentation

- `goldens/language/class/class-side/spec_6_3_factory_is_a_class_side_member.ns` has no rationale
  comment citing §6.3.
- `goldens/language/slot/accessor/spec_6_3_3_a_lazy_slot_is_initialized_on_first_use.toml`
  records trailing spaces in `stdout` that an editor could strip silently.
- Golden proposals from the 2026-09-18 audits judged marginal and not built:
  §5.1.8 closure value rules, §5.1.5 doubled quote, §5.1.7 empty and immutable
  tuples, §6.3.4 binary message pattern, §6.3.1 an unmodified method being
  protected, §4.3 inert metadata comments.
- Two "Converted representation" citations break across lines
  (`docs/design/compiler.md`, `docs/design/execution.md`), which a search for
  the heading misses.
- 33 goldens keep a whitespace-only line where the 2026-09-30 migration
  emptied a slot clause (`goldens/language/block/captures_in_initializers.ns:9`
  among them); and
  `goldens/tool/manifest/a_project_reaches_its_modules_through_the_manifest/App.ns`'s
  comment claims more than the golden pins.
