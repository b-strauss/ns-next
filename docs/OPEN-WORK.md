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

**Road step 8's third stage: sends and the caches in machine code**
([execution.md](design/execution.md), "Planned: the baseline JIT",
"Inline caches", "Stages"): a machine-readable cell beside every
`CacheEntry`, the send, miss, polymorphic and megamorphic stubs, a
send's template that compares the receiver's guard with the cell's and
calls through it, returns in machine code, and with them the native
stack's room checked at the entry and the exit that walks the return
addresses of nested compiled frames and materializes them. Its first two
stages are built ("The baseline JIT's foundation"): `ns-memory`'s doubly
mapped code zone and the entry into machine code, `ns-assembler` with
the x64 encoder, and in `jit/` the macro assembler of the VM's idioms,
the entry and exit stubs and the baseline compiler's one exhaustive
`match`, whose templates cover moves, immediate literals, jumps with a
poll on every back edge and `SmallInteger` arithmetic, and whose generic
leaf runs the straight-line instructions that allocate. Every program
measured level with `master` or faster ("What the baseline JIT's
foundation moved"), since a body runs compiled only up to its first
instruction without a template, usually its first send, and is entered
only when its code can loop before it: stage 3 is where the JIT starts
to pay. The stages after it, each measured against the tree before it:
closures, non-local return, allocation and the write barrier; on-stack
replacement, the polls and the depth count's measurement; the register
cache; the AArch64 encoder under QEMU. Road step 8 as a whole, Cog's
class, for x64 and arm64, is "The road to a high-performance VM" below.
Road step 7 is complete: 7c wrote the contract the JIT tiers build
to ([execution.md](design/execution.md), "Planned: the deoptimization
contract"). The step between 7 and 8 is built: `Closure>>whileTrue:` and
`whileFalse:` and `Array>>do:` are Newspeak loops, whose sends the JIT
sees through
([core-library.md](design/core-library.md), "Real bodies for control
flow and `do:`"); before the JIT they cost: `bench_do.ns` is 61%
slower, Bounce 17%, CD 3.3%, the geometric mean 4.8%, which the
baseline JIT is to recover by inlining the loops' sends
([execution.md](design/execution.md), "What Newspeak loop bodies
moved").

7a′, 7b and 7a are built. The register windows overlap, in Cog's layout
-- the arguments, then the header, then the other registers, the
header's words left out of the register numbering and 13 words in every
build -- so a send's own callee lies on its caller's slots, which are its
first registers, and its arguments are copied into no window of its own
([execution.md](design/execution.md), "Overlapping register windows").
Built in two stages, each measured against the tree before it: the first
-- a send's arguments contiguous slots or consecutive sources read in
place, the argument buffer gone -- no program slower than `master` with
its builds separated and twelve of twenty-four faster ("What contiguous
arguments moved"); the second level with the first on the geometric
mean, -0.09%; of the two programs slower with their builds separated,
DeltaBlueTon80 is still 0.9% slower once its causes the code could fix
were fixed, and Mandelbrot is level ("What overlapping windows
moved"). Neither a push nor a pop writes a scratch register: the
leftover analysis finds what a frame may read before writing it, the
decoder refuses image code that reads it, and every trace of the stack
nils it ("What the leftover analysis moved";
[memory.md](design/memory.md), "What the collector traces"). A
running activation is suspended into its frame and resumed from it lazily,
the landing point of every later deoptimization, and a sixth stress mode
runs every golden on from suspended frames
([execution.md](design/execution.md), "Suspending and resuming"); every
running activation is a frame of tagged words in one reservation of the
heap's, every word of which is traced, with no stack map, walked frame by
frame (`Heap::trace_stack`), and measured no slower than the stack
of entries before it -- nineteen of twenty-four programs faster, the rest
level, with three builds a side that vary the block layout inside
`vm::run` ([execution.md](design/execution.md), "What frames as words
moved"). Road step 6 and `new` as compiled code are built, so a factory
and the initializers are compiled code the JIT tiers will compile like any
other.

**What 5b1 cost.** Measured in release, three pinned builds a side, against
master (`733c15bd`) and task by task: every program within noise, but
`bench_megamorphic.ns`, +1.3-2.1% after the private and super accessor
check, at the edge of build noise and not diagnosed ("Planned: performance
and representation" below). Everything up to road step 6 on the road
is built, and `new` as compiled code, road steps 7b, 7a and 7a′, 7c's
contract and the loops' Newspeak bodies after it;
each step has its entry in [PROGRESS.md](PROGRESS.md), and the whole road is below.

**Measuring.** The protocol is [execution.md](design/execution.md)'s "What
removing the re-entries moved"; every compared binary, the current tree
included, is built with the same explicit `RUSTFLAGS` (`CLAUDE.md`,
"Working in this repository"), and the three builds a side vary the block
layout inside `vm::run` as that section says. Six of the ten measurement sources are
under `benchmarks/micro/`, `bench_signal.ns` the one an exception's
handler search moves and `bench_while.ns` the one that runs `whileTrue:`
and `whileFalse:` over blocks held in slots, which the compiler does not
inline; the other four -- `bench_new.ns`,
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
Fast Yet benchmarks (Newspeak versions in `prior-art/somns/core-lib/Benchmarks`,
Java counterparts in `benchmarks/are-we-fast-yet/java/`) and
`prior-art/newspeak-benchmarks`.

- **Step 1, the memory layer -- built** (`crates/ns-memory`;
  [memory.md](design/memory.md), "The memory layer").
- **Step 2, first plan, large integers -- built**: `LargeInteger` for
  §5.1.1's unbounded integers, adapted from `num-bigint` and `num-integer`.
  [core-library.md](design/core-library.md), "Large integers".
- **Before step 2's second plan, the Are We Fast Yet benchmarks** (decided
  with the user on 2026-09-29, so that every later step is measured on real
  programs rather than only on the microbenchmarks under `benchmarks/micro/`).
  The Newspeak versions in `prior-art/somns/core-lib/Benchmarks` come into
  `benchmarks/are-we-fast-yet/`, each a module of the harness's project,
  verified under both lowerings against `benchmarks/are-we-fast-yet.toml` and measured as
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
  2026-10-05), in two plans (decided with the user on 2026-10-17), as road
  step 3 was. **5a, mirrors that read -- built** (2026-10-18,
  [its plan](superpowers/plans/2026-10-17-2355-mirrors-that-read.md)): a
  seventh shipped module, `MirrorsFactory.ns`, behind `platform mirrors`,
  whose `ObjectMirror` and `ClassMirror` lead to mixin, class declaration,
  method and slot mirrors in Primordial Soup's protocol with the canonical
  `ClassMirror` split; a mirror sends its reflectee nothing, reads only the
  slot a name names, and hands out no value it reached through a shipped
  level, a class mirror reached by navigation holding the way to its
  class rather than the class; a mixin reified as one kernel-private
  `Mixin` object per mixin per heap, each mixin knowing its enclosing
  mixin and its module's text, and an image record of its own for it; the
  mirror table past 256 rows. [liveness-and-reflection.md](design/liveness-and-reflection.md),
  "Mirrors that read". **5b, MirrorBuilder and atomic installation**, on
  `become:` as an eager bulk forward after Primordial Soup's, reached only
  through mirrors, in two plans (decided with the user on 2026-10-18).
  **5b1, the VM's half of live editing -- built** (2026-10-18,
  [its plan](superpowers/plans/2026-10-18-1236-become-and-recorded-guards.md)):
  every version of a mixin answers the one id it was first registered
  under, the row holding the current version, its enclosing mixin's id
  and its module's text, so a fresh mixin is installed under an old one's
  id (`Mutator::replace_mixin`) -- Primordial Soup's forward of a mixin,
  done on the Rust side -- and the hidden nested-class slot, the shipped
  world's maps and the reflection leaves name a mixin by its id; `become:`
  as one bulk forward inside one VM-mirror leaf, which validates every
  pair, copies the identity hash, rewrites every reference in one walk of
  the roots and the heap, keeps the remembered set and flushes every
  inline cache, its lock refusing anything but an ordinary instance of a
  class that is no builtin and whose mixin is not shipped; and the
  recorded guard of an activation whose code addresses its own slots,
  recorded at the push only for a receiver that has the entered level, and
  of a private or super slot accessor's cache entry, one compare on the
  hot path and a walk of the receiver's chain on a mismatch, so a
  receiver the forward replaced is never written in the old layout.
  [object-model.md](design/object-model.md), "`become:` is an eager bulk
  forward", "What a slot access checks";
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "An
  edit replaces the mixin under its id". **5b2a, MirrorBuilder and atomic
  installation -- built** (2026-10-18,
  [its plan](superpowers/plans/2026-10-18-2138-mirror-builder-and-atomic-install.md)):
  a minimal MirrorBuilder after Primordial Soup's `Mirrors.ns` and the
  canonical Newspeak's `AtomicInstaller.ns`, reached from the mirrors of
  5a: edits of a class's slots, factory and initializer, through its
  header, of its instance-side and class-side methods and of its lazy
  slots, staged as source apart from the live system, each checked as it
  is staged, and compiled at run time from the module's current text; an
  atomic install, one leaf with no safepoint in its commit, that reaches
  every application of an edited mixin and their subclasses (§6.2),
  records a successor for every level whose slots stayed, migrates the
  instances whose layout changed by their fields' declaring level and name
  and forwards them on the Rust side; every edit to a shipped mixin
  refused. **5b2b, edited programs in images -- built** (2026-10-19,
  [its plan](superpowers/plans/2026-10-19-1702-edited-programs-in-images.md)):
  every declaration holds its own text, its nested declarations cut out,
  and a module's text is assembled from them; an edited program written
  to an image through the ids and loaded back edited, the code an older
  version runs in the code table and every frame naming its body; the
  program's source in every image unless `--strip-source`; and a program
  loaded from an image with source editable, so every golden goes through
  `build` then `load` again. **5b2c, reclaiming ids -- built**
  (2026-10-20,
  [its plan](superpowers/plans/2026-10-20-1318-reclaiming-ids.md)): a
  full collection gathers, while marking, the ids the live objects name
  and frees every application, guard id, successor entry, module tree of
  mixin ids, superseded version, code id and cache range nothing names
  any more, scrubbing from the caches exactly the entries that named a
  freed id; every table mints its smallest free row first and gives its
  trailing free rows back; running out of guard ids requests a full
  collection; the table of reified mixins and the application cache are
  weak; a mixin's nested declarations are named by id; and a nested
  class's cache slot holds a serial per creation, so a forward can no
  longer lose a cyclic-definition diagnosis. Deferred beyond them: a
  class declared from source with no predecessor, a shape-changing
  `setClass:`, a superclass change, edits of nested classes,
  `installAtomically:` over several builders, editing kernel methods,
  `ActivationMirror` and edit-and-continue, scoped invalidation
  ("Planned: images, liveness and reflection" below).
  [liveness-and-reflection.md](design/liveness-and-reflection.md),
  "A MirrorBuilder and an atomic install", "Edited programs in images",
  "What the id has to carry"; [object-model.md](design/object-model.md),
  "An application has two ids";
  [image-format.md](design/image-format.md), "Edited programs in the
  image"; [memory.md](design/memory.md), "Weak references".
- **Step 6, inline allocation in the interpreter -- built** (2026-10-22,
  [its plan](superpowers/plans/2026-10-22-0008-inline-allocation.md)): a
  bump cursor in the reservation, eden's top, and one comparison against
  a limit below the scavenge threshold before the cold `allocate_block`;
  the headers of instances, closures, frames and boxed floats
  precomputed; a code-id cache per body, checked against its heap's
  epoch; `Array new:`, `MakeArray` and `MakeVector` with no `Vec`; and
  `new` of a class whose application runs no code as a bump and a nil
  fill, with no walk. [memory.md](design/memory.md), "Inline allocation";
  [object-model.md](design/object-model.md), "Instantiation";
  [execution.md](design/execution.md), "What inline allocation moved".
- **After step 6, `new` as compiled code -- built** (2026-10-22,
  [its plan](superpowers/plans/2026-10-22-2051-compiled-new.md)): the
  primary factory is a body the compiler synthesizes -- allocate, run the
  class's initializer, answer the instance -- on the metaclass's factory
  entry, reached by ordinary dispatch and the inline caches; each mixin
  carries a full, an own-part and a standalone initializer body, with its
  clause messages' arguments lowered inline, and each level's initializer
  invokes its superclass's from inside its own activation; the Rust walk
  and road step 6's `new` without an activation are gone. A level a `<:`
  clause added reached from outside is an ordinary class of its mixin; a
  declaration without a body is the mixin application it writes, its own
  code resolving in its own scope; and a nested-class accessor's cache hit
  is a guard and one field load. [object-model.md](design/object-model.md),
  "Instantiation", "Mixin application", "The nested-class accessor";
  [compiler.md](design/compiler.md), "The three initializer bodies".
- **Step 7, an execution model for compiled code**: frames interpreted and
  compiled code share, a calling convention, GC maps for compiled frames, and
  deoptimization metadata, which the debugger needs as much as the optimizer.
  Decided with the user on 2026-10-24, in four parts and this order.
  **7b is built**: a running activation is suspended into its frame
  object, what the frame cannot hold kept in a side table of the heap, and
  resumed lazily, only the top coming back onto the stack and each sender
  when something returns or unwinds into it (Primordial Soup's
  `CreateBaseFrame`, Cog's `makeBaseFrameFor`); `NS_NEXT_STRESS_SUSPEND`,
  the sixth stress mode, runs every golden on from suspended frames, and
  the decoder refuses every frame not marked uncontinuable
  ([execution.md](design/execution.md), "Suspending and resuming";
  [image-format.md](design/image-format.md), "Planned: suspend and
  resume"). **7a is built**: frames as words with a fixed header, every
  word tagged, in one contiguous reservation of `ns-memory`, the depth
  limit kept and a byte bound added only as a net
  ([execution.md](design/execution.md), "Representation", "The depth
  limit"), kept because the interpreter measures no slower in release --
  no program slower with its builds not overlapping `master`'s, nineteen
  of twenty-four faster ("What frames as words moved"). **7a′ is built**:
  register windows that overlap, the caller's outgoing arguments being
  the callee's first registers, in Cog's layout, built in two measured
  stages ([execution.md](design/execution.md), "Overlapping register
  windows"), the first -- contiguous arguments read in place -- faster
  than `master` on twelve of twenty-four programs ("What contiguous
  arguments moved"), the second level with the first on the geometric
  mean ("What overlapping windows moved"), with neither a push nor a pop
  writing a scratch register ("What the leftover analysis moved").
  **7c is written** (2026-10-27): the deoptimization contract and the
  protocol between interpreted and compiled frames
  ([execution.md](design/execution.md), "Planned: the deoptimization
  contract") -- compiled frames at the interpreter's positions on the
  activation stack, no header word written on the hot path, return
  addresses on the native stack of the thread that runs the
  interpreter, every compiled frame
  materialized as interpreted ones at machine code's exit to the run
  loop, before any observer runs; deoptimization and on-stack
  replacement of baseline frames in place, of optimized frames lazily
  through the suspended frames of 7b; metadata only at interrupt points.
- **Between steps 7 and 8, real Newspeak bodies for `whileTrue:`,
  `whileFalse:` and `do:` -- built** (2026-10-27,
  [its plan](superpowers/plans/2026-10-27-1529-loop-bodies.md)):
  `Closure>>whileTrue:` and `whileFalse:` are the loop the compiler
  inlines, `^[self value] whileTrue: [body value]`, and `Array>>do:` reads
  its size through its leaf and walks it with an inlined `whileTrue:`
  sending `at:` and `value:`; the mirror rows, the native loops and their
  continuations are gone, and nothing a program can see changed but the
  level at which a recursion through a loop over blocks held in variables
  overflows, now that `Closure>>whileTrue:` is a counted activation. The
  reference lowering lowers a loop over two block literals as a naive loop
  of `value` sends to real closures, whose activations are exempt from the
  depth count. [core-library.md](design/core-library.md), "Real bodies for
  control flow and `do:`"; [compiler.md](design/compiler.md), "What it
  emits"; [execution.md](design/execution.md), "What Newspeak loop bodies
  moved".
- **Step 8, a baseline JIT**, Cog's class, for x64 and arm64, to the
  contract of 7c, designed ([execution.md](design/execution.md),
  "Planned: the baseline JIT"), its first two stages built (2026-10-27,
  [their plan](superpowers/plans/2026-10-27-2047-baseline-jit-foundation.md);
  "The baseline JIT's foundation"), stage 3 next ("Next" above). Left to
  it, and measured: whether the depth count lives in a machine register
  while machine code runs, written back at the exit, or stays in memory
  ("Planned: the deoptimization contract", "Safepoints and the depth
  limit"); whether a send calls through its cache cell or through a
  patched direct call; and the code zone's default size, 16 MiB today
  (`jit::DEFAULT_ZONE_BYTES`), a first guess. What the foundation leaves
  to the stages after it (noted in its reviews, 2026-10-27):
  - **Which bodies are entered, measured.** A body is entered when its
    code can reach a return, a compiled send or a backward jump before its
    first exit (`jit/baseline.rs`,
    `reaches_a_return_a_send_or_a_loop_before_an_exit`); code that exits
    after a few straight-line instructions cost up to 31% on the send
    benchmarks ("What the baseline JIT's foundation moved"). Whether the
    rule is the right one for the stage's benchmarks is still to be
    measured.
  - **`jit::entered` is `#[cold]`**, right while almost every push of a
    compiled body only counts; once templates make entering the common
    path of a compiled body's push, its placement is to be measured
    again.
  - **A body shared by two interpreters would ping-pong between their
    zones.** A body's `JitSlot` names one zone, and `jit::note_entry`
    compiles a body whose slot names another into the running
    interpreter's, so two interpreters running one `CompiledCode` in turn
    would compile it again at every switch, each compile leaving its old
    code in a zone that nothing reclaims before the zone is compacted. No
    body is run by two interpreters today; if code comes to be shared,
    the slot holds code per zone or the stale copy is reclaimed.
  - **`load_frame_register` has no unchecked variant.** Every frame
    register a template loads is checked against the dead-register poison
    in a debug build, as the interpreter checks a read; a copy the
    interpreter does not treat as a read -- placing a send's argument
    slots -- needs a load without the check, with stage 3's sends.
  - **The idioms' scratch register is guarded by a `debug_assert!` alone**
    (`r11`, `load_frame_register`): a template that kept a value in it
    across an idiom would be found only by a debug build that reaches it.
  - **A panic inside a leaf aborts the process**, by design, since nothing
    unwinds through machine code; a debug build's dead-register read
    inside a leaf therefore ends the whole test run rather than failing
    one test. A leaf that is to report such a defect returns it as a
    value, as a template's read does (`ExitKind::DeadRegister`).
  - **Paths compiled for no target here**: the code zone's Linux, BSD and
    Apple AArch64 paths are checked by clippy for
    `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` (the `memory`
    gate) and never run; the AArch64 encoder's stage runs the suite under
    QEMU.
  - **Small leftovers**: `ns-memory`'s `map_code` is an `unsafe fn` though
    its safety section asks nothing of its caller; the leaf shim could be
    `unsafe extern "C"`; `crates/ns-assembler/tests/encoding_sources.rs`
    reads a raw string as an ordinary one.
- **Step 9, an optimizing JIT** with type feedback, inlining and
  deoptimization, Strongtalk's and HotSpot's class.
  - **The home of a closure created in an inlined scope** (noted with the
    user on 2026-10-27): a frame object is married only to a frame that
    stands for one activation, so a block the optimizer does not inline,
    created in a scope it did inline, has no frame to take as its home
    without deoptimizing. Strongtalk gives such blocks a compiled context
    that a deoptimization forwards (`prior-art/strongtalk/vm/runtime/vframe.cpp:595-695`).
    [execution.md](design/execution.md), "Planned: the deoptimization
    contract", "Open".
  - **Scalar replacement of value objects** (noted with the user on
    2026-10-22): a value whose allocation the JIT sees can live in
    registers and never be allocated, as escape analysis does for any
    object, and a value need not even keep its identity when it escapes,
    since its `==` is its `=` and its `identityHash` its `hash`
    (`Kernel.ns`, `newspeak-spec.pdf` §3.1.1). `new` as compiled code is
    what makes it possible: allocation and initialization are code the JIT
    can inline. Four questions are open: a value's lazy slot (§6.3.3),
    which each copy would compute again; `self` escaping a value's
    initializer before its slots are filled; whether the bulk forward
    (`become:`) may take a value; and flattening values into other objects
    and arrays, which without static slot types needs speculative layouts.
    [object-model.md](design/object-model.md), "A value object's four
    conditions".
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

- **The depth flag a continuation's push takes is dead** (2026-10-27).
  `Mutator::push_continuation` takes `replaced_counted`, whether the
  activation a continuation replaces counted against the depth limit,
  which `vm::finish` and `apply_continuation` thread through from the
  popped frame (`Popped::counted`), so that a block a continuation pushes
  in its place could stay exempt. Only the native loops ever replaced an
  exempt activation; since `whileTrue:`, `whileFalse:` and `Array>>do:`
  are Newspeak, every exempt activation delivers, and everything
  `push_continuation` pushes is counted. Removing the flag, and
  `Popped::counted` with it if nothing else reads it, changes the return
  path, which `cargo xtask gates codegen` holds to a budget and the
  protocol of "Measuring" above must time before it is kept.
  [compiler.md](design/compiler.md), "How the limit counts";
  [execution.md](design/execution.md), "The depth limit".
- **`vm::run` reloads the `Mutator` pointer from its frame** (2026-10-25).
  Since the first stage of road step 7a′ the pointer arrives in a
  callee-saved register (`r12`) but is also spilled to the frame and
  reloaded at about 163 sites, one of them right before a public send's
  cache guard compare; `master` keeps it in `rdi` throughout `vm::run`,
  with no reload. A sampling profile of `bench_send.ns` puts 25 of 637
  `vm::run` samples on or right after the reloads. Two experiments did not
  change it -- dropping the two-argument send arms' call to
  `place_arguments` (160 reloads) and `PendingSend::wide` out of line (186)
  -- so it is not those calls alone. What would find out why: the
  register allocator's spill remarks for `vm::run` (`-C
  llvm-args=-pass-remarks-missed=regalloc`) at the merge base and at the
  stage, and the stage's changes to `vm::run` reverted one at a time
  until the pointer stays in its register, each build checked by
  disassembly.
  [execution.md](design/execution.md), "What contiguous arguments moved".
- **`builtins::execute`'s result comes back through a store-forwarding
  stall** (2026-10-25). Its `Option<Result<Step, Signal>>` is returned
  through a sixteen-byte stack slot the callee writes as a qword and a
  byte, which the caller copies out with one sixteen-byte load: a load the
  store buffer cannot forward. On `master` already, 24% of Mandelbrot's
  samples sit on that copy. A smaller result, or the value written
  straight to the destination register, would remove it; the first stage
  of road step 7a′ removed the same stall on the way in, for the argument
  window. [execution.md](design/execution.md), "What contiguous arguments
  moved", "No re-entry from Rust".
- **Three values of the send, push and return path cross by value
  through memory** (2026-10-25). Everything else the path passes between
  functions the compiler keeps apart is at most two scalars, which Rust's
  ABI passes in registers; these are larger and go through a stack slot:
  the decided `Step` (56 bytes) that `Mutator::push_sent` and `push_step`
  take, the pushes' `Result<ActivationReference, Signal>` (40), and the
  `Popped` (24) that `Heap::pop_activation` answers on every return. The
  release-only size tests pin them where they are
  (`what_the_hot_path_passes_through_memory_stays_the_size_it_was_measured_at`,
  `what_a_pop_answers_stays_the_size_it_was_measured_at`), and the codegen
  gate counts the wide loads over narrower stores in the functions that
  pass them. Whether lending them, or answering them in two scalars,
  removes a measurable cost is unmeasured. [execution.md](design/execution.md),
  "Keeping the hot path in registers".
- **`bench_megamorphic.ns` since road step 5b1** (2026-10-18): +1.3-2.1%
  against master after the private and super slot accessor check, on all
  three pinned builds a side, where every other program measured within
  noise. It is at the edge of build noise and not diagnosed: the check runs
  only on a private or super slot accessor's send, and the benchmark's
  megamorphic sends are method sends, so a code-layout effect is as likely
  as a cost of the check itself. Measure it again with the next change to the
  send path, and diagnose it if it stays.
  [object-model.md](design/object-model.md), "What a slot access checks".
- **A mirror walks again what it walked a moment ago** (2026-10-18): a
  navigated class mirror holds the way to its class, so every use walks
  it afresh, and `superclass` and `gathered:` -- behind a class mirror's
  `slots`, `methods` and `nestedClasses` -- walk the chain once per call,
  so reading every level of a chain `n` deep through `superclass` costs
  `n²` steps. And an object mirror's `slots` asks `getSlot:`'s leaf for
  each name, which rebuilds the visible slots each time
  (`builtins/reflection.rs`, `visible_slots`), `n²` in the slots of the
  object. And an object mirror's `slots`, a mixin mirror's `slotsLazy:`
  and a class mirror's `gathered:` (`MirrorsFactory.ns`) build their
  result by repeated `, {...}` concatenation, `n²` in the members again.
  Nothing measures mirrors yet; a leaf answering the names and
  the values together, and one walking a whole chain once, are
  candidates.
  [liveness-and-reflection.md](design/liveness-and-reflection.md),
  "Mirrors that read".

- **The parser's 3.4 ms for `Kernel.ns`** (2026-10-14): a boot that
  compiles the shipped source still parses it, `Kernel.ns` in 3.44 ms of a
  13.6 ms boot that then compiled every module (release, the measuring
  workstation, Windows 11), and the first demand of a module on that path
  parses it too, `CollectionsFactory.ns` in 0.93 ms and
  `TypedArraysFactory.ns` in 0.58 ms. That is every boot under the
  reference lowering, under which every golden runs, every boot with
  `NS_NEXT_NO_PRIMITIVE_METHODS`, every boot of a binary built without a
  current artifact, and `cargo xtask shipped-code` itself, which every gate
  run runs. Part of it is the comment attachment the runtime discards
  ("Comment attachment: the runtime pays for it and discards the
  comments" below). [execution.md](design/execution.md), "What the
  precompiled shipped code moved";
  [core-library.md](design/core-library.md), "The embedded shipped code".

- **A release binary without the embedded shipped code boots from source**
  (2026-10-14): a tree that never ran `cargo xtask shipped-code`, or whose
  Rust or core-library sources changed after it last ran, embeds no current
  artifact, and the binary boots from source silently -- correct, but
  slowly: parsing, resolving and compiling the kernel alone took about 8.6
  ms of a 13.6 ms boot from source that compiled every module, where the
  same measurement's boot from the artifact took 1.6 ms. The gates
  regenerate it and the `commit` gate's
  `the_optimized_boot_decodes_the_embedded_code` is red without it, but a
  plain `cargo build --release` is not checked. A later packaging (`cargo
  install`, a release build for distribution) needs a way to require it, a
  build that fails without a current artifact, say.
  [core-library.md](design/core-library.md), "The embedded shipped code".

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
  drop the family-map probe per factory and initializer push
  (`inline_cache.rs`'s `base_for`, from `push_step` and
  `push_initializer`); its cost was never isolated. Since `new` is compiled
  code every instantiation pushes its factory body, so the probe is paid
  at least once per `new`. [dispatch.md](design/dispatch.md), "Code
  families".
- **Clean blocks** (noted 2026-10-22): a block that needs neither `self`,
  outer registers, an implicit receiver nor `^` needs no frame and could
  be one closure made once, as Cog's clean blocks are, instead of a
  closure, and a frame reified, at every `MakeClosure`. A lowering change
  still to be weighed. [compiler.md](design/compiler.md),
  [execution.md](design/execution.md), "Closure conversion".
- **Two sites still build a `Vec` before they allocate** (noted
  2026-10-22): `Array`'s copy leaf (`builtins/collections.rs`, the
  `copyFrom:to:` leaf, about line 192) collects the elements into a `Vec`
  and hands it to `Mutator::new_array`, and `handler_message`
  (`interpreter/resolve.rs`, about line 613) copies a `doesNotUnderstand:`
  message's arguments with `to_vec`, as `Array new:`, `MakeArray` and
  `MakeVector` did before road step 6. [memory.md](design/memory.md),
  "Arrays without a `Vec`".
- **Road step 6's residual slowdowns** (2026-10-22): no part of the step is
  slower than the tree before it with the 18-value ranges separated, but
  with the per-build medians separated and in both sittings the arrays
  part is +1.7% on NBody, and the fast `new` +1.1% on Richards, +0.8% on
  DeltaBlue and +0.5% on NBody and Havlak, about level on the
  microbenchmark sources. Neither obviously touches those programs' hot
  paths, so code or data layout is a likelier cause than the work; it is
  undecided. All five parts were kept (decided with the user on
  2026-10-22). The fast `new` is gone, replaced by `new` as compiled code,
  and measured again against `master` all seven micro sources and fifteen
  of sixteen Are We Fast Yet programs are faster, Sieve level, NBody
  included (-3.4%), so none of the residuals survives; what is open is the
  arrays part, to be measured again by itself with the next change to
  allocation. [execution.md](design/execution.md), "What inline
  allocation moved", "What `new` as compiled code moved".
- **The futures of simultaneous slots stay in Rust** (noted with the user
  on 2026-10-22). Forcing a future is Rust machinery -- a
  `Continuation::Future` on the forcing block's activation, and the
  stopped instruction run again once it is resolved
  (`interpreter/future.rs`; [execution.md](design/execution.md),
  "Running an instruction again after forcing a future") -- the last
  piece of a slot clause's semantics that is no code the compiler emits,
  as instantiation was before `new` became compiled code. A JIT sees it
  as an opaque call, and every instruction that can meet a future keeps
  its forcing path. How a future's forcing becomes code the compiler
  emits is open.
  [object-model.md](design/object-model.md), "Simultaneous slots".
- **A private nested class sent from a subclass instance never takes the
  cached field** (noted 2026-10-22). A private site's entry is guarded by
  N's application, and its hidden-slot index is that application's, so a
  hit reads the field only for a receiver of exactly that application
  (`inline_cache.rs`'s `materialize`); a receiver of a subclass takes the
  accessor's own path, two hash lookups, as before the field was cached.
  Recording the receiver's guard as well would take the entry past
  `Target`'s pinned 16 bytes. Measure a program that does this before
  acting. [object-model.md](design/object-model.md), "The nested-class
  accessor"; [dispatch.md](design/dispatch.md), "An entry".
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
- **A `ByteArray`'s bulk operations move one byte at a time.**
  `ByteArrayDataMut::copy_within` and `write` (`heap/object.rs`), behind
  the leaves that copy into or replace within a `ByteArray`, read and
  write each byte through `data_byte` and `set_data_byte`, and
  `read_element` and `write_element`, behind a typed array's and a
  `DataView`'s accessors, assemble the value byte by byte too. The
  data are whole words at a 32-byte address, so a word-wise path for the
  aligned middle of a range, and a whole-word load for an aligned element,
  are open; they matter for the bulk work the alignment exists for.
  Unmeasured; noted by the `ByteArray` sub-project, 2026-10-13.
  [core-library.md](design/core-library.md), "`ByteArray` in the heap,
  the movers and the image".
- **A `String` needle is encoded again at every candidate position.**
  `ByteArray`'s `indexOf:startingAt:`, `lastIndexOf:startingAt:`,
  `startsWith:` and `endsWith:` with a `String` argument
  (`builtins/bytes.rs`) compare it as its UTF-8 bytes by encoding it while
  they compare, allocating nothing, but anew at each position they try:
  O(n·m) with an encoder's constant per byte, where encoding a non-ASCII
  needle once into a buffer would cost one allocation. Unmeasured; noted
  2026-10-13. [core-library.md](design/core-library.md), "`ByteArray`'s
  protocol".
- **`StringBuilder>>addCodePoint:` grows for four bytes even for ASCII.**
  It reserves a code point's longest encoding before it writes, so a
  builder whose data are full grows one append early. A builder costs at
  most one early growth; noted 2026-10-13.
  [core-library.md](design/core-library.md), "`StringBuilder`".
- **The image encoder's ephemeron fixpoint rescans its pending list each
  round.** `GraphTable::build` walks every waiting ephemeron in every round
  until a round finds no key newly reached, so a chain of ephemerons each
  keyed by the previous one's value costs time quadratic in its length.
  Both collectors index their pending ephemerons by key instead
  (`heap/pending.rs`); the encoder could do the same, worth it only if a
  measurement shows an image meets such chains. Noted 2026-10-16.
  [core-library.md](design/core-library.md), "The heap and the
  collectors".
- **An application lists the declared slots of its whole chain.**
  `MixinApplication::declared_slots` (`mixin::declared_slots_of`) copies
  the superclass's list and extends it, so a chain of *n* levels with a
  slot each holds about *n*²/2 entries in all: about 5 x 10⁹ for the
  101,001 levels
  `goldens/language/class/instantiation/a_chain_deeper_than_the_depth_limit_overflows.ns`
  builds, which is why that golden's levels use initializer expressions
  instead. A list shared with the superclass's (a persistent list, or the
  level's own slots and a link) would keep it linear. Noted 2026-10-22.
  [object-model.md](design/object-model.md), "Instantiation".

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
- **The module namespace on the mirror.** `moduleInstanceNamed:`, and the
  build of a shipped module on its first demand, are built; the table
  of shipped modules is still the host's, a table entry has no "shipped" flag,
  and the file-system fallback for user modules does not exist — all three
  come with the first user module. Static module linking through it is decided,
  not built. [core-library.md](design/core-library.md), "Planned: the module
  namespace on the mirror".
- **Library classes beyond Primordial Soup's core API**, each when a module
  on `corpus/runnable.toml` or a program under `benchmarks/` asks for it:
  `IdentitySet`; exceptions beyond §8.6's core and Primordial Soup's
  (`ExceptionSet`, `Notification`, `Warning`, `outer`, `retryUsing:`,
  `signalerContext`, `Object>>error:`); an embedded compiled kernel image
  if the kernel grows. What
  Primordial Soup's core API has comes with "Next" above.
  [core-library.md](design/core-library.md).
- **Beyond the typed arrays and `DataView`** (noted with the user on
  2026-10-12, none needed yet): `Float16Array`, once Rust has a stable
  `f16` or with the GPU API; bulk leaves over a typed array --
  `add:into:`, `dot:`, and a `byteSwap` that converts a big-endian block
  once; a reading view with a cursor (`nextUint32` and the like) for
  parsers; and `List` applying the kernel's sequence mixin.
  [core-library.md](design/core-library.md), "`ByteArray`, `StringBuilder`
  and the views".
- **A two-byte string width.** A `String` outside Latin-1 takes four bytes
  a code point, where Python's PEP 393 keeps a two-byte width for the
  Basic Multilingual Plane; the width is canonical and private, so a third
  one would change no program, only the memory of non-Latin text (noted
  with the user on 2026-10-12). [object-model.md](design/object-model.md),
  "Strings have two widths".
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
  that continues a decoded frame, and an encoding for the side table of
  suspended activations -- every `Continuation` variant (`NestedClass`,
  holding a `NestedClassCreation`, `Unwind`, `VmErrorSignalled`, `Future`,
  `ReturnToSuspended`), the counting, the entered level -- and for
  `depth_reserve`; suspending the whole stack into its frames exists
  (`Heap::suspend_activations`). The decoder refuses every frame not
  marked uncontinuable (decided with the user on 2026-10-24), so suspend
  and resume has to lift that refusal, and verify everything a resumable
  frame carries before it does: the program counter against the body, the
  entered level against its application; `home` against `kind` is
  checked already. A top-level frame names no code and so cannot be
  resumed (`Heap::resume_activation` takes the body from the frame); the
  runtime never pushes one today, but a REPL's top-level activation
  would. [image-format.md](design/image-format.md), "Planned: suspend and
  resume".
- **A running world's image is trusted** (2026-10-12): a snapshot may hold
  frames of shipped code whose registers hold the kernel, a module instance
  or an `Activation`, and a crafted frame could pass one to the program's
  code, so such an image is loaded only when its host decides to. A check
  over each body's dataflow, which would let it be untrusted input like a
  started image, is undesigned. [image-format.md](design/image-format.md),
  "Shipped names".
- **A second class record applying a module's root row** (2026-10-13),
  beside the one the module section names, still loads, as it did before
  module roots became records: nothing a run writes has one, and whether
  the decoder should refuse it is undesigned
  (`image/modules.rs`, `module_root_classes`).
  [image-format.md](design/image-format.md), "Module section".
- **A class accessor is matched by class name across the whole module**
  (2026-10-13): `interpreter/shipped_world.rs`'s `accessed_classes` lets a
  private class be a value when a public class accessor anywhere in its
  module, at any depth, answers a class of its name. That is right while
  every shipped class is declared directly in its module's root, which a
  name requires; it is to be revisited when a shipped module nests a class
  deeper (the kernel's views and `StringBuilder` are declared directly in
  its root; a later module may nest one).
  [image-format.md](design/image-format.md), "Shipped names".
- **The debugger**: suspend, inspect, edit-and-continue at statement
  boundaries, restart frame, breakpoints; `defaultAction` is where it plugs in
  (a test-only `resume_unhandled_exceptions` hook stands in today). Blocked on
  the yield path above. [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **Reflection beyond road step 5b2a**: a class declared from source with
  no predecessor (`ClassDeclarationBuilder fromSource:`, loading a module
  at run time), a shape-changing `setClass:`, a superclass change, adding,
  replacing and removing nested classes, and `installAtomically:` over
  several builders. Once an install can remove a nested declaration, the
  removed declaration's row stays in its module's tree, which a full
  collection frees only whole (`Heap::module_trees`), so it lives until
  the whole module dies; whether to free such a row on its own then is
  that step's to decide ([object-model.md](design/object-model.md), "An
  application has two ids").
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "A
  MirrorBuilder and an atomic install".
- **A syntax error found while applying an earlier staged operation**
  (2026-10-19) reports a line of the intermediate text the operations
  before it made, not of the module's current text nor of the operation
  the program gave (`interpreter/edit.rs`, `prepare_edit`). Whether the
  diagnostic should name the operation, or a line of the text it was
  given, is undesigned.
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "A
  MirrorBuilder and an atomic install".
- **The activation and thread mirrors** (2026-10-18): road step 5a reads
  classes, mixins, methods and objects, not activations. §8.2 asks for
  "access to the current activation" through mirrors
  (`newspeak-spec.pdf` p. 47), and edit-and-continue needs a mirror on a
  suspended one; Primordial Soup's `ActivationMirror` and `ThreadMirror`
  (`prior-art/primordialsoup/newspeak/Mirrors.ns:32`, `:2484`) are the
  model, over the kernel-private activation objects that exist. An
  activation holds what the shipped-level rule withholds -- a shipped
  frame's registers hold the kernel and the VM mirror -- so the rule has
  to reach frames before one is handed out.
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "Mirrors
  that read".
- **A body's link to its declaration text** (2026-10-19), so that an
  image carries the text of code an older version runs, with the
  activation mirror that reads it. A closure or a frame of a method an
  install replaced or removed runs its old body after a load, but the
  image carries only the texts of the declarations' current versions, and
  nothing links a body to the text of the version that compiled it; in
  the live heap the superseded version keeps its text only while
  something holds it or a closure, frame or activation runs code compiled
  in it. Nothing asks a body for its text yet -- a method
  mirror reads the current version's methods -- so the link, and an image
  carrying the old text it names, come with the activation mirror above.
  [liveness-and-reflection.md](design/liveness-and-reflection.md),
  "Edited programs in images"; [image-format.md](design/image-format.md),
  "Edited programs in the image".
- **`evaluate:` and `perform:` through a mirror** (2026-10-18): an object
  mirror that evaluates an expression in the reflectee's scope, or sends
  it a message, answering a mirror on the result, as Primordial Soup's
  `ObjectMirror>>evaluate:` (`Mirrors.ns:1600-1606`) and the canonical
  `perform:with:ifFail:` (`prior-art/newspeak/MirrorsForSqueak.ns:1345`)
  do; §8.2's "modify itself" covers it. `evaluate:` needs the compiler
  reachable at run time, and both run the program's code, unlike every
  mirror operation built.
- **`instances` and `referringObjects`** (2026-10-18): a class mirror's
  instances and an object mirror's referrers, Primordial Soup's
  `ClassMirror>>instances` (`Mirrors.ns:915-916`) and
  `ObjectMirror>>referringObjects` (`:1655`), each a walk of the heap. Both
  must keep the shipped-level rule: an instance of a shipped class or a
  referrer that is a shipped module's root or the VM mirror is not a value
  a mirror may hand out.
- **Metadata through mirrors** (2026-10-18): §4.3 makes metadata comments,
  `(*:id: ... *)`, "available at runtime via mirrors" (`newspeak-spec.pdf`
  p. 21), as Primordial Soup's mirrors answer `metadata`
  (`Mirrors.ns:758`, `:849`, `:1233`, `:1269`). The parser attaches every
  comment to its node; nothing carries one past the front end, so a
  mixin, a method and a slot would have to keep theirs, and an image carry
  them.
- **`getSlot:` of a lazy slot** (2026-10-18): an object mirror's `slots`
  leaves a lazy slot out and `getSlot:` refuses its name, since reading
  the field would answer `nil` for a slot not yet computed and forcing it
  would run the program's initializer; §6.3.3 makes a lazy slot's value
  the slot's on first use. Primordial Soup's `getSlot:` reads a slot by
  its index (`Mirrors.ns:1628-1638`). Which of the two a mirror answers is
  undecided. [liveness-and-reflection.md](design/liveness-and-reflection.md),
  "No mirror hands out a value it reached through a shipped level".
- **The indexed elements in `ObjectMirror slots`** (2026-10-18): `slots`
  lists named slots only, so a mirror on an `Array`, a `ByteArray`, a
  `WeakArray` or a `String` lists none, and a program instance of any
  other shape answers an empty group. Primordial Soup's `slots` lists an
  indexed object's elements (`Mirrors.ns:1667-1676`, `slotsIndexed`).
- **A mirror on a future that the program resolves** (2026-10-18): a
  mirror forces no future and reflects an unresolved one as the future
  object itself, its anchor and reflectee included (pinned by
  `goldens/library/mirror/a_mirror_on_a_future_runs_nothing.ns`). Once the
  program resolves the future, the mirror goes on reflecting the future
  object until the next full collection replaces its slots with the value,
  and reflects the value from then on, so what it answers in between
  depends on when the collector ran. Whether a mirror should see a
  resolved future as its value at once -- as every other leaf does --
  is undecided, and no test pins the window.
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "A
  mirror forces no future".
- **Scoped invalidation.** Every invalidation flushes every inline cache:
  the bulk forward's (`InlineCaches::invalidate`), the cache stress mode's
  and the tests' (`Interpreter::invalidate_caches`), and the atomic install
  flushes them through the forward, or itself when it forwarded nothing.
  Scoping an invalidation to the applications an edit affects is deferred.
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
- **Running an ephemeron's finalizer comes with the actors' event loop**
  (decided with the user on 2026-10-16). Today an `Ephemeron`'s finalizer
  is held and never run: a collection whose key is reached by nothing but
  ephemerons mourns it, setting its key, value and finalizer to `nil`. A
  finalizer needs a point at which code may run without interrupting the
  program, which a turn of the event loop gives; the collector then fires
  an ephemeron instead -- keeps its key and value and queues it -- and the
  loop runs the finalizer between turns, as JavaScript's
  `FinalizationRegistry` does. [core-library.md](design/core-library.md),
  "Weak references".
- **Whether upstream strips source at packaging time**, a measurement and
  a switch in `CompilerApp.ns` that disagree, to be re-checked.
  [image-format.md](design/image-format.md), "Open questions".
- **A script recorded into an image.** `ns-next build` takes no `--script`:
  it sends no `main:args:`, and `load` takes only the arguments after `--`.
  Recording a project's script, or its `scripts`, so that `load --script`
  could start an image with them would need a section of the image format of
  its own. Noted 2026-10-05.
  [image-format.md](design/image-format.md), "Packaging".

## Planned: editor support and the analyzer

- **`disasm --initializer` leaves out the factory of a class that declares
  none** (noted 2026-10-22). It lists a factory entry's body only when the
  mixin declares a factory (`crates/ns-analyzer/src/commands/disasm.rs`),
  so a class with §6's default `new` shows no `method <Name> factory new`,
  though its metaclass's entry carries the same synthesized body.
  [compiler.md](design/compiler.md), "Reading the output".

- **Range formatting (`textDocument/rangeFormatting`) and formatting on
  type** (`textDocument/onTypeFormatting`). The server formats a whole
  open document only; the printer lays out a module from its tree and
  refuses a source that does not parse, which a line being typed usually
  is ([editor-support.md](design/editor-support.md), "Planned later";
  [formatting.md](design/formatting.md), "What is deliberately not done").
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
- **A hover with the comment above a declaration.** `textDocument/hover` on
  a class, slot or method shows the `Comment::text` of the `///` block (or
  `(* ... *)`) directly above its declaration, rendered as Markdown.
  `ns_syntax::Comment` answers the text, and the attachment puts the
  comment above a declaration on it (`Before`); what is missing is the
  request in the server ([frontend.md](design/frontend.md), "Comments";
  [editor-support.md](design/editor-support.md)).
- **Hints for a class's header slots**, deferred (decided with the user on
  2026-10-14): the hints are given for the locals of methods and closures
  only. A header slot needs more than its class's text to prove a fix
  behaviour-preserving: a non-private `::=` slot has a setter its clients
  and subclasses can send; a header's `x:: e` is a setter send a subclass
  can override, which an initializer does not send; and a private slot's
  read-only fix could lift a value object's refusal of a mutable slot,
  while whether a class's instances are values is not in its declaration
  ([editor-support.md](design/editor-support.md), "Hints").
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
§6.3.4); `///` starts a line comment wherever it stands, so a binary
selector ends before it (§4.2); `outer N` without a message answers the
enclosing object (§5.9);
value objects are never copied across an actor boundary (§3.1.1); the
MirrorBuilder changes one class at a time and not every part of it
(§8.2): no mirror changes the class of an individual object or reaches
the current activation ("The activation and thread mirrors" above),
several builders are not installed as one, a superclass, the nested
classes and a class from source with no predecessor are not edited
("Reflection beyond road step 5b2a" above).

## Open design questions

- **Whether the interpreter tier is generated by the JIT's backend**
  (2026-10-25). The interpreter is the Rust `match` loop `vm::run`, whose
  register assignment LLVM decides; generated by the macro assembler of
  road step 8 instead, as JSC's LLInt (offlineasm), V8's Ignition (built
  by the optimizing compiler's backend) and HotSpot's template interpreter
  are, it would keep the program counter, the register base and the
  mutator in fixed machine registers and be free of LLVM's codegen
  surprises, typically 1.3-2x on interpreter-bound code (`vm::run` holds
  about 60-70% of `bench_send.ns`'s samples). A hand-written interpreter,
  as LuaJIT's, is not the option: beside a generated JIT it is a second
  implementation of every instruction, which drifts. A cheaper measurement
  can come first: a tail-call interpreter, one Rust function per
  instruction, each ending in a guaranteed tail call to the next
  instruction's handler, keeps the program counter, the register base and
  the mutator in argument registers across handlers with no generated
  tier. In a published experiment on a stack machine (Zack Radisic,
  "Template interpreters", zackoverflow.dev, 2025-11-08, aarch64 only) it
  was nearly as fast as a template interpreter, which ran `fibonacci(46)`
  about 2.5x faster than direct threading; those benchmarks have no
  objects and no sends, so they say nothing of the send path, where most
  of this interpreter's time goes. Rust guarantees such a call only with
  `become` (feature `explicit_tail_calls`), which in October 2026 is
  unstable and incomplete, aimed at stabilization in 2027 together with
  `loop_match`; without the guarantee every handler deepens the native
  stack. Once it is stable, the experiment shows how much of the 1.3-2x
  fixed registers give here before road step 8 builds its macro
  assembler. Decided only once the instruction set and the frame layout
  settle and the macro assembler exists.
  [execution.md](design/execution.md), "Planned: JIT tiers".
- **The resolver as a crate of its own.** It lives in `ns-runtime`
  (`resolver/`). A crate of its own would separate the front end from the VM
  and let a tool that needs only bindings — a lighter language server —
  leave the runtime unlinked (the formatter needs none: it is
  `ns-syntax`'s). It is not worth it yet: the
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
  Road step 5b2a refuses every edit to a shipped mixin, so the question stays
  open until editing kernel methods is offered.
  [liveness-and-reflection.md](design/liveness-and-reflection.md).
- **Whether a shape-changing `setClass:` is ever offered, and at what
  cost.** §8.2 makes it possible to change the class of an individual
  object; with `become:` an eager bulk forward, changing one object to a
  class of another shape would pay a walk of the whole heap for one object.
  The canonical Newspeak's `setClass:` rewrites the class word in place
  (`changeClassOf:to:`), and Primordial Soup offers none. Road step 5b2a
  leaves it out.
  [object-model.md](design/object-model.md), "`become:` is an eager bulk
  forward".
- **Whether lazy forwarding is revisited.** `become:` is eager, one walk per
  forward, because its one consumer, the atomic install, walks the heap
  anyway; Spur's lazy forwarders would cost every send, `==`, leaf and
  generated code a check. Revisit it, behind the same bulk-forward seam,
  only if a measurement shows `become:` frequent (decided with the user on
  2026-10-05). [object-model.md](design/object-model.md), "`become:` is
  an eager bulk forward".
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

- **`vm::run`'s stack frame grew from 744 to 904 bytes** (2026-10-26)
  when every raising instruction came to store its program counter
  before it unwinds (`vm::raise`, which passes the index through the
  `Handoff`). The change adds no local to `vm::run`; every variant that
  stores the counter in the raising arms, an inline store included, gave
  904 bytes, so it is likely LLVM's spill slots shifting. Measured with
  three builds a side, the 24 programs are level (+0.07%, none slower
  with its builds separated), and 904 bytes stays under a page, so no
  stack probe is added; comparing the base's and the head's assembly of
  `vm::run` would name it. [execution.md](design/execution.md), "Where
  `program_counter` is stored".
- **DeltaBlueTon80 0.9% slower after overlapping the register windows**
  (2026-10-26), with its builds separated, against the first stage of
  road step 7a′ with the value-answered send's step forgotten; it was
  1.6% before the pop's counts were decremented apart and an inlined
  conditional answered in its argument slot, and Mandelbrot's 0.5% did
  not reproduce. What remains is the return path: the loop reads the
  frame's flags for its register base on every switch, since the frame's
  start lies its argument count below its header (the switch handler
  2,189 samples against 1,619, about 0.35% of a run), and `vm::finish`
  and `vm::resume_caller` take about 550 and 340-600 samples more, on
  the loads right after the pop. The base handed over in the `Handoff`
  or in `Transfer::Switched` measured slower, read from the code's
  argument count level, so the read stays (2026-10-26). Open: it costs
  what the loop's state passing through memory costs, which an
  interpreter tier with fixed machine registers for the frame's state
  would not ("Whether the interpreter tier is generated by the JIT's
  backend" above).
  [execution.md](design/execution.md), "What overlapping windows moved".

- **The send and return functions move with the collector's code size**
  (2026-10-25). Inlining `Heap::trace_stack` into the scavenge grew
  `World::scavenge` from 1,528 to 2,763 instructions and moved
  `pop_activation`, `resume_caller` and `polymorphic_hit`, linked after
  it, by `0x3380` to other offsets within their 4 KiB pages; RichardsTon80,
  which spends no sample in the collector, measured +0.6% to +1.6% in four
  timings, about 1.3%. `trace_stack` is kept out of line for it, but any
  later growth of the collector's code, or of anything linked before the
  hot functions, can move them again, and a Towers separation in one
  sitting that did not reproduce looked the same. What would settle it:
  which placement of those functions costs, found by pinning it with a
  linker order file and measuring RichardsTon80 at each placement. [execution.md](design/execution.md), "What the
  leftover analysis moved".

- **`SendKey`'s word for `current_class`** (2026-10-22). A `super` send's
  key carries the running class beside S, so that a nested-class hit on a
  `super` site keeps its cached field for a receiver of the running
  class's application (`inline_cache.rs`, `SendKey`, `materialize`); every
  other send sets it to `None`. A monomorphic hit pays nothing for it, a
  polymorphic or global hit one store more, and the key is passed by
  reference because its copies were measured to matter. Not isolated by a
  measurement. The alternative that costs no word: a `super` key has no
  use for `lexical_class`, so it could carry `current_class` there. Still
  not isolated by a measurement: no micro source sends `super` or reaches a
  polymorphic or global hit with a `super` key, and "What `new` as compiled
  code moved" measured the whole batch, not this word.
  [dispatch.md](design/dispatch.md), "A send, step by step";
  [execution.md](design/execution.md), "What `new` as compiled code moved".

- **A store-forwarding stall in `push_step`'s `FrameKind` write**
  (2026-10-23). `FrameKind` is written as a byte and a word into a stack
  slot that the merged push code reads back in sixteen bytes: about 6% of
  `bench_new.ns`'s samples. A restructured entry write would speed every
  push, not only a factory's; not attempted.
  [execution.md](design/execution.md), "What `new` as compiled code moved".

- **No micro source covers instantiating a class whose levels run no code
  (a factory started at its `Return`), `super` sends, or body-less
  declarations** (2026-10-23). The goldens and the debug cross-check of
  `lexical_origin` hold their correctness; their speed is
  unmeasured. [execution.md](design/execution.md), "What `new` as compiled
  code moved".

- **What the reclamation of ids costs a full collection is unmeasured**
  (2026-10-20). `drain_marking` gained one match arm that records the ids
  a class object, a closure and a frame name (`GatheredIds`), and
  `Mutator::reclaim_ids` walks every table once per full collection; a
  superseded version only the table holds has its bodies decoded once per
  collection to ask whether a closure, frame or activation still runs one
  (`Heap::drop_superseded`, `image::version_bodies`). None of it was
  measured in a release build; a release measurement of a
  collection-heavy program (`NS_NEXT_STRESS_COLLECT` is a debug-only stress
  mode, not a measure) decides whether any of it shows.
  [object-model.md](design/object-model.md), "An application has two
  ids"; [memory.md](design/memory.md), "Marking".

- **`Heap::same_mixin` looks an id up for every level that differs by
  reference** (2026-10-18). Since road step 5b2a an old and a new
  application of one id match, so `Heap::with_level_applying_mixin_of`
  (behind `Mutator::kernel_slot` and an ephemeron's slot base,
  `builtins/weak.rs`) and `builtins/activation.rs`'s `inherits_mixin_of`
  (an exception handler's class test) compare each level of a chain by
  reference and then, where that fails, by `known_mixin_id`, a hash
  lookup per non-matching level, which the pointer compare alone never
  paid. It is unmeasured; a release measurement of an exception-heavy
  program decides whether to narrow it, for example to look the
  candidate's id up only when the id was edited, since an id never edited
  has one version; the heap keeps no mark of an edited id, so that would
  add one, set by `Heap::replace_mixin`.
  [liveness-and-reflection.md](design/liveness-and-reflection.md), "A
  MirrorBuilder and an atomic install".

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
- **`StringBuilder`'s state check on every member is unmeasured.** Every
  member that reads a builder's slots sends the private `checkState:`
  first, `size` and `isEmpty` included, so that a builder an image crafted
  is refused with an `ArgumentError` (`cc2aaebe`); what that costs a
  program that appends in a loop is not measured. Noted 2026-10-13.
  [core-library.md](design/core-library.md), "`StringBuilder`".
- **3.5 ms of the precompiled shipped code's gain lie outside the boot.**
  Embedding the shipped modules' compiled code shortened the boot by 12.0
  ms (13.6 ms to 1.6 ms, the summed medians of temporary marks) and a hello
  program's whole process by 15.5 ms (20.8 ms to 5.2 ms, median of 40;
  release, the measuring workstation, Windows 11). The 3.5 ms difference
  falls outside what the marks cover, and its cause is not established; one
  build per side, one program and one machine were measured. Marks around
  what precedes `bootstrap_with` and follows the platform's `build` -- the
  process start, reading the program, the run, the exit -- would place it.
  Noted 2026-10-14. [execution.md](design/execution.md), "What the
  precompiled shipped code moved".

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
  process (`tests/image.rs`, `a_stopwatch_is_a_record_of_its_shipped_class`).
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
  language server does, would close it. Until it is closed, the overflow at
  about 20,000 links sits below the instruction limit of 2^20, so that limit
  is not yet the real bound on a chain ([compiler.md](design/compiler.md),
  "Limits"). [frontend.md](design/frontend.md), "Recovery", rule 7;
  [execution.md](design/execution.md), "What the trampoline moved". Noted
  2026-10-04.
- **The AST's derived `PartialEq` and `Debug` recurse along a chain of
  messages** (`crates/ns-syntax/src/ast.rs`), as the compiler does: only
  `Send`'s `Drop` and `Clone` and `Send::receiver_chain` take a chain link
  by link. `ns-analyzer`'s `the_editor_parse_agrees_with_parse`
  compares modules with `==`, and the snapshot tests print them with
  `{:?}`; both are safe only because no fixture holds such a chain.
  Hand-written impls that take the chain link by link would close it.
  Noted 2026-10-04.
- **A read-only local a closure captures before its initializer runs reads
  nil under the optimized lowering.** The optimized lowering never puts an
  immutable (`=`) local in the indirection vector, so a closure made before
  the local's initializer has run copies its register when it is made, while
  it is still nil: `| t = platform transcript. f = [x]. x = 1. |  t show: f
  value printString` prints `nil` optimized and `1` under the reference
  lowering, and a self-referential `fib = [:n | ... fib value: n - 1 ...]`
  raises "does not understand #value:" where `fib ::= [...]` answers. The
  argument of [compiler.md](design/compiler.md)'s "A read-only local is
  never copied" -- its register is written by its initialization and by
  nothing after it -- overlooks a capture that precedes the initialization.
  The fix belongs in the closure conversion
  (`crates/ns-runtime/src/compiler/capture.rs`): putting a read-only local
  that such a closure captures in the vector, as a written one is, would
  close it. The editor's hint B stays conservative regardless, never making
  such a local `=` ([editor-support.md](design/editor-support.md), "Hints").
  Noted 2026-10-14.

## Known defects and gaps

None of these changes a correct program's output; each is wrong but safe, or
reachable only from a crafted artifact, except the first four, which a
program can reach and which end the process, and six a program meets as a
gap: `ns-next run`'s panic on a closed pipe, `Object>>printString`'s one
text, `to:by:do:`'s zero step, `String withAll:`'s nil for an argument
that only answers `isKindOfArray` or `isKindOfString`, the error of a
`hash` that is no Integer, and the `u16` ceilings of instantiation.

- **`<<`, `Array new:`, `ByteArray new:` and `WeakArray new:` probe a
  program-chosen size with
  `try_reserve_exact`, which refuses an impossible size on Windows but on a
  system that overcommits (Linux) can succeed**, and the process is then
  OOM-killed while the zeros are written, where a program-chosen size is
  meant never to abort the process. No such leaf can tell an affordable size
  from one the system will accept and then kill for.
  `crates/ns-runtime/src/builtins/collections.rs`, `allocate`, through
  `Heap::array_fits`, which probes an `Array` at or above the large-object
  threshold;
  `heap.rs`, `Heap::byte_array_fits`, behind every leaf that makes a
  `ByteArray` of a size the program chose, and `Heap::weak_array_fits`,
  behind `WeakArray new:`; and
  `builtins/numbers/mod.rs`, `shift_left`, which reaches
  `ArbitraryInteger::shift_left`
  (`crates/ns-runtime/src/arbitrary_integer/mod.rs`), where the digits are
  reserved;
  [core-library.md](design/core-library.md), "Large integers".
- **A large `Array`, `ByteArray` or `WeakArray` that fits the old space
  but not its free room still aborts** (noted 2026-10-22; the probe is
  older than road step 6). `Heap::fields_fit`, behind `array_fits`,
  `byte_array_fits` and `weak_array_fits`, compares the object's bytes against the old space's
  size -- its end less its base -- not against what is left of it above
  its top, so a length the probe passes can still find no room, and
  `allocate_block` then aborts as running out of memory where the leaf
  meant to answer its `ArgumentError`. `heap.rs`, `Heap::fields_fit`;
  [memory.md](design/memory.md), "Arrays without a `Vec`".
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
  about 129 × (limit + reserve) stack frames: `MAX_BLOCK_NESTING` is 128
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
  cannot tell them apart. Noted 2026-10-01. `Array` and `ByteArray` take
  it too, so a refusal text the kernel builds in Newspeak, with the
  argument's `printString`, says "got an Object" for an `Array` or a
  `ByteArray` (`StringBuilder>>add: (ByteArray new: 2)`, "add: requires a
  String, got an Object"), where a leaf's, built with `Heap::describe`,
  says "a ByteArray of 2"; a `printString` of their own for the two, or
  the refusals routed through `describe`, would close it (noted
  2026-10-13; [core-library.md](design/core-library.md), "Errors and
  tests").
- **`IndexedCollection`'s range checks refuse a non-Integer bound with
  another receiver.** `copyFrom:to:` and `replaceFrom:to:with:startingAt:`
  in the kernel mixin, which `TypedArray` and a source of another kind
  reach, check a bound that is no `Integer` by sending the same message
  to an empty `Array` made for the purpose, so that the text is `Array`'s
  leaf's; the `ArgumentError`'s text is right, but the receiver it
  records, which an unhandled error reports to the host
  (`Exception>>hostReceiver`), is that `Array`, not the collection the
  program sent to (`Kernel.ns`,
  `IndexedCollection`, `checkRangeFrom:to:keyword:ifNoInteger:`). A text
  built in Newspeak would end the detour. Noted 2026-10-13.
  [core-library.md](design/core-library.md), "The sequence protocol is a
  mixin".
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
- **Formatter: a long tail on a hugged block's `]` line can pass 100
  columns.** A `Choice` is decided by the first line of each alternative,
  so of the hugged form only the line up to the first `[` is measured; what
  follows the last `]` is measured again only when it is a group of its own
  (`crates/ns-syntax/src/printer/document.rs`, `layout`), and a long run of
  plain links there -- `(a foo: [ … ]) bar baz qux …`, `] on: Error do:
  handler` -- stays on the line. [formatting.md](design/formatting.md),
  "Measuring again after a line break". Noted 2026-10-12.
- **Formatter: a group may break where breaking does not shorten the
  line.** A group is measured up to the next place a line may break, the
  text after it included, so a short inner group breaks when what follows
  it is what makes the line too long; its own part gets no shorter, the
  line still overflows, and the break reads as one for nothing -- `(outer`
  and `MixinInitializerOrder` split apart in
  `goldens/language/class/mixin/mixin_initializer_order.ns`, and two nested
  levels continuing at the same +4 column. [formatting.md](design/formatting.md),
  "Outside in, all or nothing". Noted 2026-10-12.
- **`ns-next-analyzer fmt`'s rename over the target drops the file's
  permissions and replaces a symlink with a file.** The formatted text is
  written beside the target and renamed over it
  (`crates/ns-analyzer/src/commands/fmt.rs`, `write_replacing`), so the
  result has the new file's default permissions, and a symlink named on the
  command line becomes a regular file while its target stays unformatted.
  Noted 2026-10-12.
- **`ClassDeclaration::class_comment()` relies on spans**
  (`crates/ns-syntax/src/ast.rs`): it tells a comment after the header's `(`
  from one after the first initializer expression by comparing their spans,
  so on a position-erased tree (`Module::without_positions`) or one the
  runtime builds, whose spans are zero, it can answer the wrong comment or
  none. Noted 2026-10-12.
- **Formatter: a comment on its own line before a chain's receiver or an
  operand breaks every level around it.** `^x +` then `(* c *)` then `y`
  prints `^x`, `+` alone on a continuation line, and the comment and `y`
  at +8; `^(` then `/// c` then `a foo) bar` puts `^(`, the comment, `a`,
  `foo)` and `bar` each on a line of its own. The comment's `FreshLine`
  marks every group around it broken, and `expression` puts the operand
  at +4 after it, so rule 21's "the level it stands in" is taken
  literally for each level out to the statement. Idempotent and
  preserving, but it reads badly; the comment's break should end at the
  operand. [formatting.md](design/formatting.md), rule 21.
  Noted 2026-10-12.
- **Comment attachment: the runtime pays for it and discards the
  comments.** The attachment is linear in the source and in the comments
  (an `Inner` count is two binary searches over prefix sums built once per
  holder; the oracle tests check its numbers, and
  `the_work_grows_linearly_along_a_chain` and
  `the_work_grows_linearly_with_comments_among_many_children` hold the
  descent and the prepass linear). What stays is that `parse` attaches every
  comment and the runtime throws them away: under mimalloc, the allocator
  of both binaries, `parse` of `Kernel.ns` takes 0.81-0.84 ms, and 0.51
  ms with its 3,230 lines of `///` comments stripped, so the comments
  cost about 0.3 ms, 38 % of the parse (`cargo bench -p ns-syntax --bench
  corpus -- <directory> --rounds 100` from `crates/ns-syntax`, the
  benchmark built with a `#[global_allocator]` of mimalloc added;
  2026-10-17, the measuring workstation, Windows 11, release with the
  alignment pin; [frontend.md](design/frontend.md), "Comments"). That is
  paid on every boot that compiles the shipped source (`interpreter/kernel.rs`, `interpreter/core_library.rs`
  call `ns_syntax::parse`); an optimized boot from the embedded shipped
  code parses nothing ("The parser's 3.4 ms for `Kernel.ns`" above). A
  `parse` that skips the attachment for the runtime would recover it.
  [frontend.md](design/frontend.md), "Comments".
- **Parsing files in parallel.** Every module parses independently, so
  the callers that parse many -- `ns-next-analyzer parse-corpus` and
  `fmt`, the language server once it reads a whole workspace, `cargo xtask
  shipped-code` and a boot from source that parses several core-library
  modules -- can parse them on several threads (`std::thread::scope`, no
  dependency). Within one file the steps are sequential, and the largest
  file takes about 3.5 ms under mimalloc, too little for threads to pay.
  Not measured yet. Noted
  2026-10-17.
- **Fewer allocations in the syntax tree: measured, and not worth it
  yet** (decided with the user on 2026-10-17). Under mimalloc a samply
  profile of `parse` and `parse_for_editor` over the corpus puts the
  parser at 46 % (its own work 25, allocation 8, `memcpy` 13), the
  attachment at 20, the lexer at 18 and dropping the module at 15 (drop
  glue 10, the allocator 4). The `memcpy` is the nodes' size: an
  `Expression` is 160 bytes, a `Send` 296, a `Statement` 200, a
  `MethodDeclaration` 320, moved by value as they are built. A throwaway
  global allocator that bumps a pointer during each parse and frees
  nothing measured the corpus's `parse` at 40.6 ms against 45.0 ms when
  the module is still dropped (10 % less) and at 34.3 ms when it is not
  walked at all (24 % less; `Kernel.ns` 0.66 ms against 0.85 ms; 13 MB
  for the largest parse; three runs, release with the alignment pin).
  The second figure is what an arena of nodes without `Drop`, as oxc's,
  would approach; it puts a lifetime on every AST type through
  `ns-runtime`, `ns-tooling`, `ns-analyzer` and `ns-cli`, and makes the
  language server hold source, arena and tree together. No user notices
  0.25 ms on the largest file, so it waits for a workload where parsing
  dominates -- a language server reading large workspaces, say. Smaller
  nodes (rare, large parts behind a `Box`) need no lifetime and aim at
  the 13 % of `memcpy`, an estimated 5-8 %, not measured. A different
  parsing algorithm would not help: the fastest parsers (V8, Clang, swc,
  oxc) are hand-written recursive descent too, and the cost is building
  the tree, which any algorithm building this AST pays; how much the
  parser's speculative parses re-read on a clean source is not measured.
- **The feedback range a decoded closure is given is never reclaimed**
  (2026-10-20). A closure an image decoded carries no base and is given a
  range of its own on its first activation (`InlineCaches::fresh_range`,
  from `Mutator`'s closure push), which belongs to no family in the
  family map, so the full collection that frees the ranges of families
  nothing runs (`InlineCaches::forget_families`) never frees it, even once
  the closure is dead. The table grows by one range per decoded closure
  ever run, bounded by the image, not by how often anything runs.
  [dispatch.md](design/dispatch.md), "Code families".
- **A superseded class side held after its instance side was dropped
  names itself `class side`** (2026-10-20). A full collection drops a
  superseded instance side that only the table holds while a Rust holder
  may still keep its class side; the class side's `Weak` link to its
  instance side then no longer upgrades, so `Mixin::reflected_name`
  answers the bare class-side name instead of `Foo class`. No reader a
  program reaches meets such a version, since reflection goes through the
  id to the current version. [object-model.md](design/object-model.md),
  "An application has two ids".

- **A host's `Interpreter::instantiate` builds its factory body afresh on
  every call** (2026-10-22). `decide_instantiation` builds `factory_body`
  of the argument count instead of reading the class's factory entry. It is
  a cold path that only a host reaches. Past the argument limit of 255 the
  arity is checked first, so a wrong count raises the arity error, as a
  send's does. Only a class whose factory itself takes more than 255
  parameters raises "factory too large".
  [object-model.md](design/object-model.md), "Instantiation".
- **A code row records the first `MixinMethod` that registered its body**
  (2026-10-24): `Heap::register_code` fills a row's `method` only while it
  is `None`, and `reify` rebuilds a method frame's `FrameKind::Method` from
  that row (`Heap::code_method`). Were two methods ever to share one
  `Rc<CompiledCode>`, a frame of the second would reify naming the first,
  and a mirror or an image would see the wrong method. Nothing shows that
  no two do: the image decoder builds a superseded version's method around
  the decoded body (`image/decode.rs`), and a lazy slot's getter takes the
  code it is handed (`MixinMethod::lazy_slot_getter`). Either show in
  [object-model.md](design/object-model.md), "An application has two ids",
  that every `MixinMethod` owns its body, or key the row by method.
  [execution.md](design/execution.md), "Representation".

## Missing tests

- The `ByteArray` sub-project's (noted 2026-10-13): the movers' test
  (`heap.rs`,
  `an_aligned_byte_array_keeps_its_bytes_through_every_mover_at_every_position`)
  has no `ByteArray` of size 0 and none large enough for an overflow word;
  the integration round trip (`tests/image.rs`) does not assert the
  decoded data's alignment, which only the unit test checks; `inspect`
  is not run over every cut of a byte-array image; no image test gives a
  byte-array body a class record's header (a program's class) rather than
  a shipped one; `ByteArray new:`'s refusal of a size the operating
  system will not commit has no test, since no portable one can provoke
  it; no golden appends `''` to a builder whose data are full;
  `a_typed_array_has_the_sequence_protocol` cannot
  tell an `Int16Array`'s copy from a `Uint16Array`'s; a `LargeInteger`
  index is not pinned by a golden; and the crafted-slot tests cover a `BigEndianDataView` and
  one typed array, not `LittleEndianDataView` or the other kinds.
  [core-library.md](design/core-library.md), "Errors and tests".
- That a parenthesised literal block is still inlined (`true ifTrue:
  ([1])`) is pinned only by the printed answer of its golden, not by a
  `disasm`-level check that it lowers inline.
- The end position of the language server's formatting edit for a document
  ending with a line break, LF and CRLF (`LineIndex::new` records a line
  start after the last break; no test fixes the position the edit ends at).
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
- The encoder's refusal of a class record without a metaclass
  (`image/encode.rs`).
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
  0; the version word in `image_corruptions.rs`'s corruption sweep;
  `load --print-source` of an image `image::snapshot` wrote; and `run-corpus`
  refusing a row whose `send` is not unary.
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
- `new` as compiled code's (noted 2026-10-22): the reshape test
  (`inline_cache.rs`,
  `a_nested_class_hit_after_the_enclosing_object_was_reshaped_takes_the_slow_path`)
  never runs a hit on a stale guard, since every forward and every edit
  invalidates the caches before the next send.
- Frames as words' (noted 2026-10-24): no test of `ns-memory` checks the
  accessible words' end after `Reservation::record_failure`, which no
  portable test can provoke; `record` refreshes it with the byte end on
  every change to the page map, so only the commit and decommit test
  (`the_accessible_words_are_the_committed_read_write_prefix`) pins it.

## Code quality follow-ups

Small, deliberately left, none affecting an outcome.

- Leftovers of the cast lints' reviews (noted 2026-10-26): no 32-bit
  target is installed or gated, so the `cfg(not(target_pointer_width =
  "64"))` paths and the 32-bit index fix (`builtins/collections.rs`'s
  `element_position`, `builtins/value.rs`'s `SlotAt`) are never built,
  only reasoned about; the analyzer's clamping test pins `lsp_number`
  (`documents.rs`), not a `LineIndex` over a text past `u32`;
  `bytecode/leftovers.rs` narrows an instruction index to `u32` with the
  same `expect` in two places; `arbitrary_integer`'s `low_digit` also
  narrows two values that are bounded rather than split (`division.rs`'s
  `divide_wide`, `multiplication.rs`'s final carry), which its doc does
  not say; and `conversion.rs`'s `radix_digit` checks every printed digit
  against a radix the compiler cannot see is at most 36.

- Leftover of road step 7b (noted 2026-10-24): the format- and test-level
  frame accessors `Heap::frame_set_program_counter`, `frame_register_shape`,
  `frame_record` and `frame_set_uncontinuable` debug-assert only that the
  frame is not married (`frame_activation` is none), which a suspended frame
  passes; their callers are the encoder, which debug-asserts separately
  that no frame it writes is suspended, and tests, so asserting that the
  frame has returned (`!frame_is_running`) would say what they mean.

- Leftovers of `new` as compiled code's reviews (noted 2026-10-22): the
  reference lowering's `Body::prologue` (`compiler/reference.rs`) does not
  stop lowering once the body is too large, as `lower.rs`'s prologue does;
  `Heap::frame_kind` (`heap.rs`) calls `InitializerBody::of_frame_tag`
  twice for one tag; `RETIRED_CODE_KIND_TAGS`'s doc in `image/opcodes.rs`
  names the reader of the live tags loosely, as "`decode`"; and a doc
  comment above `past_sixteen_bits_the_other_tables_take_the_wide_form`
  (`bytecode/compact.rs`) is wrapped unevenly.

- Leftovers of the Java benchmarks' reviews (noted 2026-10-22): a lone
  `--warm-up` or `--measure` without `--timed` is read as a benchmark
  name and answers `Unknown benchmark: --warm-up` rather than the usage,
  the same in both harnesses; `flags::<side>::rejected` does not cover
  `+5`, an empty value or one past the Java harness's range; the
  adapted DeltaBlue, Havlak and CD files keep upstream's comments, which
  name identifiers the adaptation wrote out (`benchmarks/are-we-fast-yet/java/`);
  and the trials' source key is `DefaultHasher`'s, stable per toolchain
  only, so a toolchain change recompiles the classes once
  (`crates/ns-cli/tests/run_benchmarks.rs`; `execution.md`, "The Are We
  Fast Yet benchmarks").

- **The gates chosen from what a change touched** (noted 2026-10-14, with
  the user): `cargo xtask gates <group>` runs the groups `CLAUDE.md`'s table
  "The gates" names, but which groups a commit or a batch needs is still
  read off the table by hand. `xtask` is the place to choose them from
  `git diff --name-only` against the table's paths, one group, say
  `changed`, so that a gate the table asks for cannot be forgotten.

- Leftovers of road step 5a's reviews (noted 2026-10-18): the "once" of
  `Heap::record_mixin_object` is a `debug_assert!` only; a thread that
  boots both from the embedded shipped code and from source holds two
  copies of a shipped module's text, `shipped_module_text`'s and the
  resolved module's (`interpreter/core_library.rs`);
  `ClassDeclarationMirror>>=` is asymmetric with an object that answers
  `isKindOfClassDeclarationMirror` falsely, which is harmless, since the
  `MixinMirror>>=` it ends in hands the reified mixin to the module's own
  mirrors only; a class side's `simpleName` is
  its `name`, `'Klass class'`, which no prior art pins; and the shipped
  boundary's walk (`tests/platform.rs`,
  `no_mirror_answers_a_withheld_value`) sends every public unary method of
  every facet, `garbageCollect` and the transcript's `newLine` among them,
  and pins exact counts, seven facets and 37 classes, that change with
  every facet. Three comments of `MirrorsFactory.ns` and one link say
  less or more than the code: the root's "forces no future it is given"
  holds for reflectees and anchors, while a future passed as the argument
  of `ObjectMirror>>=` or `findMirrorNamed:` is forced by the ordinary
  sends to it; `ClassMirror>>=`'s comment names the asymmetry against a
  navigated mirror only, though a program's subclass against a plain
  `ClassMirror` is asymmetric too; and `object-model.md`'s bullet on
  futures links "A mirror sends its reflectee nothing" rather than "A
  mirror forces no future".

- Leftovers of the `ByteArray` sub-project's reviews (noted 2026-10-13):
  `Heap::byte_array_words` and `byte_array_words_mut` repeat the format
  check and the size computation; a string's UTF-8 size is computed twice,
  by `CodePoints::utf8_size` and by `argument_bytes` (`builtins/bytes.rs`);
  `builtins/bytes.rs` and `builtins/text.rs` convert sizes with `as i64`
  where a checked conversion would say why it cannot fail; `Element` and
  `Heap::byte_array_element` are public only for the tests; and
  `DecodeError::ByteArrayRecordMismatch` carries two meanings, told apart
  by its `body`, where two variants would name each.

- Nested inlined conditionals compile to code that doubles with each
  level: an inlined form's fallback closure holds its blocks compiled
  again, with their own inlined forms and fallbacks. `a ifTrue: [b g: a.
  X]` ten deep compiles to 1,024 bodies, and twenty deep took 4.7 s
  where ten took 5 ms (measured 2026-10-25, a debug test build, while
  bounding `bytecode::leftovers`). Every level is valid Newspeak, and a
  fallback could instead share one closure body per block literal. A
  source of about 400 bytes, `(a and: [(b g: a) or: [...]])` ten deep,
  compiles to 1,048,576 bodies and 18,175,273 instructions under the
  optimized lowering, against 21 bodies and 106 instructions under the
  reference one. The duplication multiplies the bodies and call sites per
  family, about 17 instructions per body there, so the per-body
  instruction limit of 2^20 does not bound it
  ([compiler.md](design/compiler.md), "Limits").
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
  (noted 2026-10-03): test names that outlived what they name
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

- **The reflective checks back in goldens** (2026-10-18): what only the
  representation showed while `class` and `Object`'s class accessors are
  protected -- the boundary between `Integer` and `LargeInteger`, chains
  of metaclasses, the identity of class objects -- is checked from the
  host by `crates/ns-runtime/tests/representation.rs`. The mirrors of
  road step 5a can check it from goldens (`getClass name`, `isMeta`, a
  class mirror's `=`), but for `Metaclass` refusing instantiation, which a
  mirror does not hand out; those goldens are not written.
  [core-library.md](design/core-library.md), "`class` and the class
  accessors are protected".

- **The amplification bound of a rejected load is counted with two-byte
  class tags.** `decode_value`'s doc comment (`image/decode.rs`), the
  table in [memory.md](design/memory.md), "The amplification bound of a
  rejected load", and `CLAUDE.md`'s image invariant take a class tag at
  two bytes, the builtin class tag's size, where every live tag is five
  (`CLASS_RECORD`, `CLASS_SHIPPED`); only the byte-array record is counted
  at five. The figures are safe upper bounds, not tight ones: the
  smallest metaclass record is 15 bytes, not 9, and the release peak
  about 8x, not 10.7x. Redoing the derivation and the three statements
  together, in one commit that runs the gates, would make them tight.
  Noted 2026-10-13.

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
