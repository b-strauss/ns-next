# ns-next — Open work

Everything that is still to be done, in one list. Each item says what is
open, why it matters, and where it is explained; the mechanism itself is
described in the design document named, under its `Planned:` heading where
it has one ([design/README.md](design/README.md), "How to read these
documents"). [PROGRESS.md](PROGRESS.md) records when things were done; this
file records what is not.

Keep it current: remove an item in the same batch that closes it, and add one
whenever a review, a measurement or a design discussion leaves something open.
Code references of the items dated before 2026-09-30 were checked against
the tree on 2026-09-28; those of the items dated 2026-09-30 to 2026-10-04
when they were written.

## Next

**The kernel completed to Primordial Soup's core API**, before road step 5
(decided with the user on 2026-10-06): `Kernel.ns` and
`CollectionsFactory.ns` take Primordial Soup's whole public core API --
selectors and behaviour adopted, every body written here -- in six
sub-projects, in this order: aligning, which is built (`Block` is
`Closure`, `sentTo:` is `sendTo:`, `Array>>at:put:` answers the value, the
loops send `value` to a non-block); foundations, which is built too (the
error classes under `Error`, the `isKindOf…` rule, `yourself`,
`identityHash`, the kernel facet's accessors, `Proxy`, a public `Future`);
collections and strings, which is built too (an abstract `Collection` in
the kernel with `Array` under it, the whole protocol of `Array`, `List`,
`Map` and `Set`, `IdentityMap`, `ConcurrentModification`, `String`'s
protocol in code points, every member of the shipped modules documented);
numbers, which is built too (`Float`'s mathematics and texts, `Integer`'s
and `Number`'s protocol, `Fraction`, and the symmetric tower); `ByteArray`
and `StringBuilder`, with typed arrays over a `ByteArray` as an API of our
own beside them (decided with the user on 2026-10-11); weak references.
Each has its own design and plan; the fifth sub-project is next --
`ByteArray` and `StringBuilder`, the UTF-8 conversions, and the typed
arrays over a `ByteArray` -- and has no design or plan yet. The decisions
and the scope are
[core-library.md](design/core-library.md), "Planned: the kernel completed
to Primordial Soup's core API".

**Then road step 5, mirrors and atomic installation** ("Step 5, mirrors and
atomic installation" below; decided with the user on 2026-10-05). Its
building block is `become:` as an eager bulk forward, as Primordial Soup's:
a one-way forward of an array of old objects to an array of new ones that
copies the identity hash, makes forwarders that exist only while it runs,
rewrites every reference by one walk of the roots the collector traces and
of every heap object, and flushes the inline caches -- so no send, `==`,
leaf or accessor ever meets a forwarder. No program sends `become:`; it is
a leaf on the VM mirror that the shipped mirrors module's atomic install
alone uses. On it the step builds a minimal MirrorBuilder: staged edits of
a class's instance slots and methods, an atomic install (§8.2) that reaches
every application of an edited mixin and migrates the instances whose
layout changed, the refusal of every edit to a shipped mixin, a mirror
capability from the platform, and `ClassMirror` and `ObjectMirror` to read
with. With it come the recorded guard of an activation and the slot-access
check, since the install's forward is the first way a running activation's
receiver can change shape ([object-model.md](design/object-model.md),
"Planned: `become:` as an eager bulk forward", "Planned with road step 5:
what a slot access checks";
[liveness-and-reflection.md](design/liveness-and-reflection.md), "Planned:
road step 5, mirrors and atomic installation", which also lists what the
step's plan decides and what it defers). It has no implementation plan
yet, so the next session starts by writing one. Everything before it on
the road is built, road step 4 last: generations
([memory.md](design/memory.md), "Generations";
[image-format.md](design/image-format.md), "The image under
generations"), whose scavenger's forwarded format is the one the bulk
forward writes. Each step has its entry in [PROGRESS.md](PROGRESS.md); the
whole road is below.

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
  number a `LargeInteger`, a boxed `Float` or a `Fraction`.
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
- **Step 4, generations -- built** (2026-10-04,
  [its plan](superpowers/plans/2026-10-04-2030-ns-next-generations.md)):
  eden and two survivor spaces below the old space, a Cheney scavenger with
  age-based tenuring, a remembered bit and set behind the store accessors'
  write barrier, large objects allocated old, a full collection that
  compacts both generations into the old space, an image load that tenures
  once before the graph pass allocates old, and a fifth stress mode that
  verifies the remembered set and poisons the vacated nursery.
  [memory.md](design/memory.md), "Generations";
  [execution.md](design/execution.md), "What generations moved".
- **Step 5, mirrors and atomic installation** (decided with the user on
  2026-10-05), on `become:` as an eager bulk forward after Primordial
  Soup's, reached only through mirrors. A minimal MirrorBuilder after
  Primordial Soup's `Mirrors.ns` and the canonical Newspeak's
  `AtomicInstaller.ns`: edits staged apart from the live system, of a
  class's instance slots and methods; an atomic install that reaches every
  application of an edited mixin and their subclasses (§6.2), migrates the
  instances whose layout changed and forwards them in one bulk forward, and
  flushes every inline cache; every edit to a shipped mixin refused; a
  mirror capability a program obtains from the platform; `ClassMirror` and
  `ObjectMirror` to read with. It brings the recorded guard of an activation
  and the slot-access check with it (decided with the user on 2026-10-03).
  It is the live editing of mixins, which §6.2 requires to reach every
  application: a `Mixin` stays an immutable Rust value outside the object
  space, its identity is its `MixinId`, and an edit installs a fresh mixin
  under the old one's id -- Primordial Soup's forward of a mixin, done on
  the Rust side -- so the places that still name a mixin by its `Rc`'s
  address (`Heap::mixin_id`, `Mixin::nested`, the hidden nested-class slot,
  the encoder's row ids) move to the id (decided with the user on
  2026-10-05). The application cache, kept strong since road step 3a,
  reclaims no mixin id, application index or guard id; the reuse rules of
  [object-model.md](design/object-model.md), "An application has two ids",
  and a weak application cache come with this step (decided with the user
  on 2026-10-03). An edited program, its edited mixins and their code
  included, is written to an image and loads back edited (decided with the
  user on 2026-10-05; [image-format.md](design/image-format.md), "Planned:
  edited mixins in the image"). Left to its plan: how removing a slot
  recompiles a mixin whose declaration is absent, as a decoded one's is,
  and the questions "Planned: edited mixins in the image" leaves open.
  Deferred beyond it: a shape-changing `setClass:`, a superclass change,
  edits of nested classes, `installAtomically:` over several builders,
  editing kernel methods, `ActivationMirror` and edit-and-continue, scoped
  invalidation ("Planned: images, liveness and reflection" below). The
  comments on the forwarder's guard in
  `crates/ns-runtime/src/mixin.rs` and on `evacuate` in
  `crates/ns-runtime/src/heap/scavenge.rs` still give lazy forwarding as
  their reason; the step rewrites them.
  [object-model.md](design/object-model.md), "Planned: `become:` as an
  eager bulk forward", "Planned with road step 5: what a slot access
  checks"; [liveness-and-reflection.md](design/liveness-and-reflection.md),
  "Planned: road step 5, mirrors and atomic installation";
  [memory.md](design/memory.md), "Weak references".
- **Step 6, inline allocation in the interpreter**: a bump in eden for `new`,
  closures, boxed floats and `Array new:`, with headroom past the scavenge
  threshold so that collection stays at safepoints.
  [memory.md](design/memory.md), "Planned: inline allocation".
- **Step 7, an execution model for compiled code**: frames interpreted and
  compiled code share, a calling convention, GC maps for compiled frames, and
  deoptimization metadata, which the debugger needs as much as the optimizer.
- **Between steps 7 and 8, real Newspeak bodies for `whileTrue:`, `whileFalse:` and
  `do:`.** Making them Newspeak loops keeps what their leaves settle --
  `do:` reads each element as it goes and sends `value:` to a non-block,
  `whileTrue:` and `whileFalse:` send `value` to a non-block body -- and
  changes nothing a program can see.
  [core-library.md](design/core-library.md), "Planned: real bodies for
  control flow and `do:`, before the baseline JIT".
- **Step 8, a baseline JIT**, Cog's class, for x64 and arm64.
- **Step 9, an optimizing JIT** with type feedback, inlining and
  deoptimization, Strongtalk's and HotSpot's class.
- **Last, after step 9: whether pointer compression pays, measured** (noted
  with the user on 2026-10-11). A value as a 32-bit word -- an object
  space offset divided by 8, addressing 32 GB, which handles that are
  already space-relative make possible -- would halve every slot and
  `Array` element, as V8's pointer compression and HotSpot's compressed
  oops do. It would cost the immediate `Float` (every float boxed, as
  V8's `HeapNumber`), shrink `SmallInteger` from 61 to 31 bits, add a
  base to every pointer access, bound the heap, and rework the word
  layout, the collector, the write barrier, the caches and the leaves.
  Only the optimizing JIT, which keeps floats unboxed in registers, makes
  the comparison fair, so it is decided then, from the heap composition
  of real programs, not before; a large homogeneous run of numbers is
  served meanwhile by the typed arrays of the kernel's fifth sub-project.

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
  below the old space's top, so a stale offset into the tail a compaction
  vacated already finds no word, one below the top is caught by the
  debug build's epoch check, and a word naming the nursery a scavenge
  vacated reads the debug build's poison. Generated code that loads through
  base plus handle bypasses the slice, and protecting what a compaction or a
  scavenge vacates is what turns its stale load into a fault
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
- **A subquadratic gcd.** `ArbitraryInteger::gcd` is Euclid's algorithm
  with a full division per step, quadratic or worse in the digit count, and
  the image decoder runs it on every `VALUE_FRACTION` to check the normal
  form (`decode::read_fraction`): an image carrying a fraction of millions
  of digits keeps the loader busy for a long time, though its memory stays
  inside the bound `decode_value` derives. Lehmer's gcd, or a binary one
  on words, would bound it; not taken.
  [image-format.md](design/image-format.md), "A `Fraction` is a value on
  the wire too".
- **Burnikel-Ziegler division copies the divisor at every block and level**
  (`arbitrary_integer/division.rs`: `divide_blocks` passes `divisor.to_vec()`
  per block, `divide_two_blocks_by_one` clones both divisor halves and
  `divide_three_halves_by_two` the high one per call), as upstream
  `num-bigint` does. Lending the slices instead removes
  an allocation and a copy per recursive step; unmeasured.
  [core-library.md](design/core-library.md), "Large integers".
- **`LargeInteger` arithmetic copies its operands' digits out.** `=`, `hash`,
  the comparisons and the sign tests read a `LargeInteger`'s digits in place
  (`Heap::with_large_integer`), in `builtins/numbers/` and the derived
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
- **`String`'s search leaf copies the whole receiver on every call.**
  `search` (`crates/ns-runtime/src/builtins/text.rs`), behind
  `indexOf:startingAt:` and `lastIndexOf:startingAt:`, collects the
  receiver's code points and the argument's into two `Vec<u32>` before it
  compares, so `startsWith:` and `endsWith:`, which compare one position,
  still take time and an allocation in the receiver's size. Comparing the
  code points in place, in either width, would remove the copy. Unmeasured;
  found by the collections and strings sub-project's review, 2026-10-10.
  [core-library.md](design/core-library.md), "What exists".

## Planned: language and library

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
- **Library classes beyond Primordial Soup's core API**, each when a module
  on `corpus/runnable.toml` or a program under `benchmarks/` asks for it:
  `IdentitySet`; exceptions beyond §8.6's core and Primordial Soup's
  (`ExceptionSet`, `Notification`, `Warning`, `outer`, `retryUsing:`,
  `signalerContext`, `Object>>error:`); a mirror `slot name` leaf for
  reflection; an embedded compiled kernel image if the kernel grows. What
  Primordial Soup's core API has comes with "Next" above.
  [core-library.md](design/core-library.md).
- **`String`'s `ByteArray` forms.** `asByteArray`, `copyByteArrayFrom:to:`,
  and a `ByteArray` as the argument of `indexOf:`, `indexOf:startingAt:`,
  `lastIndexOf:`, `lastIndexOf:startingAt:`, `startsWith:`, `endsWith:`
  and `String withAll:` wait for `ByteArray`, the kernel completion's fifth
  sub-project ("Next" above). The collections and strings sub-project left
  them out on 2026-10-10. [core-library.md](design/core-library.md), "What
  exists".
- **`ReadOnlyTuple` and value collections.** Neither is built (decided
  with the user on 2026-10-10): tuple literals stay mutable `Array`s, the
  recorded deviation from §5.1.7 ("Deviations from the specification"
  below), and there is no deeply immutable collection a value may hold. A
  `ReadOnlyTuple` is only shallowly immutable and fits under the kernel's
  `Collection` class as it is. A value collection does not: its class must
  inherit `Value` (§3.1.1), and `Collection` is under `Object` beside
  `Value`, so building one turns `Collection` into a mixin applied both to
  `Object` and to `Value`, as SOMns's `ArrayReadMixin` is.
  [core-library.md](design/core-library.md), "Decisions", "Collections and
  strings".

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
- **Reflection beyond road step 5**: a shape-changing `setClass:`, a
  superclass change, edits of nested classes, `installAtomically:` over
  several builders, and `ActivationMirror`, on the activation objects that
  exist, for edit-and-continue and §8.2's access to the current activation.
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "Planned:
  road step 5, mirrors and atomic installation".
- **Scoped invalidation.** `Interpreter::invalidate_caches` exists, and
  nothing but a test and the cache stress mode calls it; road step 5's
  install will flush every inline cache through it. Scoping an invalidation
  to the applications an edit affects is deferred beyond that step.
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "Caches
  and their dependency keys".
- **A `SourceOrigin` for code entered in the debugger.** `SourceOrigin` has
  `File(PathBuf)` and `Embedded(String)` (`types.rs`); code entered live
  wants a variant of its own. [object-model.md](design/object-model.md),
  "Source retention and provenance".
- **Actors and real concurrency**: an M:N scheduler, per-actor heaps as
  snapshot units, promises for `<-:`, and copying value objects across an
  actor boundary. The treatment of in-flight cross-actor promises at snapshot
  time still needs design. [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **A decompiler** for exporting an image back to `.ns`, and the other image
  questions below. [image-format.md](design/image-format.md), "Open questions".
- **A script recorded into an image.** `ns-next build` takes no `--script`:
  it sends no `main:args:`, and `load` takes only the arguments after `--`.
  Recording a project's script, or its `scripts`, so that `load --script`
  could start an image with them would need a section of the image format of
  its own. Noted 2026-10-05.
  [image-format.md](design/image-format.md), "Packaging".

## Planned: editor support and the analyzer

- **Formatting**, through `textDocument/formatting`, which needs comments
  attached to AST nodes -- their spans are kept, beside the AST
  (`parse_for_editor`) -- and an `ns-next-analyzer fmt` command sharing the
  formatter.
- **Navigation** — go to definition, find usages —
  `definition` and `references` in the server, on the
  fault-tolerant parse that exists ([frontend.md](design/frontend.md),
  "Recovery").
- **Project-aware analysis for the value-class marking.** The server
  analyses one file at a time. With the project file -- every module, and
  which root is instantiated with which arguments -- a whole-program
  data-flow analysis could follow the class objects that flow into slots
  and factory parameters (`Value = platform kernel Value.`) and make the
  structure view's marking of nested value classes, an approximation today
  ([editor-support.md](design/editor-support.md), "Structure"), right in
  most real cases; navigation needs the same analysis. It cannot be exact
  in general: a superclass clause is an expression evaluated in the
  enclosing object and a module can be instantiated with different
  arguments, so only running the instantiation knows, and the analyzer
  never runs a program.
- **More platforms**: a CI build for the other five `bin/<os>-<arch>/`
  binaries of the language server; the repository has no CI.
- Publication on the JetBrains Marketplace stays possible and is not done.
- **Further `ns-next-analyzer` commands**, each looking without running: a
  heap dump, a deeper inspection of an image, and inspection of the JIT's
  compiled code once the JIT tiers exist (road steps 8 and 9 above).

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
`Character` (§5.1.4); a tuple literal is a mutable `Array`, not a
`ReadOnlyTuple` (§5.1.7; "`ReadOnlyTuple` and value collections" above);
a modifier needs no whitespace after it (§6.3.3,
§6.3.4); `outer N` without a message answers the enclosing object (§5.9);
value objects are never copied across an actor boundary (§3.1.1); there is
no mirror-based reflection (§8.2), which road step 5 closes but for
changing the class of an individual object and access to the current
activation.

## Open design questions

- **The resolver as a crate of its own.** It lives in `ns-runtime`
  (`resolver/`). A crate of its own would separate the front end from the VM
  and let a tool that needs only bindings — a lighter language server, the
  formatter — leave the runtime unlinked. It is not worth it yet: the
  resolver uses the runtime's symbol interner, `Binding`, `ResolvedModule`,
  `SourceOrigin`, `instantiate::method_name`, `mixin`'s `RootClauseDefect`
  and `CLAUSE_INDEX_LIMIT`, and `nested::superclass_clause_message`, which
  would all move with it; and the VM mirror's first lock (`CLAUDE.md`, "The
  VM mirror is a capability") rests on `resolve_kernel` being `pub(crate)` —
  in a crate of its own it would turn public, and any Rust host could resolve
  a module with `Binding::MirrorSend`. No tool today would gain: the analyzer
  links the runtime for `disasm` and `inspect`, and `ns-tooling` resolves
  through it. Revisit when a tool must run without the runtime or compile
  times show it, deciding first how the mirror's lock survives the split —
  for example the runtime setting `MirrorSend` into the bindings after
  resolving, instead of the resolver recording it.
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
  Road step 5 refuses every edit to a shipped mixin, so the question stays
  open until editing kernel methods is offered.
  [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **Whether a shape-changing `setClass:` is ever offered, and at what
  cost.** §8.2 makes it possible to change the class of an individual
  object; with `become:` an eager bulk forward, changing one object to a
  class of another shape would pay a walk of the whole heap for one object.
  The canonical Newspeak's `setClass:` rewrites the class word in place
  (`changeClassOf:to:`), and Primordial Soup offers none. Road step 5
  leaves it out.
  [object-model.md](design/object-model.md), "Planned: `become:` as an
  eager bulk forward".
- **Whether lazy forwarding is revisited.** `become:` is eager, one walk per
  forward, because its one consumer, the atomic install, walks the heap
  anyway; Spur's lazy forwarders would cost every send, `==`, leaf and
  generated code a check. Revisit it, behind the same bulk-forward seam,
  only if a measurement shows `become:` frequent (decided with the user on
  2026-10-05). [object-model.md](design/object-model.md), "Planned:
  `become:` as an eager bulk forward".
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
  leaf too, as a value (§3.1.1) and as the boxed float does, is open; two
  allocations of one `Fraction` are not identical to the leaf either.
  Noted 2026-10-02. [core-library.md](design/core-library.md), "Large
  integers".
- **Whether `platform kernel Fraction` must name a program that needs
  it.** `PlatformFactory.ns`'s `KernelNamespace>>Fraction` names none: the
  on-demand rule ("The on-demand rule" in
  [core-library.md](design/core-library.md)) asks every `PlatformFactory.ns`
  accessor to name the runnable module or benchmark that demanded it, while
  the kernel facet only hands out classes of `Kernel.ns`, which the
  kernel completion is not bound by. `KernelNamespace>>Collection` set the
  precedent of an accessor without such a program. Whether the facet's
  accessors fall under the rule, and the rule's wording with it, is the
  user's question, raised by the numbers sub-project's review, 2026-10-11.
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
  ([core-library.md](design/core-library.md), "What exists"). `Map`,
  `IdentityMap` and `Set` exist and remove
  ([core-library.md](design/core-library.md), "The collections module
  (plan 2)"), their table still the lazy `hashtable_ ::= rehash`, but
  nothing reaches this yet: no image holds one, and the benchmarks build
  theirs at run time. `rehash`, which the lazy
  `hashtable_` runs when the slot is nil, therefore runs today only when
  the data array is full, where it is the growth that drops removed
  entries and their tombstones; the path that rebuilds a table after a
  load has no test, and gets one when this item is settled. Noted with
  the user on 2026-10-01, while planning the Are We Fast Yet benchmarks'
  stage 3.

## Unexplained measurements and unmeasured decisions

- **The numeric tower's slow paths are unmeasured on their own.** The Are
  We Fast Yet benchmarks run as fast as without the tower, but none of
  them stresses what still goes the long way: `Float>>hash` of a
  non-integral double builds a `Rational` and computes a modular inverse
  by Fermat's theorem, some 120 multiplications in `u128`, where it used
  to hash the bits (an integer and an integral double skip the inverse); a
  value's derived hash (`builtins/value.rs`, `hash_node`) asks the tower
  of every object it hashes whether it is a number; and an integer's leaf
  with a `Float` argument (`3 * 2.5`, `3 < 2.5`) goes through the tower's
  classification, where `Float`'s own leaf takes a `SmallInteger` on its
  fast path. A program keying a `Map` by floats, or hashing large values,
  would show what they cost. Noted by the numbers sub-project, 2026-10-11.
  [core-library.md](design/core-library.md), "The numeric tower".
- **Eden's first touch costs the short allocating programs 1.5% to 6.8%.**
  Measured with road step 4 against its base, Sieve is 6.8% slower
  (separated), Permute 4.5% and RichardsTon80 3.1% (separated in one
  sitting), and the four allocating microbenchmarks 1.5% to 3.7%, under
  `bench --iterations 10`. The measurement's reading: each scavenges at most
  once, so nearly every allocation lands on a page of the 8 MiB eden the
  process has never touched -- about 1,800 extra page faults per process and
  their zeroing, the kernel's samples up by about 900 over sixty Sieve or
  Permute processes -- while the barrier holds 0.1% to 0.3% of the samples
  and `remember` none; at `--iterations 200` every one of them is faster
  than before. A smaller eden (2 or 4 MiB) would touch fewer pages and
  scavenge Havlak, CD and DeltaBlue roughly two to four times as often;
  pre-faulting eden when the heap is made would move the cost into startup
  and raise every program's peak working set by 8 MiB (`bench_loop.ns` from
  6.5 to about 14.5 MB). Neither is measured: eden variants (2, 4 and
  8 MiB, `EDEN_BYTES` in `heap/space.rs`, with `SURVIVOR_BYTES` at an
  eighth) over all twenty-four sources would settle it. Noted 2026-10-04.
  [execution.md](design/execution.md), "What generations moved";
  [memory.md](design/memory.md), "Generations".
- **Why Havlak still makes 85 full collections cannot be seen.** Road step 4
  took it from 780 to 85, and its scavenges are the expensive ones, 0.65 ms
  each; whether its survivors are tenured early -- by a full survivor space
  or the adaptive threshold -- or its graph is long-lived and rightly
  tenured, `--collector-statistics` cannot say, since it reports no promoted
  bytes, no survivor occupancy and no tenuring threshold
  (`crates/ns-cli/src/entry.rs`, `print_collector_statistics`;
  `Heap::collector_statistics`). Recording the three per scavenge would
  answer it before the survivor spaces' size or the threshold's rule is
  revisited. Noted 2026-10-04. [execution.md](design/execution.md), "What
  generations moved".
- **Road step 4's constants chosen by its plan await the user's
  confirmation**, each one line to change and none measured on its own:
  eden's 512 KiB reserve (`EDEN_RESERVE`), the scavenge threshold of 7.5 MiB
  it leaves; an allocation that finds eden spent going old, born
  remembered; 264 bytes of slack in every nursery region (`REGION_SLACK`);
  the old space's first 256 KiB committed past the nursery, which is
  committed whole; a scavenge leaving resolved futures to the full
  collection; and the remembered set's verification under
  `NS_NEXT_STRESS_SCAVENGE` only. The measurement found nothing for or
  against the reserve or the 32 KiB large-object limit.
  [memory.md](design/memory.md), "Generations".
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
- **`Array>>do:` with a closure reads each element through
  `Heap::array_element` per step** instead of indexing a copy taken at the
  start (kernel aligning sub-project, 2026-10-06). Its cost on the benchmarks
  is unmeasured; measure it per the `measuring-performance` skill
  ([execution.md](design/execution.md)'s protocol) before the next
  performance step.
- **The collections' remaining cost on Havlak and DeltaBlue.** With `List`,
  `Map` and `Set` written here and their modification count, measured at
  `7a661484` against `master` `eeaa1d90` (release builds, three builds by
  three rounds): Havlak 3,491 ms to 3,711 ms, +6.3%, and DeltaBlue
  670.5 ms to 702.3 ms, +4.7%, both separated; the other fourteen within
  noise, -1.7% to +2.3%. Before `Map`'s probes sent `hash` and `=` inline,
  the tables were filled by `whileTrue:` and the data array became the
  removed mark, Havlak was +23.4%. The remainder is the read of a private
  slot per element, a getter send in this VM, and the modification check
  after every block, which is all of DeltaBlue's loss: removing the
  per-block checks was measured at Havlak +4.0% and DeltaBlue -2.2%, and
  was not done, since the check is the decided semantics. Accepted with
  the user on 2026-10-10. Two ways out were identified and not measured: a
  compiler-side direct load for a read of a private slot, in place of the
  getter send, and an inlined `to:do:` ("Inlining `to:do:` and
  `to:by:do:`" above). [core-library.md](design/core-library.md), "What is
  measured".

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
- **A valid method whose body is one long chain of messages aborts `ns-next
  run`.** A chain, `1 + 1 + …`, nests a send per message through its
  receivers, as deep as the source is long, and no depth guard bounds it,
  since every length is valid. The parser, the AST's drop, the resolver and
  the language server take a chain link by link, but the compiler's walks
  (`crates/ns-runtime/src/compiler/`) recurse into each receiver: a body
  that is one chain of 20,000 `+ 1` overflows the interpreter thread's 8 MiB
  and aborts the process (release; 10,000 compile and run), where the
  program should run. Walking the chain from `Send::receiver_chain`, as the
  language server does, would close it. [frontend.md](design/frontend.md),
  "Recovery", rule 7; [execution.md](design/execution.md), "What the
  trampoline moved". Noted 2026-10-04.
- **The AST's derived `Clone`, `PartialEq` and `Debug` recurse along a
  chain of messages** (`crates/ns-syntax/src/ast.rs`), as the compiler
  does: only `Send`'s `Drop` and `Send::receiver_chain` take a chain link by
  link. The compiler clones a slot's initializer for a simultaneous slot's
  future (`compiler/simultaneous.rs`, `future_block`), so a long chain there
  overflows in the clone too. `ns-analyzer`'s `the_editor_parse_agrees_with_parse`
  compares modules with `==`, and the snapshot tests print them with
  `{:?}`; both are safe only because no fixture holds such a chain.
  Hand-written impls that take the chain link by link would close it.
  Noted 2026-10-04.

## Known defects and gaps

None of these changes a correct program's output; each is wrong but safe, or
reachable only from a crafted artifact, except the first three, which a
program can reach and which end the process, and five a program meets as a
gap: `ns-next run`'s panic on a closed pipe, `Object>>printString`'s one
text, `to:by:do:`'s zero step, `String withAll:`'s nil for an argument
that only answers `isKindOfArray` or `isKindOfString`, and the error of a
`hash` that is no Integer.

- **`<<` and `Array new:` reserve a program-chosen size with
  `try_reserve_exact`, which refuses an impossible size on Windows but on a
  system that overcommits (Linux) can succeed**, and the process is then
  OOM-killed while the zeros are written, where a program-chosen size is
  meant never to abort the process. Neither leaf can tell an affordable size
  from one the system will accept and then kill for.
  `crates/ns-runtime/src/builtins/collections.rs`, `allocate`, and
  `builtins/numbers/mod.rs`, `shift_left`, which reaches
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
  manifest's included, have no path (`crates/ns-analyzer/src/commands/disasm.rs`);
  `inspect --full` disassembles every body. A module selector, by name or
  index, would close it. [image-format.md](design/image-format.md),
  "Reading an artifact".
- **A `^` into an initializer that is still running** ends the run as
  "internal: non-local return escaped the entry send" instead of a Newspeak
  error. Pinned by `tests/trampoline.rs`'s
  `a_return_into_a_running_initializer_escapes_the_run`.
  [execution.md](design/execution.md), "Activations".
- **The identity leaves index `arguments[0]` unchecked**
  (`builtins/object_class.rs`, `==` and `Object`'s `=`), and the decoder does
  not check a send's argument count against its selector, so a crafted
  artifact can panic while executing — not a decoder defect, but a check
  away from none.
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
- **`Message>>sendTo:` with too few arguments for a native-backed method
  answers nil silently.** `(Message new selector: #at:put:; arguments:
  {1}) sendTo: anArray` stores nil at index 1 and answers nil, at exit
  status 0, where the same message sent to a method written in Newspeak
  signals the arity `Error`. Not yet diagnosed: the leaf
  `message:sendTo:ifFail:` (`crates/ns-runtime/src/builtins/message.rs`)
  hands the message's arguments to `Mutator::decide_send` without
  comparing their count with the selector's, which the activation of a
  Newspeak method checks and a primitive method's leaf apparently does
  not. Found 2026-10-10, outside the collections and strings
  sub-project's scope. [core-library.md](design/core-library.md),
  "Exceptions".
- **`String withAll:` answers nil for an argument that answers
  `isKindOfArray` or `isKindOfString` true without being one.** The class
  side (`Kernel.ns`, `String class>>withAll:`) asks the leaf, and when the
  leaf declines it gathers the argument into an Array -- unless the
  argument claims to be an Array or a String, in which case it answers
  nil, the answer meant for a subclass of `String` as receiver. A
  program's class that answers `isKindOfArray` (or `isKindOfString`) true
  and `do:` its code points gets nil at exit status 0 instead of a String.
  Telling the two declines apart -- a subject that is not `String` itself
  from an argument the leaf does not read -- would close it. Found by the
  collections and strings sub-project's review, 2026-10-10.
  [core-library.md](design/core-library.md), "What exists".
- **A `hash` that answers no Integer surfaces as "does not understand
  `#\\`" from a `Map` or a `Set`.** The probes
  (`CollectionsFactory.ns`, `scanFor:in:` and `emptyBucketFor:in:`) send
  the answer of `hash` `\\` with the table's size, so a `hash` answering
  `1.5` ends in a `MessageNotUnderstood` of `\\` from inside the probe,
  which names neither `hash` nor the collection. The program's `hash` is
  the defect, and the probes' comments say what is signalled; an
  `ArgumentError` naming `hash` and the answer would read better, at the
  cost of a check on every probe. Found by the collections and strings
  sub-project's review, 2026-10-10. [core-library.md](design/core-library.md),
  "The collections module (plan 2)".
- **Two corners of the tower's `Float` rounding divisions.** A `Float`'s
  floored `\\` corrects C's `fmod` by adding the divisor, and that sum can
  round to the divisor itself: `-1.0e-20 \\ 1.0` answers `1.0`, outside
  the range `[0, 1)` a remainder by `1.0` should lie in (Python answers
  the same). And `float_compute` (`builtins/numbers/tower.rs`) tests the
  divisor for zero after rounding it to a double, so an exact divisor too
  small for a double -- a `Fraction` below the smallest subnormal -- beside
  a `Float` signals "division by zero" though it is not zero. Both were
  kept by the plan; found by the numbers sub-project's review, 2026-10-11.
  [core-library.md](design/core-library.md), "The numeric tower".
- **A `.ns` root declaring `packageUsing:`** is refused with the message
  meant for a factory that takes the platform, rather than one pointing at a
  project file (`program::instantiate_application`); no test takes that
  branch.
- **Editor recovery: a run of more than 128 consecutive unclosed methods
  keeps only its first.** Each method's statement parses the method after
  it speculatively, and that one the next, so the speculative parses nest
  one per method of the run and reach `MAX_DEPTH`. Measured in a file of
  nothing else (release): a run of up to 128 keeps every method, one
  diagnostic each, in at most 7 ms; from 129 on only the first survives,
  the rest inside its body, in 8 ms at 200 and 60 ms at 1,000. Nothing is
  invented. [frontend.md](design/frontend.md), "Recovery". Noted
  2026-10-04.
- **Editor recovery: the work budget is the whole parse's.** Once input
  has spent it, no later statement of the file is looked at again (rule
  1), so the members after an unclosed body there stay inside it, and a
  recovery skip stops only at a nested class or a lazy slot, so a method
  after junk is skipped with it. What spends it is pathological rather
  than typed: a keyword message of thousands of parts holding an error
  (8,000 parts spend it in 5 ms), or half-typed members nested forty deep.
  Unclosed methods do not: twenty `)` removed from `KernelTests.ns` spend
  17,000 of its 9.8 million. Noted 2026-10-04.
- **Editor recovery: `class Module () (` gives two diagnostics for its one
  missing `=`** -- a missing message selector, then the missing `=`. Noted
  2026-10-04.
- **Editor recovery: one token can be described two ways at one span**,
  "found the start of the next member" by a diagnostic under a re-parse's
  bound and "found Bar" (the token's kind) by another. Noted 2026-10-04.
- **Structure view: a stale tree after a timeout, and duplicate requests.**
  After an edit whose request times out, the plugin's structure view keeps
  its last answer, positions stale until the next one; two views of one file
  (the tool window and the popup) each send their own request; and a
  question on the EDT counts against the sixty fast retries
  (`NewspeakStructureViewModel` in the plugin's
  `structure/NewspeakStructureViewFactory.kt`).
  [editor-support.md](design/editor-support.md), "Structure" ("Known
  behaviours"). Noted 2026-10-04.
- **Editor parse: `the_editor_parse_agrees_with_parse`**
  (`crates/ns-analyzer/tests/corpus.rs`) assumes `parse` accepts no module with
  a warning; it is to filter by severity once a warning exists. Noted 2026-10-04.

## Missing tests

- `ns_syntax::parse_for_editor`'s answer to a source too long for a span --
  an empty module and the one `source_too_long` diagnostic -- has no test,
  since it needs a source past 4 GiB.
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
  `runIde`: folding, with the `(...)` placeholder in the editor's font, and
  the structure view's tree, navigation, breadcrumbs, sticky lines, its icons
  in light and dark themes, the hovers on a row's text and on its icon (the
  same), first open against a running server, the module's play button
  and the context menu's Run against a running server (the daemon
  restarted by the answer), and a run configuration starting `ns-next`),
  and `dump-tokens` on an unreadable file, a block comment over a
  whole-line selection and TODO markers through the project-wide index are untested. [editor-support.md](design/editor-support.md), "Known gaps".
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
- Road step 4's unit tests leave out (noted in its reviews, 2026-10-04):
  `Space::new`'s refusal of a reservation too small for the nursery and the
  old space's first commit (`heap/space.rs`, `MemoryError::InvalidRange`);
  the move's `Shift::apply` across several non-empty regions at once, which
  only the stress runs exercise; that a full collection clears every
  survivor's age, and that it places the past survivor space's survivors
  before eden's; a stale canonical-symbol entry refused by the scavenge's
  check (`Heap::forward_weak_symbols`); and a disagreement of the
  remembered set and the remembered bits.
- The numbers sub-project's (noted in its reviews, 2026-10-11): goldens
  for a `LargeInteger` receiver of `/`, `quo:` and `rem:`, a negative
  divisor of `quo:` and `rem:`, a `Fraction` with a `Float` under `//` and
  `\\`, `(-1/2) rounded`, and the class-side `LargeInteger parse: …` (a send to the subclass, which
  declines and ends in doesNotUnderstand:); unit
  tests for a negative `shift_right` at a 64-bit boundary, for
  `float_text`'s precision text of zero and of 21 digits; and `numeric_hash`'s test of `1/P`, which
  should assert the infinity's hash.

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

- Road step 4's leftover (noted 2026-10-04): `tests/collector.rs`'s
  `every_kind_of_stored_word_survives_moves_and_collections` runs a
  scavenge at every safepoint too, which its name does not say.
- `crates/ns-analyzer/src/document_symbols.rs` (noted 2026-10-04): `effective`
  answers an `Option` that is always `Some`, so `detail`'s `(None, true)` arm
  is unreachable; and `SymbolCategory` has no doc comment of its own.
- The numbers sub-project's leftovers (noted in its reviews, 2026-10-11):
  `numeric/rational.rs` scales by `2f64.powi`, whose exactness Rust does
  not guarantee, where a power of two built from its bits would be;
  `execute_float`'s `FloatPrimitive::Parse` arm is unreachable after the
  early return above it; `IntegerPrimitive` has no doc comment saying it
  is the family of a `SmallInteger` receiver; `builtins/numbers/mod.rs`'s
  helpers are `pub(super)`, visible to all of `builtins`, where private
  would do; `fraction.rs`'s module doc omits the decoder among the makers
  of a fraction, and `declines_for_another_class` reads backwards; the
  `Kernel.ns` comments of `Fraction>>numerator` and `denominator` omit
  `Fraction new`, and a few of `Number`'s run past the 72-column wrap;
  `tower`'s fast-path agreement test repeats `arithmetic_double` and
  `ordering_double` inline rather than calling them, and `float.rs`'s
  `doubles_equal` says in its doc that it compares two Floats only; `tests/image.rs` repeats its `1e+300`
  assertion inside the NaN and Infinity loop; and `float_text`'s
  precondition lines lack a blank `///` before them.

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
