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

**The specification of ns-next, its fourth batch: the image format**
(decided with the user on 2026-11-23, before the debugger's stage 6 goes
on; [specification.md](design/specification.md), "The order of building
it", item 4): the structure of an image, its sections and tags, its
validation and its bounds, written into `docs/spec/image-format.md` with
their rules named by the tests that check them; a body's own encoding and
the rules the decoder checks on it are the bytecode chapter's already.
The first three batches, the book with the syntax, the semantics with
the run-time semantics, and the bytecode, are built. The batches after it are that document's "The order of
building it"; the complete install resumes after the fifth.

**The debugger's stage 6: hot swap, then edit-and-continue of running
activations, and `ActivationMirror` for programs** (decided with the
user on 2026-11-11: the debugger goes on before road step 9 resumes;
[debugger.md](design/debugger.md), "Planned: the order of building
it"). Hot swap installs a saved module's changed declarations through
the MirrorBuilder's atomic install and sets the breakpoints again on the
new bodies; edit-and-continue recompiles an edited method and moves its
running activations onto the new code at a statement boundary, its
values moved by name through both bodies' variable tables, a running
activation divorced into a larger frame object through the bulk
forward; and `ActivationMirror` in `MirrorsFactory.ns` gives a program
the access to activations §8.2 asks of mirrors, withholding a shipped
activation's values ("Hot swap and edit-and-continue", decision 1,
"Evaluation never reaches the VM mirror"; the item "The activation and
thread mirrors" below). With the install, "Create Method" for an
uncaught `MessageNotUnderstood` and the forced `nil` from
`Object>>doesNotUnderstand:` when it is declined ("Exceptions"). Over
DAP, requests of ns-next's own, and in the plugin a hot-swap action.
Hot swap is split into three batches, the complete install, hot swap in
the debugger and developing in an empty world, and the first one's
decisions are taken but for one, the change of a module's identity
([liveness-and-reflection.md](design/liveness-and-reflection.md),
"Planned: the complete install"); it waits for the specification of the
current state, which comes first (decided with the user on 2026-11-23).
Stages 1 to 5 are built (2026-11-11 to 2026-11-17; "The debugging API
at the safepoint", "Launching, the start and the end", "The MCP
server", "The JetBrains plugin"): stage 5 evaluates expressions, sets
variables, decides conditions and logpoints, restarts frames and forces
returns, smart-steps into a chosen send, marks objects, shows the memory
view and folds the shipped frames, in the VM, the DAP agent and the
plugin, and `ns-next mcp` lets an agent debug a program, several
numbered sessions served at once, and read a traced session's trace
file through four tools of its own (2026-11-22; "The MCP server",
decisions 18 and 19). It needs its questions decided and a plan of its
own.

**Road step 9, an optimizing JIT with type feedback, inlining and
deoptimization**, Strongtalk's and HotSpot's class: hot code recompiled
along the receiver types the inline caches recorded, built to the
deoptimization contract road step 7c wrote and on the feedback, frames,
assembler and code zone road step 8 left for it
([execution.md](design/execution.md), "Planned: JIT tiers", "Planned:
the deoptimization contract"; the entry in "The road to a
high-performance VM" below). Designed with the user on 2026-11-04 and
2026-11-05 in fourteen decisions ([execution.md](design/execution.md),
"Planned: the optimizing JIT"): one tier, an SSA control-flow graph in
Maglev's shape, the repository's own backend, a compiler thread fed a
snapshot, customization per receiver application, traps by default,
inlining planned by frequency, unboxed numbers, frame objects for
inlined activations, partial escape analysis and invalidation per
application, in ten stages. Its first stage, the foundation without
speculation, is built (2026-11-06; "The optimizing tier's foundation"):
a budget per body that baseline code spends, a snapshot compiled at the
safepoint into an SSA graph, a linear scan that keeps values in machine
registers between interrupt points and writes them back before each,
the code emitted through the baseline's templates and installed beside
its baseline code, abandonment, and the optimize and deopt stress
modes; measured -1.45% on the geometric mean against `master`
("What the optimizing tier's foundation moved"). Its second stage,
speculation, is built (2026-11-06 and 2026-11-07; "Speculation"): a
method body's copies per receiver application and entered application,
reached by receiver-guarded cells, the push and a dispatching entry for
`super`, private lexical and interpreted sends; own slots at fixed
offsets; guards that trap at arithmetic, conditionals and sites never
executed; versions against deoptimization loops, an abandonment one
too, the last compiled without traps; the invalidation of copies by an
edit, a bulk forward and a reclaimed id, and `NS_NEXT_STRESS_INVALIDATE`;
measured -3.17% on the geometric mean against `master` ("What
speculation moved"). Its third stage, inlining, is built (2026-11-07
to 2026-11-09; "Inlining"): a plan over a tree of snapshots of the body
and its callees, by frequency under limits measured; type-cases of up
to four inlined callees in the cache's order; accessors, quick methods,
`==`, `class`, `SmallInteger` operations and indexed access as nodes;
every inlined scope at the interpreter's position in the physical
frame; blocks compiled with their home and eliminated where nothing
lets them escape, made by the exit that finds them live; the
recompilee found by walking up the stack; a frame that left at a poll
gone back into its optimized code there; and a copy freed when anything
it inlined changes; measured -34.0% on the geometric mean against
`master` ("What inlining moved"). Beside it, `Float` in machine code
is built (2026-11-09; "Floats in machine code"): `Float`'s arithmetic,
orderings, `=`, `sqrt`, sign, class tests, roundings and `asInteger`
and a `SmallInteger`'s mixed rows computed bit for bit as their leaves
compute them, through row stubs in baseline code and as tagged nodes in
optimized code, every x86_64 binary built for x86-64-v2; measured
-10.1% on the geometric mean against `master`, Mandelbrot 75% and NBody
72% faster ("What floats in machine code moved").

When the work returns to road step 9, its next stage is the **fourth, inlined frame objects
and the non-local return into an inlined home**, with `NS_NEXT_STRESS_INLINED_FRAMES`
("Planned: the optimizing JIT", "Closures, frames and non-local return
in inlined scopes", "Stages"), which lifts the plan's rule that sends
every callee whose scope would create a block that escapes; it gets its
design questions and its plan when it comes up.
Beside them stand what the first three stages left (the items under
step 9) and what the baseline's stages left to the steps after them
(the items under step 8).

Road step 8 is complete (2026-11-04): the baseline JIT, Cog's class,
in six stages ("The baseline JIT's foundation", "Sends, returns and the
exit that materializes", "The entry at a send", "Stage 4: closures,
non-local return, allocation and the barrier", "The code zone", "When a
body is compiled", "Polls and the depth check", "Stages"): `ns-memory`'s
doubly mapped code zone and the entry into machine code, `ns-assembler`
with the x64 and AArch64 encoders, and in `jit/` the
macro assembler of the VM's idioms and its register roles, the stubs and
the baseline compiler's one exhaustive `match`; a cached send calls
through a machine-readable cell beside its cache entry into the callee's
machine code or a stub, the callee returns by `ret`, the miss leaf
compiles a callee from inside machine code, and an exit at any depth
materializes every compiled frame; an interpreter's send calls a
compiled callee's machine code through a call stub, and every compiled
body is entered wherever it runs. Stage 4 put the rest in machine code:
eden's bump and the write barrier inline, the own slots, instantiation
and the initializers as compiled calls, `MakeClosure` and a closure's
activation as a compiled call through the closure stubs, the outer
locals, the futures and their forcing by `resolve`, and the cut of a
non-local return; every body compiles lean, its frame reified only in a
twin compiled on demand. And it made everything that grows reclaimable:
the code zone is compacted and frees the code of dead bodies, the exit
budget decays and an abandoned body is revived, a program-made symbol
takes no number and dies with its object, every table beside the object
space has a growth limit, the site ranges slide at a full collection,
and committed memory is given back by a band and a damping. Measured
with three builds a side, the geometric mean over twenty-three sources
is 54.6% faster than `master` and 5.7% faster than stage 4's tree before
the reclamation ("What reclamation moved"); `bench_megamorphic` (+2.8%),
`bench_recursion` (+0.7%) and, against that tree, `bench_signal` (+0.9%)
were slower, each item below. Stage 5 brought back into machine code
the frames that ran interpreted to their return: an interpreted
backward jump switches its frame into the body's machine code at the
loop's head, the body compiled at its 40th such jump if it has none
(`jit::back_edge_ready`, `jit::at_back_edge`), a frame that left its
machine code for a poll's safepoint goes back into it once the
safepoint is served, a compiled push makes one compare for the depth
and a safepoint request, and the depth count, measured in `rbp`, stays
in its machine word. Measured with three builds a side, the geometric
mean over the twenty-three sources is 8.6% faster than `master`,
stage 4's tree,
`bench_megamorphic` 85.1%, `bench_new` 38.3%, Havlak 2.9% and DeltaBlue
4.5%, Queens' +2.6% the code's placement ("What on-stack replacement
moved"). Stage 6 is all of it on AArch64, on Linux and Apple's
platforms, Windows on ARM staying interpreted: the AAPCS64 roles mapped
one to one from x64's, the return address in a 16-byte slot on `sp`
behind three operations of the call convention that encode on x64 as
nothing, `jmp` and `call`, and the suite run under `qemu-aarch64` in
Docker in the gate group `aarch64`, beside `linux`, both at the end of a
batch ("Fixed registers and the calling convention", "The native stack
carries the return addresses", "Tests"); AArch64 is unmeasured, since
QEMU times nothing. Road step 8 as a whole, Cog's class, for x64 and
arm64, is "The road to a high-performance VM" below.
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
master (`4c6fe489`) and task by task: every program within noise, but
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
layout inside `vm::run` as that section says, and the current tree's
three lay the code zone out at three paddings as "What reclamation moved"
says. Seven of the eleven measurement sources are
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

## The specification

- **The grammar as a machine-readable block** (2026-11-23). Decision 5 of
  [specification.md](design/specification.md) has the grammar, the
  instruction table and the tag table stand in blocks tests hold to the
  code; the lexical structure's three tables do, but the PEG of
  `docs/spec/language/grammar.md` stands in `text` blocks nothing reads,
  so a production and the parser can drift apart unnoticed. What a test
  would check against it -- the parser's productions, or a parser
  generated from it run over the corpus -- is undecided.

## The road to a high-performance VM

Decided with the user on 2026-09-29, in this order: nine numbered steps,
with the compact instruction encoding after road step 2 and real Newspeak bodies
for the control-flow loops between steps 7 and 8. Steps 1 to 6 give a very good
interpreter on a modern object representation; road step 8 reaches Cog's class,
road step 9 Strongtalk's and HotSpot's, measured against Java with the Are We
Fast Yet benchmarks (Newspeak versions in `prior-art/somns/core-lib/Benchmarks`,
Java counterparts in `benchmarks/are-we-fast-yet/java/`, Python ones in
`benchmarks/are-we-fast-yet/python/`) and `prior-art/newspeak-benchmarks`.

- **Step 1, the memory layer -- built** (`crates/ns-memory`;
  [memory.md](design/memory.md), "The memory layer").
- **Step 2, first plan, large integers -- built**: `LargeInteger` for
  §5.1.1's unbounded integers, adapted from `num-bigint` and `num-integer`.
  [core-library.md](design/core-library.md), "Large integers".
- **Before step 2's second plan, the Are We Fast Yet benchmarks** (decided
  with the user on 2026-09-29, so that every later step is measured on real
  programs rather than only on the microbenchmarks under `benchmarks/micro/`).
  The Newspeak versions in `prior-art/somns/core-lib/Benchmarks` come into
  `benchmarks/are-we-fast-yet/newspeak/`, each a module of the harness's project,
  verified under both lowerings against `benchmarks/are-we-fast-yet/newspeak.toml` and measured as
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
  three pinned release builds of `7afe9a6`, pooled, as that document's
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
- **Step 8, a baseline JIT -- built**, Cog's class, for x64 and arm64, to the
  contract of 7c, designed ([execution.md](design/execution.md),
  "Planned: the baseline JIT"), its first two stages built (2026-10-27,
  [their plan](superpowers/plans/2026-10-27-2047-baseline-jit-foundation.md);
  "The baseline JIT's foundation"), its third, sends and the caches in
  machine code, built (2026-10-29,
  [its plan](superpowers/plans/2026-10-28-1928-jit-sends.md); "Sends,
  returns and the exit that materializes"), the entry into machine code
  at its cost built (2026-10-29,
  [its plan](superpowers/plans/2026-10-29-2030-jit-entry-cost.md); "The
  entry at a send"), its fourth, closures, non-local return, allocation
  and the write barrier in machine code with the reclamation of
  everything that grows, built (2026-11-01,
  [its plan](superpowers/plans/2026-10-30-0103-jit-closures.md); "Stage
  4: closures, non-local return, allocation and the barrier", "The code
  zone"), its fifth, on-stack replacement at an interpreted backward
  jump, the return into machine code after a poll's safepoint and the
  one compare at a compiled push, built (2026-11-03,
  [its plan](superpowers/plans/2026-11-02-2051-jit-osr.md); "When a body
  is compiled", "Polls and the depth check"), 8.6% faster than `master`
  on the geometric mean over twenty-three sources and no program slower
  but Queens, by the code's placement, the depth count left in its
  machine word after `rbp` measured no faster ("What on-stack
  replacement moved"), and its sixth, the AArch64 encoder, built
  (2026-11-04, [its plan](superpowers/plans/2026-11-03-2047-jit-aarch64.md);
  "Fixed registers and the calling convention", "The native stack carries
  the return addresses", "Tests"), the JIT compiled for AArch64 on Linux
  and Apple's platforms and its suite passing under QEMU, x64's machine
  code byte for byte unchanged and AArch64's unmeasured.
  Left to it, and measured: whether a send calls through its cache cell
  or through a patched direct call, which stage 3's measurement could not isolate
  ("What sends in machine code moved"); and the code zone's default size,
  16 MiB today (`jit::DEFAULT_ZONE_BYTES`), a first guess, which no
  measured program fills: only CD compacts it, six times in a run, all of
  it churn ("A nested class's accessor on a private or `super` chain
  exits machine code" below). What the stages so far leave to the ones
  after them (noted in their reviews and measurements, 2026-10-27 to
  2026-11-01):
  - **Without allocation, a rarely exiting body still spends a bounded
    share interpreted.** In a run that allocates nothing the exit counts
    never decay, and an abandoned body is revived only by its interpreted
    entries: 4,096 of them, doubling at each further abandonment up to
    65,536 (`jit::REVIVAL_ENTRIES`, `jit::MAXIMUM_REVIVAL_DOUBLINGS`;
    [execution.md](design/execution.md), "The baseline JIT's
    foundation"). A body that exits once in N calls so runs 64·N calls in
    machine code between waits of at most 65,536 interpreted calls: an
    interpreted share below 65,536 / (65,536 + 64·N), about half for an
    exit in a thousand calls and a tenth for one in ten thousand, where
    an always-exiting body should be interpreted. Telling the two apart
    needs the count of the body's calls in machine code, which the hot
    path does not keep; whether to count them, or to pace the decay by
    something other than allocation (polls, entries), is open. Stage 4's
    measurement found no program that needs it: of the twenty-three
    sources only CD and `bench_signal` abandon a body at all, and both
    allocate ([execution.md](design/execution.md), "What reclamation
    moved").
  - **A block the interpreter pushes above its sender's window pays the
    bridge** (accepted with the user on 2026-10-29 until stage 4, which
    left these pushes to it): an
    interpreted `value` send and its kin call a block's machine code
    through the closure call stub, as a method's (`jit::called_closure`),
    once a push of the block has written its machine entry row,
    and every compiled send that activates a block calls it through a
    closure stub, but the interpreter's own pushes of a block -- for
    `ifTrue:` and its relatives sent a block that is no literal,
    `ifNotNil:` and its relatives, a mirror row's failure block, a
    future's computation -- are entered at the push, an entry that costs
    about 8.5 ns more than interpreting a one-instruction block -- the
    switch, `jit::run_machine_code`, `jit::entered`, the entry stub saving
    and restoring eight registers ([execution.md](design/execution.md),
    "What the entry at a send moved"). Of the bridge
    itself, `run_machine_code` still clones two `Rc`s at every
    entry, the zone's and the entry block's; and `vm::run_stretch`'s
    `Transfer::Stopped` arm repeats the five reloads of its `Switched`
    arm, left as it is since a change there costs a codegen round for
    nothing measured.
  - **A call refused at the depth bound is counted nowhere.** A call the
    callee's own prologue refuses -- at the depth limit's bound, which
    the machine words' `DepthLimit` keeps at L while a
    `doesNotUnderstand:` handler holds the reserve -- answers
    `Called::Refused`, and the interpreter pushes the callee; no exit
    budget counts it, only the statistics' `exits` and `pushes_refused`
    (`jit::called`). So a
    recursion that stays at the bound pays a round trip through the call
    stub on every send that no budget stops: correct, only slow
    ([execution.md](design/execution.md), "The entry at a send", "A
    refusal is the push's"). Whether such a refusal should spend the
    callee's exit budget, or the bound be read before the call, is open.
  - **A leaf call costs more than the interpreter's step for the same
    target**: the leaf stub's leaf decides the send again
    (`inline_cache::act_at`) for every target but a slot accessor its send
    hit on its site's own cell directly -- a builtin class's accessor, a
    slot accessor whose field lies past the stubs `jit::stub_entry`
    assembles -- writing a frame for a sender machine code pushed, and
    the mirror method leaf finds its send's selector and argument count by
    a hash lookup of the call's return address (`JitState::call_site`).
    Reading the selector and argument count where the leaf can reach them
    without a lookup would remove most of what remains. Such a call costs
    about 3.4 ns (measured 2026-10-31, when 83 million of CD's 85 million
    leaf calls a run were `Float`'s); `Float`'s operations no longer make
    it, run by row stubs and nodes instead ("Floats in machine code" in
    [execution.md](design/execution.md)), and of what still does,
    `Integer`'s bit operations are the hottest left ("Templates for
    `Integer`'s bit operations" under "Planned: performance and
    representation").
  - **A nested class's accessor on a private or `super` chain exits
    machine code**, as does one whose hidden slot lies past the
    `NESTED_CLASS_STUBS` slots or holds no class yet: only a public or
    protected chain's cell compares the receiver's own guard, which the
    stub's hidden slot needs (`jit::stub_entry`). A private one would
    take the guarded accessors' recorded-guard compare. It costs CD 3%:
    a class's methods name its sibling classes by implicit-receiver
    sends (`Vector2D x:y:` in `plus:` and `minus:`, `Node`,
    `RedBlackTree`, `InsertResult`, `Vector3D`), each a refused call
    and an exit, so the bodies that make them spend their budgets in 64
    calls, their callers' refused calls abandon those in turn, and every
    revival -- by the decay, at every second scavenge -- compiles them
    again only for them to be abandoned again: 13,331 abandonments and
    as many revivals in a run of four sends, the exits nearly all such
    refused calls. A build that never revives an abandoned body ran CD
    3.0% faster (2,980 ms against 3,073, one build a side, three
    rounds, measured 2026-11-01); the guarded nested-class stub removes
    the cause, where suppressing revival would end a rarely exiting
    body's run interpreted again.
  - **Every initializer invocation calls a leaf.** `Leaf::Initializer`
    decides the invocation afresh each time (`jit::sends::initialize`),
    so an instantiation calls it once per level that runs code, and once
    more for the instance, where a send's cell hits without one; a class
    whose levels run code pays a leaf call per level. Whether the
    decision should be cached per invocation -- the superclass level and
    its body depend on the frame's class only -- is to be measured.
  - **`jit::entered` is `#[cold]`**, though every push of a compiled
    body that is no send -- a block the interpreter pushes itself among
    them -- enters through it, and so does the push that revives an
    abandoned body; since stage 4 made a closure's activation a compiled
    call, fewer pushes reach it, and its placement is to be measured
    again.
  - **The cut exits at every unwind marker, `on:do:`'s too, and calls a
    leaf per frame it passes** (`jit/cut.rs`; [execution.md](design/execution.md),
    "Stage 4: closures, non-local return, allocation and the barrier").
    An `on:do:` activation runs nothing when an unwind pops it -- the
    unwinder answers a block only for `ensure:` and `ifCurtailed:` -- so
    the cut could pop it as it pops any frame, and an `ensure:` or
    `ifCurtailed:` whose block has run (`complete` set) likewise; a `^`
    out of a protected block now exits instead. And Rust reads no native
    stack, so the walk to the home costs a leaf call per frame above it:
    whether a `^` from a block several calls deep below its home is worth
    a walk in machine code that hands the frames over at once is to be
    measured with the stage.
  - **A cell of a body an exhausted zone holds calls the miss leaf on
    every hit.** A compaction points the cells of every body it frees at
    the miss stub (`jit/zone.rs`), which is right while the body may
    compile at its next due entry. While the zone is exhausted and the
    body does not fit the room left (`JitState::may_compile_now`), the
    miss leaf decides the send, compiles nothing, refuses the call and
    writes the miss stub back, so every hit costs a leaf call and an
    exit until a retry ends the exhaustion -- a wait the doubling of the
    retries lets grow with the run ([execution.md](design/execution.md),
    "The code zone"). A stub of its own for such a cell, which refuses
    the call at once without the leaf and which the end of the exhaustion
    rewrites to the miss stub, would save the leaf call per hit; it is
    not built, because those hits are held compiles that pay towards the
    retry, a stub would need its own way to count them, and an exhausted
    zone needs a body that does not fit in a zone smaller than any
    measured program's (the default zone holds 16 MiB). Worth building
    if a measurement shows a run that stays exhausted.
  - **A body shared by two interpreters would ping-pong between their
    zones.** A body's `JitSlot` names one zone, and `jit::note_entry`
    compiles a body whose slot names another into the running
    interpreter's, so two interpreters running one `CompiledCode` in turn
    would compile it again at every switch, each compile leaving its old
    code in the other's zone until that zone is compacted. No
    body is run by two interpreters today; if code comes to be shared,
    the slot holds code per zone or the stale copy is reclaimed.
  - **`load_frame_register` has no unchecked variant.** Every frame
    register a template loads is checked against the dead-register poison
    in a debug build, as the interpreter checks a read; a send's argument
    placement reads only registers the interpreter reads, so stage 3
    needed none, but a copy the interpreter does not treat as a read
    would.
  - **The idioms' scratch register is guarded by a `debug_assert!` alone**
    (`r11`, `x9` under AAPCS64, `load_frame_register`): a template that kept a value in it
    across an idiom would be found only by a debug build that reaches it.
  - **A panic inside a leaf aborts the process**, by design, since nothing
    unwinds through machine code; a debug build's dead-register read
    inside a leaf therefore ends the whole test run rather than failing
    one test. A leaf that is to report such a defect returns it as a
    value, as a template's read does (`ExitKind::DeadRegister`).
  - **Paths no gate runs** (2026-11-04): the gate groups `linux` and
    `aarch64` run the suite on Linux x64, with the benchmark programs,
    and, under QEMU, on Linux AArch64, at the end of a batch, but
    neither runs `images` or `caches` there, so those run on Windows x64
    alone, and AArch64 runs no benchmark program; the stress modes run
    on Linux x64 and macOS on Apple silicon only nightly
    (`.github/workflows/stress.yml`), never on Linux AArch64. The code
    zone's BSD paths are checked by clippy alone (`memory`) and never
    ran; Apple's JIT runs on GitHub Actions' macOS runners, but QEMU
    models no incoherent instruction cache, so a missing flush on Linux
    AArch64 would pass the `aarch64` group.
  - **AArch64's speed is unmeasured** (2026-11-04). QEMU checks
    correctness only and times nothing, so whether the port's code is
    fast -- the 16-byte return-address slot, the `movz`/`movk`
    constants without a literal pool, the `call_unentered` sequence at
    every entry at a push, a conditional branch's one `b.cond` -- and
    whether the codegen budgets x64 is held to have an AArch64 analogue,
    needs real hardware to measure on, which the project does not have
    ([execution.md](design/execution.md), "Stages").
  - **Windows on ARM runs interpreted** (2026-11-04). `build.rs` sets
    `cfg(ns_jit)` for AArch64 on Linux and Apple's platforms only. Much
    of what the JIT needs there exists: the encoder never uses `x18`,
    the register Windows reserves, so `Roles::of_abi(Abi::Aapcs64)` would
    serve, and `ns-memory`'s Windows code zone and native-stack bounds
    exist and build for that target. What is open is whether Windows on ARM's
    convention and unwinding differ from AAPCS64 in anything a stub
    crosses -- the entry, the leaf shim, the stack probes and the
    unwinder that finds machine code on the stack -- and a machine to
    run it on: no gate builds the JIT for `aarch64-pc-windows-msvc`
    (`release-checks` lints it without one), and nothing of it has ever
    run ([execution.md](design/execution.md), "The baseline JIT's
    foundation").
  - **Small leftovers**: `ns-memory`'s `map_code` is an `unsafe fn` though
    its safety section asks nothing of its caller; the leaf shim could be
    `unsafe extern "C"`; `crates/ns-assembler/tests/encoding_sources.rs`
    reads a raw string as an ordinary one.
  - **What stage 4's reviews and measurement left** (2026-10-30 to
    2026-11-01), none of them a wrong outcome:
    - *A rarely taken path costs an exit*: an object whose header's guard
      is `DispatchId::NONE` -- its application past `DispatchId::LIMIT` --
      exits at every own-slot access; a closure stub declines to copy a
      block's arguments past the ordinary end, so the interpreter's push
      grows the stack, where the growth leaf could make the room; a block
      whose code id lies past the machine entry table's 2^20 rows is
      always called by the interpreter; a future's clause end, which
      exits for a future among a leaf's operands ("A future as a leaf's
      operand leaves machine code" below), is unmeasured.
    - *The decay and the revival walk whole tables*: every scavenge
      halves the exit count of every installed body
      (`JitState::decay_exits`), and every revival writes every cell
      that names the refusing stub afresh by a walk of every site, block
      and global row (`InlineCaches::refresh_all_cells`), at most once
      per scavenge or compile trigger. Neither is measured on its own;
      the final measurement shows no program slower for them.
    - *Statistics*: `JitStatistics::send_exits` does not count a call
      refused at an interpreter's send, which only `exits` counts; and
      `ns-next bench --collector-statistics` prints neither the JIT's
      statistics nor `CollectorStatistics.decommits`, so the final
      measurement read them from a binary with a temporary print.
    - *Tests*: the private and `super` accessor stubs reached through a
      polymorphic block's cell or a global row -- the row's
      `ROW_CHAIN_START` word, the shift by sixteen -- are reached by no
      test, a defect there costing speed only; the barrier's tests and
      most long-run tests of the reclamation (`tests/collector.rs` whole,
      the million program-made symbols, the churn of families, the rare
      exit with allocation) compile only with debug assertions, so the
      `release-checks` build runs few of them; the remembered set's limit
      request that machine code's barrier makes through `Heap::remember`
      is reached by no test of its own; the instantiation test of the
      refusals the inline bump does not make
      (`instantiations_the_inline_bump_does_not_make_answer_as_interpreted`
      in `tests/jit.rs`) holds their outcomes to the interpreter's but
      not that the refusing send ran in machine code; and the class-format
      check of `MacroAssembler::load_instance_header` is reached by no
      program, since a factory body's receiver is always a class object.
    - *Code that says less than it could*: `Heap::machine_word_count_bits`
      builds a header of guard `DispatchId::NONE` and format
      `FixedPointers` and relies on both encoding as 0, where an accessor
      of `heap/header.rs` for a word count's bits would state it.
    - *Wasted writes*: a compaction fills the room it frees with stops
      and a site slide writes vacant cells over ranges that the shrink
      after the collection then gives back (`jit/zone.rs`,
      `InlineCaches::vacate_sites`), dirtying pages just before they go;
      a cost only, worth changing if a measurement shows it.
  - **What stage 3's reviews left** (2026-10-28 and 2026-10-29), none of
    them a wrong outcome:
    - *The architecture scan's blind spots*: `tests/jit_architecture_split.rs`
      sees an `x64` path or `use`, not a `Register(n)` built directly, a
      register re-exported from `macro_assembler.rs`, or an alias declared
      outside `jit/`.
    - *The compiled bit outlives its meaning*: a frame resumed in machine
      code that exits at its own instruction, or a switch that returns for
      a requested safepoint, keeps the bit, so the frame re-enters machine
      code at its next send's return; valid, since a baseline frame is the
      interpreter's at every boundary, but `FrameFlags::compiled`'s
      documentation says "stopped in machine code" (`heap/frames.rs`).
    - *The miss leaf counts an entry the interpreter counts again* when it
      refuses a callee without code, so a body first reached from machine
      code is compiled at its first real entry.
    - *Verification is loose in two places*: `verify_cells` accepts the miss
      stub's address for any expected entry, not only for a method that may
      be compiled; `verify_application_ids`'s check past the bound is
      vacuous; and no test pins why `VACANT_GUARD` is all ones.
    - *The polymorphic stub jumps to any entry but its own* on a
      `VACANT_GUARD` cell; only the megamorphic stub's can be there today.
    - *A non-private lexical site computes N on every guard mismatch*,
      though only the megamorphic probe needs it, and the miss block takes
      the receiver by value (a possible store-forwarding stall); neither is
      measured. `vm::run` gained 48 instructions in its miss blocks.
    - *Statistics*: a stub's hit is not counted in `compiled_calls`, and
      `compiled_calls` counts calls a prologue then refused.
    - *A switch gives up one resumption* just after a served collection,
      since `Heap::real_safepoint_requested` still answers `true` before
      the request is cleared.
    - *The depth words rest on convention*: `Mutator::depth_reserve` is
      still `pub(crate)`, so that every change goes through
      `set_depth_reserve`, which writes `DepthLimit`, is a rule no type
      holds; the immediates' arrays are typed `[_; 5]` beside
      `IMMEDIATE_COUNT`; and `an_immediate_class_word_survives_a_compaction`
      says it covers the move stress mode's move, which it does not run.
    - *Tests*: `send_recursion_overflow.ns` pins the overflow depth to the
      nearest thousand; no test pins that an uncached send, a
      declaration's own lexical send or an exit row's `SendMirror` does not
      count as a transfer; no JIT test pins that a requested safepoint is
      taken at a compiled push and return; the miss leaf's compile on the
      native stack within `NATIVE_STACK_NEED`'s 64 KiB leaf allowance is
      not measured at depth in a debug build.
    - *Wording*: the five object rows of `builtins/primitive.rs` share one
      comment, part of it false for each; `application_ids.rs`'s rationale
      for the table's bound is imprecise; `jit::baseline::lower`'s
      documentation does not name `Lowering::Transfer`.
    - *The stress verification's cost*: a debug build checks every cell
      against its entry before every `NS_NEXT_STRESS_SCAVENGE` scavenge
      (`InlineCaches::verify_cells`), about 1.2 µs a check, which made the
      slowest golden under that mode 32% slower (34 s to 45 s,
      `runtime/jit/counting_loop` under the reference lowering); pacing it
      by the cells checked, as `StressSchedule` paces collections by the
      bytes traced, would cut the stress gate's time if it comes to
      matter.
    - *Trivia*: `GlobalCache::group` is `pub(crate)` for one test, where a
      `#[cfg(test)]` accessor would keep it private;
      `goldens/runtime/jit/mixin_applied_twice.ns` covers the
      implicit-receiver form of an outer send but not the `outer`
      keyword's lowering; `ns-memory`'s Windows stack bounds read a failed
      `SetThreadStackGuarantee` as a guarantee of 0 rather than as no
      answer (`platform/windows.rs`); `the_ordinary_end_follows_growth`
      does not say why three distinct ends prove that the end follows a
      growth.
    - *Smaller costs*: the cells' vacate helpers build a temporary `Vec`;
      `describe_activations` runs on every unhandled error for a test
      hook; `emit_group` uses `multiply_checked` as a plain multiply,
      four instructions on AArch64; the BSD stack-bounds path is built by
      no gate; the global mirror reserves 64 MiB of address space for
      2^20 rows where 1,024 is the default.
- **Step 9, an optimizing JIT** with type feedback, inlining and
  deoptimization, Strongtalk's and HotSpot's class, designed with the user
  on 2026-11-04 and 2026-11-05 ([execution.md](design/execution.md),
  "Planned: the optimizing JIT"), in ten stages, the first three built:
  the foundation (2026-11-06; "The optimizing tier's foundation");
  speculation with customization, traps and invalidation (2026-11-06
  and 2026-11-07; "Speculation"); inlining (2026-11-07 to 2026-11-09;
  "Inlining"); then frame objects for inlined activations, next;
  numbers, with the raw area; on-stack replacement into optimized code;
  counting at polymorphic and megamorphic sites; the compiler thread;
  escape analysis with late inlining; range analysis. The first stage
  measured the budget (240,000 instructions, a sixth of it after a
  cache change) and its spend in baseline code ("What the optimizing
  tier's foundation moved"), the second the trap limit (5) and the last
  version (3), both kept ("What speculation moved"), the third the
  inlining plan's limits, `BLOCK_LIMIT` raised to 140 and the others
  kept, and the walk's least home calls, 4 ("What inlining moved").
  Left by the design to measurement, each in the stage that builds it:
  the share of polymorphic and megamorphic sends in the benchmarks
  before their counting is built, and the counting window's size and
  rhythm. What the first stage left:
  - **Register allocation over a body** (decided with the user on
    2026-11-03): a frame register's value kept in a machine register
    across instructions and merges is built in the first stage, a
    linear scan between interrupt points ("The optimizing tier's
    foundation", "The allocation and the write-back"), its registers
    `Roles::allocatable`, `r13` and `x22` among them. Open: an
    immediate literal taken as an operand, and a `SmallInteger`
    comparison fused with the branch that tests it, which runs at most
    0.08 times per instruction. Only a store a body provably needs no
    longer may be left out, and only once the collector's pre-pass nils
    what the frame did not write, or the leftover analysis is broken;
    the first stage leaves none out. [execution.md](design/execution.md),
    "The register cache", "What a register cache would move".
  - **The stores and reloads at a fast path's merge** (2026-11-05). An
    `IntegerBinary`'s fast path and its fallback send meet in a merge
    whose phis the allocator keeps in registers where the fast way first
    reaches it, so the fallback way writes back every dirty binding
    before its send and reloads after it, and a binding dead and
    untainted at a merge is stored on every pass through its way in,
    as frame consistency at every interrupt point requires. Two rules
    took most of it away: a float-fed site is sent alone
    (`Snapshot::fallback_taken`), and a natural loop's head leaves a
    register its frame word where every way in brings it so. The
    fallback-merge phis remain, on the sites that saw `SmallInteger`s
    and something else; a phi of the optimizer's own for a dead dirty
    register, or the merge's state chosen from the way the counts of
    stage 7 say is taken, would remove them.
    [execution.md](design/execution.md), "The optimizing tier's
    foundation", "What the optimizing tier's foundation moved".
  - **What the first stage's reviews left** (2026-11-05), none a wrong
    answer, each in `crates/ns-runtime/src/jit/`:
    - *Costs*: `sends::call_slots`' path for an instruction that is no
      cached send decodes and clones it, on the exit's, the cut's and
      the materialization's paths only; a frame read valid across a
      forward merge with no frame node between is dropped and read again
      (`optimizing/build.rs`), extra loads only; the bindings kept per
      block for the allocation's verifier are unmeasured in memory.
    - *Checks a debug build lacks*: the back edges' cold polls are
      emitted under a stale hold set rather than each under its own
      (`optimizing/emit.rs`, `emit_polls`); a direct write through
      `assembler()` or a template's `Roles` between two frame nodes is
      documented, not guarded by `MacroAssembler::temporary`.
    - *Tests*: `SnapshotInstruction`'s fidelity tests would pass a slip
      made the same in `convert` and `unsnapshot`; a two-phi swap is
      tested on one shape only; nothing compares baseline code byte for
      byte with the code before the snapshot form, a probe that ran once
      over 14,106 bodies and was removed; `futures_exit` in optimized
      code has no test of its own; `a_deferred_optimization_keeps_its_back_off`
      runs the held path twice and never the full zone's install path
      its comment names; `a_full_collection_keeps_what_is_left_of_a_budget`
      runs in debug builds only, `collect_now` having no release
      counterpart.
    - *Statistics*: `optimized_bodies_resumed` counts the frames returned
      to of every body that has optimized code, not only of frames that
      ran it, and `deepest_forced_exit` counts the points since the last
      safepoint across all optimized code, not per activation.
  - **What the third stage's measurement left** (2026-11-08), none a
    wrong answer; [execution.md](design/execution.md), "What inlining
    moved":
    - *An inlined scope that would create a block is sent*: until
      optimized code creates blocks itself (stage 4), a `MakeClosure` in
      an inlined scope leaves the copy, so the plan sends every callee
      whose scope would create a block it does not eliminate on a way
      that ran (`plan::Planner::creates_a_block`). The rule is stage 4's
      to lift, and it costs eliminated blocks: a callee handed one that
      creates another is sent, and the block handed to it escapes -- the
      inlining corpus eliminates 6 blocks with the rule, 18 without it.
    - *A home with a loop of its own is never taken by the walk*: the
      walk goes up from a block to its home only when the home has been
      called `LEAST_HOME_CALLS` times since its budget was armed, which
      only a body without a backward jump can show (`jit::recompilable`,
      `called_repeatedly`; [execution.md](design/execution.md),
      "Inlining", "The recompilee walk"). A home with a loop of its own
      is refused even when called often, and is optimized at its own
      run-out instead: its budget row cannot tell its entries from its
      back edges; no cost was measured on the twenty-seven sources. A count of entries
      apart from the back edges' spend would let the walk take it, and
      on-stack replacement into optimized code (stage 6) would let a
      home called once be entered at its loop's head.
    - *NBody's rare slow run*: since NBody's `do:` block is eliminated,
      about one run in thirty-five takes 1.01 s or more against 0.93 to
      0.96 s; `71f65b83` had none in seventy. No slow run has been seen
      with its statistics: 68 runs with `--jit-statistics`, none of them
      slow, gave identical statistics, and the flag changes the code.
      Not found
      ([execution.md](design/execution.md), "What inlining moved").
      The steady-state comparison against Java (2026-11-09) showed no
      slow NBody run: its nine `bench` medians lay at 926 to 951 ms
      ([execution.md](design/execution.md), "Against Java").
    - *Mandelbrot's one slow median*: in the same steady-state
      comparison one of Mandelbrot's nine `bench` medians, on build c,
      was 1968 ms, two of its three runs slow,
      against 1781 to 1808 ms for the other eight. Not looked into.
    - *A block handed on through a local loses its bonus*: the snapshot
      tree weighs a callee against `METHOD_LIMIT` plus the block
      argument bonus only for an argument a `MakeClosure` wrote, or a
      parameter or copied value (`tree::block_registers`), while the
      plan counts every register that may hold a block of the compile,
      through moves. A block stored in a named temporary and then handed
      to `do:` -- `| add = [...] | collection do: add` -- leaves the tree
      recording a callee heavier than `METHOD_LIMIT` alone (`Array`'s
      `do:` weighs 47 where its loop never ran) as one no plan could
      inline (`CalleeFacts::Other`), so the plan never sees it and the
      block escapes into the send.
    - *`ValueOfParameter` and `CullParameter` stay sends*, and the
      "cheap" weight of a send counts their fast paths, the mirror's and
      a closure's `value` among them, at an accessor's 2 though they may
      run a whole block.
    - *Compile time*: a debug build's verification of every allocation
      takes a quarter to a third of its optimizing compiles; DeltaBlue's
      97 copies take 29 ms in a release build.
  - **What the second stage left** (2026-11-06), none a wrong answer;
    [execution.md](design/execution.md), "Speculation":
    - *For stage 6*: the budget
      leaf receives the frame register in place of the program counter,
      so stage 6's on-stack replacement into optimized code, which
      wanted the counter at the leaf, needs another way to learn it.
    - *A copy past `DISPATCHING_ROWS`*: a body's block holds eight
      rows, so a ninth key's copy stands in none, and a `super` or
      private lexical send of that key runs the baseline code, while
      an interpreted send reaches it through the key's lookup
      (`jit::called`) and a receiver-guarded cell names it directly.
    - *What is not speculated*: an inlined `whileTrue:` or `whileFalse:`
      condition's non-Boolean path, `SignalNonBoolean`, has no site, so
      it keeps its exit, an ordinary one counted against the exit budget.
    - *Approximations*: the budget is one per body, shared by its keys,
      so a re-arm for one key shortens another retired key's wait of 4
      to its version (`rearm_optimization_budget`); the trapped bit is
      per site, shared by every key of the body, so one key's trap
      gives every key's next copy that site's slow path.
    - *Costs*: a method body's keyed copies are searched linearly at a
      push and at a call past the rows (O(keys)); `Heap::machine_frame_receiver_and_current_class`
      repeats its neighbour's read of the frame words; and
      `intervals_knowing` runs three times per optimization, once in
      the allocation and twice in `verify_allocation`.
    - *Tests*: `a_caller_whose_callees_keep_requesting_optimization_is_not_abandoned`
      takes about 95 to 150 s under INVALIDATE and the pairs of ZONE with
      OPTIMIZE or DEOPT, and `counting_loop` 72 to 97 s under the ZONE
      pairs; `a_hot_loop_requests_its_body_s_optimization` skips its pin
      under INVALIDATE in both lowerings, where only the reference
      lowering re-requests; `a_compaction_remaps_the_dispatching_rows`
      cannot fail on its own, the check being `verify_dispatching_rows`
      after every compaction under the zone stress mode.
    - *Gates*: no gate runs the stress modes in pairs. ZONE with
      OPTIMIZE or DEOPT had failed on `master`, unnoticed, until this
      stage's runs of the pairs found it: a refused call to a callee a
      compaction freed abandoned its caller, the deopt countdown's
      rotation locked in step with a long activation, and the
      allocation's verification ran cubic in a body's size (`729b4eac`,
      `fbc56be0`, `168f4403`). A gate of the pairs that touch the
      optimizer would keep such failures found.
  - **What the third stage's callees always inlined leave** (2026-11-07),
    none a wrong answer; [execution.md](design/execution.md),
    "Inlining":
    - *Sent, not expressed*: a builtin class's accessor
      (`MethodFastPath::BuiltinClass`), whose class only the heap's
      tables name, which no simple node reads; `^<parameter> value` and
      `cull:`, which activate a block, whose scope a quick method has no
      way to lay out, so that the block escapes; and every arm with a check in a copy that speculates
      nothing or at a site that trapped (`build::Laying::expressed`).
    - *Traps that recur*: a lazy slot's getter traps for every instance
      whose slot is still empty, and `==` for every pair that holds a
      future; such a site is sent once its trap retired the copy.
    - *Costs*: the barrier of a setter and of an `Array`'s `at:put:`
      calls the remembering leaf out of line, so that every value live
      across either leaves the registers the call clobbers -- moved into
      a survivor or written back -- on the straight line too.
  - **An optimized caller is never recompiled to inline a callee that
    became hot later** (2026-11-08). The recompilee walk
    (`jit::find_recompilee`) stops at every caller whose key has a copy
    installed: optimized code spends no budget, so no row says how long
    a copy has run since its install, and `optimize` drops a request for
    a pair that has its copy. A caller optimized first keeps sending a
    callee that grew hot afterwards, which is then optimized alone.
    Strongtalk recompiles an old nmethod when its own counters fire
    (`recompiler/recompiler.cpp`, `shouldNotRecompileNMethod`,
    `code/nmethod.cpp`'s `isYoung`). The fix needs a spend measure in
    optimized code and a way to recompile a live copy: raise its version,
    compile, install in its place.
  - **A poll whose entry cannot restore a value keeps the full exit**
    (2026-11-08). A frame of optimized code that left at a poll goes back
    in at the poll's entry, which reads every value the code reads past
    the poll back from a frame word the leftover analysis leaves to the
    frame there, or from a constant's word (`optimizing::metadata`'s
    `poll_restores`). A value read past the poll that is bound only to
    registers the analysis taints at the loop's head, and no constant,
    has no such source: the compile gives that poll no entry, and every
    scavenge sends the frame on in baseline code until its next push
    (`JitStatistics::polls_without_an_entry`). Among `ns-runtime`'s
    tests only graphs built by hand compile one; a value the copy loads
    across the back edge without the interpreter holding it -- a receiver
    or a field load hoisted out of the loop -- would, and is rebuilt only
    once a rule shows that no store between the load and the poll wrote
    the field.
  - **What `Float` in machine code left** (2026-11-09), none a wrong
    answer; [execution.md](design/execution.md), "Floats in machine
    code":
    - *The trapped bit conflates two causes.* A site's trapped bit is
      set by a never-run site's trap as by a failed check, and a site
      that trapped is sent in the next copy (`build::Laying::expressed`):
      a `Float` site whose copy was optimized early -- before the site
      ran, at a budget spent inside a long loop -- traps once at its
      first run and stays a send through its row stub in every later
      copy, where the node would have run. A bit of its own for a
      never-run site's trap would keep the node.
    - *An `IntegerBinary` with a mixed operation answers no known
      kind*: an `Integer` arm whose row also takes a `Float` argument
      may answer a `Float`, so a later node tests its tag again where it
      tested none before. Measured level, Sieve +3.8% at forty
      iterations, not separated and without a verdict ("What floats in
      machine code moved"); a kind that says "a `SmallInteger` on the
      fast path's way" would remove the test, and the mixed operation
      can only decline where the right operand is known to be a
      `SmallInteger`, so it could be left out there.
  - **Scalar replacement of value objects** (noted with the user on
    2026-10-22): a value whose allocation the JIT sees can live in
    registers and never be allocated, as escape analysis does for any
    object, and a value need not even keep its identity when it escapes,
    since its `==` is its `=` and its `identityHash` its `hash`
    (`Kernel.ns`, `newspeak-spec.pdf` §3.1.1). `new` as compiled code is
    what makes it possible: allocation and initialization are code the JIT
    can inline. The partial escape analysis of the ninth stage does it;
    four questions are decided one by one when that stage begins (decided
    with the user on 2026-11-05): a value's lazy slot (§6.3.3),
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
- **Last of all, once everything above is done: machine-level
  experiments, each measured** (noted with the user on 2026-11-09, for
  future optimization). Small gains, none worth a step of its own
  before the structural ones above; each needs the encoder form, its
  `iced-x86` check, the AArch64 counterpart where one applies, and a
  measurement. The encoder today has `move_if` (`cmov`) and
  `multiply_checked`, none of these:
  - **`lea` for tagged arithmetic**: an addition with a constant, an
    index or an address computed into another register without
    touching the flags, a `mov` saved per operation (`SmallInteger`
    with a constant, `at:`).
  - **`setcc`/`cmov` for a Boolean result**: a comparison whose value
    is stored or answered rather than branched on made `true` or
    `false` without a jump; pays only where the comparison predicts
    badly. First check what the templates emit for it.
  - **Prefetch at allocation (`prefetchw`)**: the lines past the bump
    pointer fetched ahead, HotSpot's `AllocatePrefetch`; for
    allocation-heavy programs (CD, Storage, Richards); strongly
    CPU-dependent.
  - **`rep stosq` to initialize a large object**: an `Array new:` of
    about 1-2 KB and more filled with `nil` in one instruction, unrolled
    stores below.
  - **Loop heads aligned** to 32 or 64 bytes for the micro-op cache and
    the front end, HotSpot's `OptoLoopAlignment`; 0-3%, sometimes
    negative.
  - **`crc32` (SSE4.2) for the runtime's hash tables**, such as the
    mirror method leaf's lookup by return address. Every x86_64 build
    targets x86-64-v2, which has SSE4.2 ([execution.md](design/execution.md),
    "Floats in machine code"), so the Rust side needs no
    `#[target_feature]` and no runtime switch for it; AArch64's `crc32`
    instructions are optional before ARMv8.1, so the hash there needs
    its own answer.

Steps 7 to 9: [execution.md](design/execution.md), "Planned: JIT tiers".

## Planned: performance and representation

- **Templates for `Integer`'s bit operations** (decided with the user on
  2026-11-09 to be an item of its own). `<<`, `>>`, `bitAnd:`, `bitOr:`
  and `bitXor:` are mirror method leaf calls in machine code, about
  3.4 ns each; they are the act-leaf calls Mandelbrot still makes,
  1.27 million a run, `byteAccumulator << 1` once a pixel and `bitXor:`
  once a byte ("What floats in machine code moved"). A row stub, as the
  `Float` rows have ("Floats in machine code"), saves about 2.4 ns a
  call, and a node in optimized code the call itself. BMI2's `shlx` and
  `sarx`, which shift without the flags and without `cl`, and `lzcnt`,
  for the shift's overflow check, would save about 0.2 ns more, but lie
  beyond x86-64-v2: they need a detection at start, a choice in the
  macro assembler and a stress mode that forces the base forms, or no
  test on a machine with them runs the base forms -- the machinery the
  `Float` work decided not to build ("No extension form beyond v2").
  Whether they pay for it is open; the shifts' base forms are the
  template. [execution.md](design/execution.md), "Floats in machine
  code".
- **Whether x86-64-v2 pays on the Rust side** (2026-11-09). Every
  x86_64 build is compiled with `target-cpu=x86-64-v2`, decided for the
  JIT's `roundsd` ("Floats in machine code"), which also lets LLVM use
  SSE4.2, POPCNT and the rest in the interpreter, the collector and the
  leaves. The measurement of the `Float` work built both sides with the
  flag, so what it moves on its own is not known ("What floats in
  machine code moved", "Not covered"). What would find out: `master`'s
  shipped configuration against the same tree with the flag, three
  pinned builds a side, by the protocol of "Measuring". It is a
  requirement either way; the measurement says only whether the Rust
  side gains anything from it.
- **The float row stubs could share their tail** (2026-11-09). Each
  float row stub is about 455 bytes on x64, its own copy of the box and
  the return -- 30 stubs, about 14 KB of every zone, 0.1% of its
  16 MiB. A tail shared by every float stub, entered with the double in
  a float scratch register, would shrink them to the operation and its
  operand checks. [execution.md](design/execution.md), "Floats in
  machine code".
- **Further JIT techniques, options beyond the ten stages** (noted with
  the user on 2026-11-10 as options, none decided). The ten stages of
  "Planned: the optimizing JIT" already plan unboxed numbers, on-stack
  replacement into optimized code, counting at polymorphic sites, the
  compiler thread, partial escape analysis with late inlining and range
  analysis; a sea of nodes, a second optimizing tier now and Cranelift
  are rejected there, with reasons. Five families are planned nowhere,
  each to be weighed once the stages that precede it are built, each by
  a measurement on the programs of "Against Java and CPython":
  - **Loop optimizations** after stage 5: loop-invariant code motion,
    global value numbering, loop peeling and unrolling, strength
    reduction, and guards hoisted out of a loop -- C2's and Graal's
    standard set, which NBody, Mandelbrot and the array programs would
    meet first.
  - **A better register allocator**: a graph-colouring or SSA-based one
    (`regalloc2`, which "Planned: the optimizing JIT" keeps possible
    behind the same interface) if a measurement shows the linear scan
    limits the peak.
  - **Code in step with the collector**: a write barrier left out where
    the target is provably young, pretenuring by allocation site (V8's
    allocation-site feedback), and allocations folded into one bump --
    for CD and the allocating programs, once a profile shows where their
    time goes ("CD runs slower on ns-next than on CPython", below).
  - **Less warm-up**: machine code or profiles kept across runs, as V8's
    code cache, Java's class-data sharing and Leyden, or .NET's
    ReadyToRun do; an image ([image-format.md](design/image-format.md))
    could carry them.
  - **More speculation**: argument and return type profiles (HotSpot's),
    class-hierarchy assumptions invalidated by an edit, and splitting --
    duplicating a merge's tail rather than merging the types, Self's.
  Considered and not pursued: SIMD vectorization, which serves only
  loops over typed arrays, and a different architecture -- a tracing
  JIT (PyPy, LuaJIT) or partial evaluation of an interpreter (Truffle,
  on which SOMns runs) -- which would replace the method JIT in
  Strongtalk's and V8's line rather than extend it.
  [execution.md](design/execution.md), "Planned: the optimizing JIT",
  "Stages".
- **Exploiting immutable slots** (noted with the user on 2026-11-10 as an
  option, not decided). Nothing in the compiler or the JIT uses that a
  slot is immutable (`=`, §6.3.2) -- only the parser and the dispatch,
  which gives it no setter, know it; read-only locals have their
  counterpart, never copied protectively ("Fewer protective copies").
  After its initializer, an immutable slot is written only through a
  mirror or by `become:`'s bulk forward, which both already invalidate
  optimized copies ("Invalidation" in "Planned: the optimizing JIT").
  Six optimizations rest on that:
  1. **A load kept across sends.** A mutable slot is reloaded after every
     send, which may have written it; an immutable one need not be:
     optimized code keeps the value in a register across sends and
     interrupt points. Sound because `InitializeSlot` for an object runs
     only in that object's initializer frame, which cannot go on while
     the code reading the slot runs above it.
  2. **A load hoisted out of a loop**, which follows from 1 without an
     analysis: `bodies` in NBody, a vector's fields in CD.
  3. **A constant where the receiver is known** -- an inlined callee's
     constant receiver, a module instance. The module case keys on the
     values, not the instance: a module's immutable slots depend only on
     its factory's arguments, there being no global namespace, so every
     instance made with the same `platform` shares one copy in which
     `private List = platform collections List` and every other import
     and class reference is a literal, its `new` resolved and its factory
     inlined; an instance with other values gets its own copy, entered
     through one compare, as customization per application is
     ("Customization"). A nested class keys on the outer slots its code
     reads. HotSpot's trusted final fields fold the same for a constant
     object; in Newspeak every class reference is such a load.
  4. **A value speculated constant**: a site that has seen one value of
     an immutable slot takes it as a constant behind a cheap guard -- V8's
     const field tracking, Truffle's `@CompilationFinal`, HotSpot's
     `@Stable` -- easier here, since the language states the
     immutability rather than the VM guessing it.
  5. **Escape analysis**: the immutable fields of an object whose
     allocation is removed are SSA values with no stores to track.
  6. **Value objects** (§3.1.1), immutable and without identity, take all
     of the above and may be copied freely ("Research directions for the
     JIT", below).
  Open before it is built: whether a mirror can write an immutable slot
  in ns-next, and if so that the write invalidates as an edit does; code
  called from an initializer can read a slot not yet written, so 1 holds
  there but 3 and 4 assume nothing; a lazy slot only once filled. 1 and
  2 fit with stage 5 or right after it, 3 and 4 with the speculation.
  [execution.md](design/execution.md), "Planned: the optimizing JIT".
- **Research directions for the JIT, options** (noted with the user on
  2026-11-10 as options, none decided; each to be designed and measured
  before it is taken up). Four rest on Newspeak's own semantics, where
  other VMs would have to speculate:
  - **Specializing optimized code to the values of a module's immutable
    slots**: every import and class reference a literal, one copy shared
    by every instance made with the same arguments ("Exploiting
    immutable slots", above, point 3).
  - **Capabilities as a proof of a closed world.** Only a holder of a
    mirror can edit or reflect on a class, and a mirror is a
    capability; a module never handed one cannot change its classes.
    For such modules class-hierarchy assumptions, static resolution of
    sends and even ahead-of-time compilation need no invalidation.
    Optimization grounded in capability safety appears to be unexplored;
    Smalltalk and JavaScript cannot offer it.
  - **Value objects passed without a box.** A value object (§3.1.1) is
    deeply immutable and has no observable identity, so it may be
    passed across calls in registers (the scalarized calling convention
    of Java's Valhalla), duplicated freely, and inlined into the fields
    of its holder (Wimmer and Mössenböck, "Automatic Feedback-Directed
    Object Inlining in the Java HotSpot VM", VEE 2007, never in
    production for lack of immutability). CD lives on small vector
    objects ("CD runs slower on ns-next than on CPython", below).
  - **An image that carries profiles and optimized code**, so that a
    program starts warm: HHVM's Jump-Start (Ottoni and Liu, CGO 2021)
    shares profiles across a server fleet and Java's Leyden works
    towards it; the invalidation per application already says when such
    code no longer holds. [image-format.md](design/image-format.md).
  And from recent research, rarely in production:
  - **Deoptless** (Flückiger, Ječmen, Krynski and Vitek, PLDI 2022): a
    failed assumption continues in a continuation compiled for the case
    that failed, not in baseline code, sparing the round trip of a trap
    and a recompilation.
  - **Contextual dispatch** (Flückiger et al., OOPSLA 2020, the Ř VM):
    several versions of a body chosen by the calling context --
    argument types and constants -- the generalization of customizing
    by the receiver.
  - **Equality saturation in the mid-end**: Cranelift's acyclic
    e-graphs (2023) apply rewrite rules without an order to get wrong;
    the tag, untag and range rules of stage 5 could be stated that way.
  - **Learned heuristics**: inlining limits chosen by a model trained on
    the corpus and the benchmarks (LLVM's MLGO, Trofin et al., 2021)
    instead of Strongtalk's fixed limits.
  - **Copy-and-patch templates** (Xu and Kjolstad, OOPSLA 2021; CPython
    3.13's JIT): baseline templates stitched from code LLVM compiled
    ahead of time instead of written by hand.
  - **Intel APX**: 32 general-purpose registers on coming x64 processors,
    which the macro assembler's roles and the linear scan could use.
  [execution.md](design/execution.md), "Planned: the optimizing JIT";
  "Further JIT techniques" above.
- **The recompilee walk takes a snapshot tree and a plan at every step**
  (noted at the inlining branch's final review, 2026-11-09).
  `jit::find_recompilee` asks `inlines` at each frame it goes up to,
  which takes a whole `SnapshotTree` and makes a plan of the candidate
  every time, and `inlined_into_a_copy_below` reads up to
  `RECOMPILEE_SEARCH_LENGTH`, ten, senders per step: up to ten trees and
  a hundred frame reads per budget exit. Budget exits are rare -- one
  per body per budget, counted by `budget_exits` -- and the walk's cost
  was not separated from the rest in the measurements of "Inlining".
  What would find out whether it matters: the time of the run loop's
  budget-exit path, by the protocol of "Measuring", on the benchmarks
  with the most `budget_exits`; if it shows, the tree and plan cached
  per candidate body within one walk.
  [execution.md](design/execution.md), "The recompilee walk".
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
- **The initializer leaf copies its decided `Step` through memory**
  (2026-11-05). `Mutator::decide_superclass_initializer` answers a
  `Result<Step, Signal>` through a stack slot it writes field by field,
  which the initializer leaf (`jit::leaves`) copies out with 16-byte
  loads, a load no store forwards: the instruction after the first of
  them holds about 4% of `bench_new2`'s samples. Inline, or an answer
  in two scalars, would remove it, as inlining `JitState::body_of`
  removed the same stall from `materialize::leaf_frame`.
  [execution.md](design/execution.md), "What the optimizing tier's
  foundation moved".
- **`bench_megamorphic.ns` since road step 5b1** (2026-10-18): +1.3-2.1%
  against master after the private and super slot accessor check, on all
  three pinned builds a side, where every other program measured within
  noise. It is at the edge of build noise and not diagnosed: the check runs
  only on a private or super slot accessor's send, and the benchmark's
  megamorphic sends are method sends, so a code-layout effect is as likely
  as a cost of the check itself. It stayed: "What closures in machine
  code moved" measured +3.1% against `master` (not reproduced in a second
  sitting), and "What reclamation moved" +2.8% (4.744 against 4.613 ms,
  2026-11-01), with stage 4's tree before the reclamation already +2.7%
  in the same sitting and level with the head (+0.1%); its loop ran
  interpreted there, so the cost lies in the interpreter's send or its
  layout. Since stage 5 its loop runs in machine code, 85.1% faster than
  `master` ("What on-stack replacement moved" in
  [execution.md](design/execution.md)), so the cost is the interpreter's
  alone, to be diagnosed with the JIT off. [object-model.md](design/object-model.md), "What a
  slot access checks"; [execution.md](design/execution.md), "What
  reclamation moved".
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
  `become` is experimental in Rust 1.100.0 (checked 2026-11-16), so the
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
- **A symbol literal costs machine code a leaf call.** `LoadLiteral` of
  a `Symbol` (and of a `String`, a `LargeInteger` and a boxed `Float`)
  lowers to the step leaf (`jit::baseline`, `allocated_literal`), which
  views the frame (`materialize::leaf_frame`) and runs the instruction
  interpreted: about 17 ns where a template would take one or two. Nearly
  every step leaf call DeltaBlue makes is `#forward` (about 4,200 of every
  4,300 sampled), about 2.2 million a run of 247 ms, so a template that
  loads an interned symbol's handle -- kept current across moves, as a
  closure site's words are -- would save about a seventh of the run.
  Measured 2026-11-01 (a profile and a sampled count of the step leaf's
  instructions). [execution.md](design/execution.md), "The code zone"
  (the lookup by a code address).
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
- **A future as a leaf's operand leaves machine code** (noted
  2026-10-31). `ResolveFuture` forces a future by a compiled call of its
  `resolve`, and `resolve` runs a whole chain of futures in machine code,
  but a future met as an operand where no value is taken still exits:
  `InitializeSlot` handed a future for a value's slot, and a leaf that
  declined for a future among its operands -- a mirror row, or a
  primitive method's leaf -- once the program has made a future
  (`interpreter::mirror::send_inlined_for_machine_code` answers `Exit`
  for any decline then). The interpreter sees a resolved one through
  and forces any other under `Continuation::Reexecute`, running the
  instruction again interpreted ([execution.md](design/execution.md),
  "Running an instruction again after forcing a future"). A sibling's
  future handed to a leaf inside a slot clause -- `sum = 1 + base` -- can
  take that path, and each exit spends the exiting body's exit budget,
  so a clause doing it often enough has its initializer abandoned. How
  machine code should see through a future among a leaf's operands, and
  what it costs, is open.
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
- **The stress gate's time** (noted with the user on 2026-11-01). The
  stress step of `cargo xtask gates end-of-batch` takes about 662 s with
  its sixteen suites at once (2026-11-14; 1,038 s before the move and
  scavenge modes took a floor of 2,000 and a budget of 250, the pause
  mode weighed the stack and a full collection's fixed costs were cut,
  1,233 s before the tests no switch reaches were left out, 1,642 s on
  the whole suite): every mode ends after 551 to 642 s, COLLECT the
  last. The suites share the cores, so the gate is bound by the work of
  all of them. Each mode leaves out the tests measured to reach no
  switch (WIDE keeps those of them that compile; `docs/design/README.md`,
  "The tests a gate leaves out"), runs `debug_metadata` only under WIDE
  and starts the tests that ended a run alone first. The slowest tests
  now, under the sixteen suites at once: `runtime/jit/counting_loop`
  232 s under COLLECT, 168 s under SCAVENGE and 146 s under MOVES (it
  traces about 1,000 units over 10 million safepoints, so the floor
  moves and scavenges it at every one; under SCAVENGE most of it is the
  remembered set's verification walking the old space), the
  `the_depth_limit_counts_suspended_activations` test 265 s under PAUSE
  and `runtime/optimizer/inlined_block_returns_from_its_home` 220 s under
  MOVES. The dev profile already builds at `opt-level = 1`, so a profile
  of `opt-level = 2` with debug assertions would gain less than an
  unoptimized build would suggest; unmeasured. The candidates left, none
  built:
  - *The remembered set's verification under `NS_NEXT_STRESS_SCAVENGE`*,
    a walk of the old space at every scavenge, which the floor runs at
    every safepoint of a small loop; and the cell verification beside
    it, paced by the cells checked ("What stage 3's reviews left", "The
    stress verification's cost", above). A gain for the gate only.
  - *A full collection's remaining fixed costs*: `Heap::applied_mixin_ids`
    is built afresh whenever the tree walk runs (the walk is skipped
    while the last walk's witnesses stand, `object-model.md`), and
    marking's roots outside the heap are visited in full at each
    collection. Both are small beside the marking of a real heap.

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
- **`Decimal` and its literal** (decided with the user on 2026-11-23). An
  exact decimal is a kernel class of its own, `Decimal`, reached through a
  protected accessor on `Object`, with the literal `2.50s`, `2.5s2`,
  `3s2`, `-0.1s` (no radix); today `2.5s2` is `2.5` and the send `s2`.
  Open for the batch that builds it: what a scale below the digits written
  means (`3.14159s2`). The specification's gap `semantics.gap.decimal`;
  [specification.md](design/specification.md), "The second batch's
  deviations", decision 1.
- **Read-only and value collections, and `ImmutableArray` as the tuple
  literal's class** (decided with the user on 2026-11-24). A tuple literal
  becomes a shallowly immutable `ImmutableArray`, and the collections get
  read-only and value variants built from seven mixins applied to
  `Object` and to `Value` alike (`Collection`, `OrderedCollection`,
  `IndexedCollection`, `MutableIndexedCollection`, `GrowableCollection`,
  `KeyedCollection`, `MutableKeyedCollection`), with `ImmutableArray`,
  `ValueArray`, `ImmutableMap`, `ValueMap`, `ImmutableSet`, `ValueSet`,
  the views `ReadOnlyArray`, `ReadOnlyMap` and `ReadOnlySet`, and the
  conversions `asImmutable`, `asValueArray`, `asValueMap`, `asValueSet`
  and `asReadOnly`; `String` stays outside the hierarchy. The whole
  hierarchy and where each class lives are
  [specification.md](design/specification.md), "The second batch's
  deviations", decision 3; the specification's gap
  `semantics.gap.immutable-array`.
- **Pattern matching** (decided with the user on 2026-11-24 to be a
  topic of its own). A pattern literal signals the gap
  `semantics.gap.pattern` today. The design is open: how a variable
  pattern `?y` is bound in the block an application runs, `=>`,
  `case:otherwise:`, the combinators, a subject's `match:` deciding what it
  reveals, as Felix Geller's `NSPatternMatching.ns` in Bracha's canonical
  implementation has them, weighed against Scala's extractors, Rust's
  `match` and Java's record patterns; lowering a literal to §5.1.9's
  `Pattern` sends is part of it.
  [specification.md](design/specification.md), "The second batch's
  deviations", decision 6; [frontend.md](design/frontend.md), "What is
  kept but ignored, and what a reprint drops".

## Planned: images, liveness and reflection

- **Metadata through the mirrors** (§4.3: metadata "are available at
  runtime via mirrors"): `ns-syntax` collects every metadata comment by
  its §4.3 target (`metadata.rs`'s `metadata_of`;
  [modules.md](design/modules.md), decision 6), and only `:module:` is
  read; a class, method and slot mirror are to answer theirs. Open is
  what an image written without source answers, since Primordial Soup
  parses metadata from the source on demand
  (`prior-art/primordialsoup/newspeak/Mirrors.ns:758, 1216-1223`). Two
  targets the collection does not tell yet, which a mirror answering
  them must: a metadata comment after a pattern's return type or its
  where-clause gets no target at all, and a return statement's metadata
  is an `Expression` target, not told apart from its expression's.
- **Suspend and resume.** Needs a yield path out of `vm::run`, a re-entry
  that continues a decoded frame, and an encoding for the side table of
  suspended activations -- every `Continuation` variant (`NestedClass`,
  holding a `NestedClassCreation`, `Unwind`, `VmErrorSignalled`,
  `Reexecute`, `ReturnToSuspended`), the counting, the entered level -- and for
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
- **The debugger**, at the level of IntelliJ's Java debugger, plus
  edit-and-continue of running activations; its design is decided (seventeen
  decisions, nine on 2026-11-10, stage 2's six and stage 3's two on
  2026-11-12). Its first stage is built (2026-11-11): the debugging API at
  the run loop's safepoint, with atomic poll words a remote arms from
  another thread to pause the program, the commands served while it runs,
  the stop, which deoptimizes a frame stopped at a poll of optimized code in
  every JIT build, the stack and values read at it, continue and terminate,
  and the pause stress mode ([debugger.md](design/debugger.md), "The
  debugging API at the safepoint"). Its second stage is built (2026-11-12):
  the stop locations, send entries and variable tables both lowerers collect
  when the debugger compiles a declaration's level again from its module's
  current text, kept per body only when it compiles again alike, which give
  a stopped frame its name, its place and its named variables
  ([debugger.md](design/debugger.md), "Compiling the metadata again"). Its
  third stage is built (2026-11-13): breakpoints patched in place (a
  method's fast path switched off while its body is patched), stepping by
  one-shot breakpoints with the step filters, run to cursor, method
  breakpoints, exception breakpoints, which stop where the handler search
  invokes a handler for a caught exception and where it is about to send
  `defaultAction` for an uncaught one, and the breakpoint and step stress
  modes ([debugger.md](design/debugger.md), "What stage 3 moved" for its
  cost). Its fourth stage is built (2026-11-14): the DAP agent in `ns-cli`
  behind `run` and `load`'s `--debug` and `--debug-connect`, hit counts
  counted by the VM, the language server's breakable lines, and the plugin's
  debugger with line, method and exception breakpoints, from which the
  debugger is usable in the IDE ([debugger.md](design/debugger.md),
  "Launching, the start and the end", "The JetBrains plugin", "What stage 4
  moved" for its cost). An uncaught exception whose `defaultAction` override
  a fast path answers -- a constant, a slot -- pushes nothing at that send,
  so it stops at the next poll, the return to `signal`, after the override
  answered; a handler that is no block and whose `cull:` a fast path answers
  likewise stops after it ran ([debugger.md](design/debugger.md),
  "Exceptions"). Its fifth stage is built (2026-11-17): an expression
  evaluated as a block in a stopped frame's scope, setting a variable,
  conditional breakpoints and logpoints, restart frame and force return,
  smart step into, marking objects, the memory view and folding the
  shipped frames, in the VM, the DAP agent and the plugin, and `ns-next
  mcp`, an MCP server through which an agent debugs a program
  ([debugger.md](design/debugger.md), "Evaluation", "Writing a frame",
  "Restart frame and force return", "The MCP server", "What stage 5
  moved"). The rest: hot swap, edit-and-continue and `ActivationMirror`
  ("Next" above); after stage 6, attaching to an `ns-next` started
  without `--debug` ([debugger.md](design/debugger.md), "Planned: the
  order of building it"). The time a step took, shown inline, stays
  planned in no stage yet (decided with the user on 2026-11-15;
  [debugger.md](design/debugger.md), "The time a step took"), and so do
  the variables' values shown inline in the editor, which the plugin
  would add over its client ([debugger.md](design/debugger.md), "The
  JetBrains plugin"). "Create Method" for an uncaught
  `MessageNotUnderstood`, and the forced `nil` from
  `Object>>doesNotUnderstand:` when it is declined, need stage 6's
  install, the forced return being built; until then going on from such
  a stop ends the run ([debugger.md](design/debugger.md), "Exceptions").
  It needs no yield path out of `vm::run`. Still to decide, before a
  stage takes it: watchpoints on slots ([debugger.md](design/debugger.md),
  "Open questions"). What the values view still lacks: a Newspeak
  collection's renderer -- `List`, `Map` and `Set` show their slots,
  where a renderer would run `size`, `do:` and `printString` as an
  evaluation under a limit of time and sends -- the elements of a
  `ByteArray` and a `String`, read by the heap's accessors and paged,
  and pinning a value other than by a mark past its stop
  ([debugger.md](design/debugger.md), "Values the IDE holds").
  - **What stage 1's reviews left** (2026-11-11), none of them a wrong
    outcome:
    - *A stop holds a value once per mention*: every description
      (`values::StopValues::describe`) stashes a new `Root`, so a value
      the agent reads twice in one stop takes two `Root`s and two ids,
      and the table grows with the reads rather than the values; and the
      ids, continued across the stops of one attachment, wrap after
      2^32 values handed out, past which an id of an earlier stop could
      name a value of a later one ([debugger.md](design/debugger.md),
      "Values the IDE holds").
    - *Tests*: `Children` from a `start` past the last child, the pause
      of a remote whose debugger was detached rather than replaced, and
      the refusal's text for a stale value id
      (`a_value_id_does_not_outlive_its_stop` asserts only the command's
      name) have none; `a_pause_while_a_collection_is_requested_is_served`
      cannot tell a pause served before the early return of a safepoint
      whose service raised a request from one served at a later
      safepoint, since no deterministic setup keeps a collection
      requested at the stop; and the order in which
      `Interpreter::serve_between_runs` clears the commands bit before
      it drains is held by review only, since the race needs the agent's
      `send` between two instructions of the VM's thread and the runtime
      has no hook there ([debugger.md](design/debugger.md), "Commands
      while the program runs", "Testing and gates").
    - *The machine-code assertions skip every stress mode*:
      `counters_are_exact` (`crates/ns-runtime/tests/debugger.rs`)
      turns off the assertions of `a_pause_stops_compiled_recursion` and
      `a_stop_at_an_optimized_poll_is_deoptimized` on the JIT's counters
      under any `NS_NEXT_STRESS_*`, where only the modes that change
      when code is compiled or left (JIT, EXIT, ZONE, OPTIMIZE, DEOPT,
      INVALIDATE) make them inexact.
    - *A fence that tests nothing*: the unit test
      `fetch_or_and_fetch_and_change_bits_and_answer_the_previous_value`
      (`heap/machine_words.rs`) calls `MachineWords::fence` after joining
      the writer thread, whose join already orders its write.
  - **What stage 2's reviews left** (2026-11-12), none of them a wrong
    outcome:
    - *Compiling an image's text again grows the interner*: the
      metadata's compilation parses and resolves the module's text, which
      interns every name in it, local variables' and those of
      declarations the image's code never named among them; for an
      image, whose text is untrusted input, that grows the thread-local
      interner at a stop, against `Symbol::lookup`'s rule that no run
      grows it ([memory.md](design/memory.md), "Weak references",
      "Permanent and program-made"). The growth is bounded by the text
      the image carries and the edits made to it, since a name is
      interned once; a resolution that probes rather than interns, or an
      interner of the debugger's compilation's own, would close it
      ([debugger.md](design/debugger.md), "Compiling the metadata
      again").
    - *A level neither lowering pairs is compiled twice*: the cache
      compiles a level again under the other lowering whenever the
      first left one of its bodies unpaired, also when no lowering can
      pair it -- a superseded version an edit changed, a body past the
      compiler's limits -- once per level, since the result is cached
      ([debugger.md](design/debugger.md), "Compiling the metadata
      again", "Which lowering").
    - *Each stop walks every method family per frame*:
      `stack::origin_of` finds a block's member by walking the families
      of its current class's level, and of each version of it an
      install superseded that the heap still holds when the current
      one holds none, every frame at every stop; the pause stress
      mode's time is where it would show.
    - *The oracle's gaps*: a golden that does not parse or resolve, and
      one whose `build` fails, passes its `debug_metadata` trial without
      being held to its recorded failure; a statement range's end, a
      factory's send completeness (skipped by member), the factory of an
      unapplied nested declaration without a class side, and a vector
      slot's index (the tables record no vector's size) are not checked
      (`debugger/metadata/check.rs`).
    - *Tests*: none of a text that parses but does not resolve, of the
      class-side branch of `compile_level` with a registered id, nor of
      a vector variable read before its `MakeVector` ran;
      `a_vector_variable_shows_its_current_value` does not assert which
      `VectorHolder` it read through; `a_frame_is_named_and_located`
      checks that the top frame waits on no send only under the
      optimized lowering; and `tests.rs`'s assertion message names five
      bodies where it asserts at least four.
    - *Code*: the reference lowering's send entry tells a send was
      emitted by the code's length rather than by the `BodyTooLarge`
      check; a local initializer's statement span includes its period,
      which no comment says; and the selector-span tests sit at the end
      of `declaration.rs`'s tests rather than beside the expression
      tests.
  - **What stage 3's reviews left** (2026-11-13), none of them a wrong
    outcome:
    - *Setting breakpoints on a large module is unmeasured*:
      `SetBreakpoints` resolves its offsets against every level of the
      module, compiling each again for its metadata
      (`metadata::Cache::level`), so a breakpoint in the kernel or
      another large module compiles all of it once per install; the cache
      keeps the result, but the first `SetBreakpoints` after an install
      pays it at a stop or a safepoint ([debugger.md](design/debugger.md),
      "A breakpoint names a source position").
    - *Superseded bodies stay patched, until the set is sent again*: the
      old bodies an install replaced keep a line breakpoint's patches,
      and their code ids are named at every full collection, until the
      breakpoint is removed -- and a `SetBreakpoints` re-sending the
      module's set, which an IDE sends at every change of it, removes
      them from the old bodies and resolves the set against the current
      ones only, so a frame still running an old body runs past it;
      `MethodBreakpointEntry::bodies` likewise only grows across
      installs, every body a method breakpoint was set on kept. A
      program that installs in a loop under a breakpoint grows both
      ([debugger.md](design/debugger.md), "A breakpoint names a source
      position", "Method breakpoints").
  - **What stage 4's reviews left** (2026-11-14), wrong outcomes among
    them -- a breakpoint a line off, a stop nobody asked for, a frame
    over an old text:
    - *The plugin starts no image*: its run configurations run a program
      or a project from source, so `ns-next load --debug-connect`, which
      the agent serves, has no run configuration of the plugin's; an
      image is debugged only by a client the user starts
      ([debugger.md](design/debugger.md), "Launching, the start and the
      end"; [editor-support.md](design/editor-support.md), "Known gaps").
    - *A line step fetches the whole stack at every repetition*: a `next`
      or `stepIn` by line sends the VM's step again while its stop stays
      on the line in the same frame, and to tell the frame it asks for
      the stop's whole stack trace each time (`debug/stepping.rs`), up to
      100,000 activations deep, where it needs only the depth and the top
      frame; a VM query answering the depth alone would make a line step
      in deep recursion cheap ([debugger.md](design/debugger.md),
      decision 4).
    - *A `setBreakpoints` after an install counts lines in the older
      text*: the agent knows a module's text as a stop last read it, so a
      `setBreakpoints` sent after an install but before the next stop
      turns its lines into offsets of the older text, which the VM then
      resolves in the current one; the breakpoint can land a line off
      until the client sends the set again ([debugger.md](design/debugger.md),
      "Launching, the start and the end").
    - *A pause racing a stop stops twice*: a `pause` sent as the VM makes
      a stop of its own -- a breakpoint's, a step's -- reaches the VM
      after that stop, so its bit stays set and stops the program again
      at the next safepoint after the client's `continue`, a stop the
      client did not ask for there ([debugger.md](design/debugger.md),
      "The stop").
    - *One class list for both exception filters*: the VM keeps one
      list of classes (`Command::SetExceptionBreakpoints`), so a class
      condition given for caught exceptions restricts uncaught ones too,
      and the plugin's class breakpoints share it; two lists would give
      each filter its own ([debugger.md](design/debugger.md),
      "Exceptions", "The JetBrains plugin").
    - *An unverified method breakpoint warns at every resend*: the
      plugin reports a method breakpoint the agent cannot set as a
      warning each time it sends the method breakpoints, which the
      platform does at every change of any of them, since it shows no
      verified state for a method breakpoint
      ([debugger.md](design/debugger.md), "The JetBrains plugin").
    - *An exception stop bypasses the platform's breakpoint handling*: a
      stop at an exception is shown through `positionReached`, since its
      `hitBreakpointIds` name no line or method breakpoint, so what the
      platform does at a breakpoint hit -- removing a temporary
      exception breakpoint, its suspend policy, its log -- does not apply
      to it ([debugger.md](design/debugger.md), "The JetBrains plugin").
    - *A module shown by reference keeps its first text in the plugin*:
      the plugin keeps a `sourceReference`'s `LightVirtualFile` for the
      whole session (`NewspeakSources.kt`), while the agent keeps a
      module's reference across an install that changes its text, so
      after such an install the frames' lines and columns, counted in the
      new text, are shown over the editor's old one; the plugin would
      have to read the text again when it changed -- the agent knows its
      revision -- and replace the file's content
      ([debugger.md](design/debugger.md), "The JetBrains plugin").
    - *The agent's event pump polls*: the pump waits for the VM's next
      event 10 ms at a time, so that it notices a detach or the drain
      at the program's end (`debug/mod.rs`, `PUMP_SLICE`), and wakes a
      hundred times a second for the whole run; a wake-up the agent
      sends with its request would let it sleep. Only a debugged
      process pays it ([debugger.md](design/debugger.md), "Launching,
      the start and the end").
    - *Tests*: the real `newspeak/breakableLines` round trip through the
      platform's LSP client (the breakpoint types' test replaces the
      fetcher), a breakpoint in a shipped module's read-only file, the
      turning off and on that carries an edited pass count or exception
      box to a running session, and a `pause` during a line step's first
      repetition have
      none ([editor-support.md](design/editor-support.md), "Known gaps",
      "Plugin testing and gates").
  - **What stage 5's reviews left** (2026-11-17), a stop's reason or
    text wrong among them:
    - *A failed condition at an exit by an unwind loses its text*: a
      pause that aborts a method breakpoint's
      condition at an exit by an unwind makes the stop report that exit
      (`StopReason::MethodExit`) rather than the pause
      (`conditions::exit_ended`); a line condition that fails at the stop
      after an exit condition failed is named with the exit's error and
      its own text dropped, and an exception's stop names the failed
      exit's breakpoint without its error (`conditions::failure_named`)
      ([debugger.md](design/debugger.md), "Hit counts and temporary
      breakpoints").
    - *An abort names no evaluation*: `Command::AbortEvaluation` aborts
      whichever evaluation runs, so an abort the agent sends for a hover
      the VM answered meanwhile can abort the client's next `evaluate`,
      which answers "evaluation aborted"; and dropping a remote's
      `DebugEvents` alone, its `DebugControl` kept, never aborts an
      endless evaluation ([debugger.md](design/debugger.md),
      "Evaluation").
    - *A shipped module's root class is not withheld everywhere*:
      `Mutator::is_withheld` holds for the VM mirror, its class and
      metaclass and a recorded shipped module's instance, but not for that
      instance's class object and metaclass, which the memory view
      withholds by their class alone ([debugger.md](design/debugger.md),
      "Marking objects", "The memory view").
    - *Instances of an unknown class*: `nsNext/instances` of a class name
      no module declares answers an empty page rather than a refusal
      ([debugger.md](design/debugger.md), "The memory view").
    - *The memory view's cost, unmeasured*: every `Instances` and
      `Referrers` answer walks the whole object space again, a
      `Referrers` answer more than once -- the referrer walk, then the
      walks that decide which returned frame objects nothing living
      refers to -- and the withholding of a closure asks
      `stack::code_is_shipped` for each closure the walk meets, which
      searches every registered mixin's members; nothing measures either
      on a large heap ([debugger.md](design/debugger.md), "The memory
      view").
    - *An unexplained failure*: once, while the MCP server's start was
      built (2026-11-17), the agent answered a request "`` is not
      supported yet" -- a command named by an empty string -- in a run
      whose server's standard output closed early; it did not recur in
      more than twenty runs, nor since. A debug build's agent now logs an
      unknown command's connection, name, seq and message keys to its
      standard error, and the MCP server every refused response, so that a
      recurrence says what arrived ([debugger.md](design/debugger.md),
      "The MCP server").
    - *The MCP server's copy is put by hand*: `.mcp.json` starts
      `target/mcp/ns-next`, so until `cargo xtask mcp` has run -- in a
      fresh checkout, or after `target/` was cleaned -- the client fails
      to start the server, and the copy stays at the build it was made
      from until `target/mcp/` is deleted and the task run again
      ([debugger.md](design/debugger.md), "The MCP server", "Starting
      it").
    - *The plugin*: its hovers send no `context: "hover"` -- they
      evaluate only a frame's variable names -- so the agent's hover
      bound and its abort never apply to them; the memory view's panel
      is tested through its model only; and Force Return, Step Into
      Shipped, Mark Object…, Smart Step Into, Reset Frame and the memory
      panel under the split debugger were checked by hand on 2026-11-17
      (IntelliJ 2026.2, `xdebugger.toolwindow.split` on) but by no test,
      the plugin's tests running the monolithic one
      ([debugger.md](design/debugger.md), "The JetBrains plugin").
    - *Tests*: `an_error_is_the_result_and_the_program_goes_on` asserts
      only that the error's description is not empty,
      `a_collection_during_an_evaluation_keeps_the_stop_s_values` does
      not show that a collection ran, and
      `a_block_an_evaluation_stored_reads_the_frame_after_it_returned`
      asserts only that the answer has a class name; none evaluates in a
      frame whose optimized code inlined scopes, restarts the initializer
      of a declaration with no slots, smart-steps in a `whileFalse:` loop
      over a bare local, or detaches with marks set; and the endless
      evaluations' tests (`start_endless`) wait for the evaluation to
      start by a sleep (`crates/ns-runtime/tests/debugger.rs`). The pause
      stress reader's refusal to evaluate while a breakpoint's token waits
      (`Breakpoints::waits_for_a_fetch`) is exercised only beside the
      suspend stress mode, by one test, never by the pause mode alone; and
      the two regression tests of the stage's end-of-batch fixes,
      `a_suspended_frame_refers_from_the_stack` and
      `a_stress_pause_beside_suspensions_leaves_a_token_to_its_frame`, do
      not assert their precondition -- that a frame was suspended, that
      a token waited when the mode stopped -- so a change that stopped
      producing it would leave them passing vacuously.
- **Reflection beyond road step 5b2a**: a class declared from source with
  no predecessor (`ClassDeclarationBuilder fromSource:`, loading a module
  at run time), a superclass change, adding,
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
  a mirror may hand out. The walk exists, the debugger's memory view's
  (`Heap::each_object`, `Heap::refers_to`; [debugger.md](design/debugger.md),
  "The memory view").
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
  time still needs design. How a turn waits for a promise is open
  (decided with the user on 2026-11-24): callbacks and promise pipelining
  as Bracha and E have it, `await` as Swift's actors have it, or a parked
  stack as Go's goroutines and Java's virtual threads have it
  ([specification.md](design/specification.md), "The second batch's
  deviations", decision 4). Until then an asynchronous send signals the
  gap `semantics.gap.actors`. [liveness-and-reflection.md](design/liveness-and-reflection.md).
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

- **The editor misses a reserved id in a program's file that stands at a
  shipped source's place** (noted 2026-11-19). The analyzer exempts a
  document from the reserved-namespace finding when it is a copy of the
  binary's own source: its text that source's byte for byte, or its path
  ending in that source's place below a `core-library` directory, or a
  fingerprint's directory within one
  (`crates/ns-analyzer/src/module_metadata.rs`, `is_the_binary_s_own`),
  since the copy of `Kernel.ns` being edited is no longer the binary's
  text. So a program's own `…/core-library/Kernel.ns` declaring
  `newspeak.core.Kernel` shows clean in the editor, while `ns-next run`
  refuses it: a false negative of the editor only, never of the loader.
  [editor-support.md](design/editor-support.md), "Module metadata".

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
- **Hints for a class's header slots**, deferred (decided with the user on
  2026-10-14): the hints are given for the locals of methods and closures
  only. A header slot needs more than its class's text to prove a fix
  behaviour-preserving: a non-private `::=` slot has a setter its clients
  and subclasses can send; a header's `x:: e` is a setter send a subclass
  can override, which an initializer does not send; and a private slot's
  read-only fix could lift a value object's refusal of a mutable slot,
  while whether a class's instances are values is not in its declaration
  ([editor-support.md](design/editor-support.md), "Hints").
- **`@deprecated`**, not a doc comment tag for now (decided with the user
  on 2026-11-01); until it is, it is an unknown tag, shown in the
  description and warned about. Adding it means a section of its own and
  the grammar in both parsers; striking a deprecated member's uses through,
  as Java's editor does, would need the uses' declarations across files
  ([editor-support.md](design/editor-support.md), "Doc comments").
- **`@since`, `@author` and `@version`**, not doc comment tags (decided
  with the user on 2026-11-01); each is an unknown tag today. Adding one
  means a section of the table and the grammar in both parsers.
- **Links in doc comments** (decided with the user on 2026-11-01): no
  `[Name]` link; a reference stays in backticks until go to definition
  (above) exists, which a link needs to resolve a name to a declaration in
  this or another module.
- **Completion showing a member's summary.** The summary of a doc comment
  (`{@summary …}`, else its first sentence) is shown by the structure
  view's hover only; there is no completion yet, and when there is, its
  items should carry the summary as Java's carry the first sentence
  ([editor-support.md](design/editor-support.md), "Doc comments").
- **`{@inheritDoc}`, undecided.** An override's comment cannot take its
  overridden method's documentation, so an override in the core library
  repeats or rephrases it; whether to add Javadoc's `{@inheritDoc}` (or to
  inherit a missing comment, as Javadoc does) is not decided, and both need
  the superclass's members, which the server sees only within one file.
- **A doc comment's prose is not escaped as HTML.** The hover's Markdown
  and the rendered view pass the description and the tags' text on as
  they are. In JetBrains, the platform's `DocMarkdownToHtmlConverter`
  escapes a tag it does not know -- `<Integer>`, `<selector>`, even
  `<script>` show as written, in prose and in the section table's cells,
  which the core library relies on where it writes a placeholder outside
  backticks -- but passes a known HTML tag through: `<b>` is honoured, and
  `</td></tr></table>` typed in a comment closes the hover's table early.
  Another LSP client may render the raw HTML in the hover's Markdown as
  it is. Escaping `<` and `&` outside code spans in both `hover.rs` and
  `NewspeakDocTags` would make prose safe everywhere
  ([editor-support.md](design/editor-support.md), "Doc comments").
- **More platforms**: a CI build for the other five `bin/<os>-<arch>/`
  binaries of the language server; the repository has no CI.
- Publication on the JetBrains Marketplace stays possible and is not done.
- **Further `ns-next-analyzer` commands**, each looking without running: a
  heap dump and a deeper inspection of an image; what a method compiles
  to, machine code included, the developer tool shows
  ([developer-tool.md](design/developer-tool.md), "Stage 2: printing
  code", "Stage 3: the developer tool in the IDE").
- **A profiler** (asked for by the user on 2026-11-15; not designed yet):
  the time a program spends per method and per send site. It matters because
  a slow program today shows where its time goes only to a native sampling
  profiler, which sees the VM's Rust frames and the code zone's anonymous
  addresses, never a Newspeak method -- the profiles behind "Unexplained
  measurements and unmeasured decisions" below were read so. Whether it
  samples or counts, what it costs a run, and how an agent and the IDE
  reach its report are open.
- **A developer tool like V8's `d8`** (asked for by the user on 2026-11-15;
  designed with the user on 2026-11-18 and, its stage 3, on 2026-11-21;
  stages 1 and 2 built 2026-11-20, stage 3 2026-11-21): four stages,
  each reaching the agent and the IDE as it is built. Stage 1, tracing,
  is built: typed events with their reasons reported to a sink the host
  installs (`ns_runtime::trace_events`), written by `ns-next run
  --trace=<categories>`, narrowed by `--trace-filter`, passed on by
  `debug_start` into a trace file of the MCP server's. Stage 2, printing
  code, is built: bytecode, baseline,
  twin and optimized machine code at install and the inlining plan,
  written by `--print-bytecode`, `--print-code` and `--print-inlining`,
  machine code decoded by `iced-x86` and `yaxpeax-arm` in `ns-cli`, and
  a stopped frame's code through `nsNext/frameCode` and `debug_code`.
  Stage 3, the developer tool in the JetBrains plugin, is built: the
  trace and the listings as typed JSON lines in `--trace-file`'s file,
  each body described with its source positions, switched on in the run
  configuration's "Developer Tool" tab and by watched methods, read live
  and shown in the "Newspeak JIT" tool window, the gutter, inlay hints,
  editor tabs of the code and Show Frame Code; and the MCP server
  answers a traced session's file through four bounded tools
  (decisions 36 to 39, built 2026-11-22). Open: stage 4,
  `ns-next shell`, evaluating against a workspace object as the
  Newspeak IDE does; it matters because an expression is still
  evaluated only at a stop. It is set back behind the debugger's stage
  6, when to take it up not decided (decided with the user on
  2026-11-22). Its open questions are decided in its plan
  ([developer-tool.md](design/developer-tool.md), "The decisions",
  "Planned: the stages", "Open questions").
- **The IDE's debug session as MCP tools** (asked for by the user on
  2026-11-17; after the debugger's stage 5, not designed yet): the
  plugin contributes the tools of `ns-next mcp` -- `debug_step`,
  `debug_evaluate` and the rest, with the same stop summary -- to the
  JetBrains IDE's own MCP server, acting on the IDE's running debug
  session (`XDebugSession`) rather than on a child of their own, so that
  an agent steps, evaluates and sets breakpoints while the user watches
  the editor follow and can take over at any point. It matters because
  `ns-next mcp` debugs on its own, out of the user's sight, and the IDE's
  debugger alone is the user's; the two meet only here. Open: the
  platform's extension point for contributed MCP tools and the IDE
  versions that offer it, whether the tools and the summary are shared
  with `ns-next mcp` as code or as a specification, and how the agent's
  commands and the user's interleave in one session
  ([debugger.md](design/debugger.md), "The MCP server", "The
  JetBrains plugin").

[editor-support.md](design/editor-support.md), "Planned later".

## Deviations from the specification

Every difference from Bracha's specification is a rule of the
specification of ns-next, and its appendix, "Differences from Bracha's
Newspeak" (`docs/spec/appendix/differences-from-bracha.md`), lists them:
the differences adopted as ns-next's own rules, and the gaps -- the
constructs not built yet, each a rule whose identifier holds the segment
`gap`. A gap's work is the item above that names its rule.

## Open design questions

- **A comparison in containers** (2026-11-10). The three-language
  comparison ran natively on Windows, ns-next, Java and CPython only. A
  second mode is open: ns-next, Java, CPython and PyPy each in a
  container from a pinned image, kept beside the native mode, so that the
  comparison is repeatable on Linux and includes a JIT for Python. The gate image (`xtask/docker/Dockerfile`) already has a JDK and
  Python 3.14.4 and runs ns-next, Java and Python; what drives the runs
  and collects the scores, and how PyPy enters the image, are open, and
  are to be decided with the user ([execution.md](design/execution.md),
  "Against Java and CPython").

- **A NaN's payload after arithmetic on two NaNs** (2026-11-09). `+`
  and `*` of two NaNs answer the left operand's payload in machine code
  and whatever LLVM's code for the Rust leaf answers, which nothing
  pins -- an operand order it may swap. A program observes the
  difference through `==` on boxed `Float`s, which compares their bits,
  and through a `Float64Array`'s bytes; no golden depends on it. The
  options are to canonicalize every NaN a `Float` operation answers, in
  the leaf and in machine code, or to pin the leaf's operand order to
  machine code's. A semantic choice, the user's.
  [execution.md](design/execution.md), "Inlining" and "Floats in
  machine code"; [core-library.md](design/core-library.md), "The
  numeric tower".

- **A forwarding `Proxy` breaks the contract of `==` and `=`** (noted
  2026-11-01). A program's subclass of `Proxy` that forwards every message
  sends `==`, `=`, `hash` and `identityHash` on to its target. For a
  target of the kernel's classes or of a class without an `=` (and `==`)
  of its own -- a target whose `=` asks its argument, as some mirrors'
  do, can answer otherwise -- `proxy == target` is true but `target ==
  proxy` false, `proxy == proxy` and `proxy = proxy` are false (they are
  `target == proxy` and `target = proxy`), and a forwarding proxy stored
  as a key of a `Map` or an `IdentityMap`, or an element of a `Set`, is
  never found again and is stored twice. §5.5 asks a proxy to be indistinguishable from its target;
  a future is, because the runtime sees through it in every identity and
  equality leaf (`interpreter/future.rs`), but no other proxy is. Whether
  a program's proxy should get the same -- a forwarder the runtime knows
  to see through, or an `Object>>==` that unwraps one -- or whether
  §5.5's indistinguishability is the forwarder's to arrange, is open.
  `Kernel.ns`, `Proxy>>==`, `Proxy>>=` and `Proxy>>identityHash`, state it.
  Deliberate and harmless beside it: the `=` of `ObjectMirror`,
  `ClassDeclarationMirror`, `MethodMirror` and `SlotDeclarationMirror`
  asks the other object whether it is such a mirror and for what it
  reflects, so an object of a program's that claims to be one decides the
  symmetry with it; the comparison hands it nothing a mirror holds back
  (`MirrorsFactory.ns`, each `=`).

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
  `become` (feature `explicit_tail_calls`), which in Rust 1.100.0 (checked 2026-11-16) is still
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
  leaves it out. It is the specification's gap
  `semantics.gap.reflection.object-class`, a topic of its own (decided with
  the user on 2026-11-24, [specification.md](design/specification.md), "The
  second batch's deviations", decision 7): open besides the cost are what
  happens to the slots the two classes do not share, and what it may do to
  a value object, a builtin instance or the receiver of a running
  activation.
  [object-model.md](design/object-model.md), "`become:` is an eager bulk
  forward".
- **Whether lazy forwarding is revisited.** `become:` is eager, one walk per
  forward, because its one consumer, the atomic install, walks the heap
  anyway; Spur's lazy forwarders would cost every send, `==`, leaf and
  generated code a check. Revisit it, behind the same bulk-forward seam,
  only if a measurement shows `become:` frequent (decided with the user on
  2026-10-05). [object-model.md](design/object-model.md), "`become:` is
  an eager bulk forward".
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

- **ParserCombinators and Splay run slower than Primordial Soup's
  interpreter** (an outside measurement, 2026-11-24). On the eleven
  benchmarks of Primordial Soup's `BenchmarkRunner` on an Apple M1 Max --
  a 5 s warm-up, the median of three windows of at least a second, in
  runs a second -- ns-next's JIT made ParserCombinators 10.5 against
  psoup's 28.6 (0.37×) and Splay 296 against 387 (0.77×), where the other
  five non-micro benchmarks ran 2.3× to 14× psoup's; its JIT barely helps
  either over its own interpreter (9.5 and 211). Both allocate heavily,
  and ParserCombinators leaves blocks by non-local returns. The run
  predates `repeat`, unary `whileFalse` and `valueWithArguments:`: the
  sources were rewritten around them, ParserCombinators with a helper
  that dispatches `valueWithArguments:` by the argument count, so a
  measurement of the unmodified sources is the first step; then a
  profile of each on a release build -- the share in optimized code,
  baseline code, the interpreter, the leaves, the unwinder and the
  collector -- says where the time goes. The benchmarks are not in the
  repository: adopting them is a licensing decision of its own.
- **The interpreter runs at 0.2× to 0.6× Primordial Soup's** (the same
  measurement). With `NS_NEXT_NO_JIT=1`, ns-next made 24.5 against
  psoup's 65.7 on ClosureDefFibonacci (0.37×), 40.3 against 135 on
  ClosureFibonacci (0.30×), 87.8 against 261 on DeltaBlue (0.34×), 59.2
  against 140 on MethodFibonacci (0.42×), 9.5 against 28.6 on
  ParserCombinators (0.33×), 139 against 233 on Richards (0.60×) and 211
  against 387 on Splay (0.55×). psoup's is a bytecode interpreter in C++
  with its own primitives; what makes the gap -- the send's path, the
  push and return, the leaves, allocation -- is unmeasured, and matters
  for every body the JIT has not compiled yet, and on Windows on ARM,
  which runs interpreted.

- **What stage 1 of the debugger moved, unmeasured** (2026-11-11). The
  measurement against `master` found no slowdown (-0.61% on the geometric
  mean), but Permute at forty iterations is separated slower, +1.4% on a
  75 microsecond program (+0.8%, +0.9% and +2.6% by build; +2.0% and not
  separated at eight iterations), unexplained; a sweep of the zone
  paddings, which shift its code's layout, would say whether it is
  layout. And the cold path the batch moved into `vm::take_safepoint` was
  never timed with safepoints actually requested: an attached debugger
  with its defaults was measured with stage 4 and costs nothing measurable
  ("What stage 4 moved"), but no measured program paused, queued a
  command or stopped, so what those cost a running program is unknown, as
  is the poll's cost inside machine code beyond what the benchmark
  programs exercise ([debugger.md](design/debugger.md), "What stage 1
  moved").
- **CD runs slower on ns-next than on CPython** (2026-11-10). In the
  three-language comparison CD made 0.4 runs a second on ns-next, 1.1 on
  CPython 3.15's interpreter and 64.7 on the JVM: the one program ns-next
  loses to an interpreter, and its largest gap to Java, 162×. Its `Float`
  arithmetic is already in machine code, so the time goes elsewhere --
  allocating its `Vector2D` and `Vector3D` objects, the red-black tree,
  the SOM collections or its sends are guesses, since CD was not
  profiled. It matters because the optimizing tier's remaining stages are
  chosen by where the time goes, and a program an interpreter beats
  points at a cost the stages may not address. What would settle it: a
  profile of CD on a release build, the share of its time in optimized
  code, baseline code, the interpreter, the leaves and the collector, and
  the same program's profile under CPython for comparison
  ([execution.md](design/execution.md), "Against Java and CPython").
- **The send- and object-heavy programs are barely ahead of CPython**
  (2026-11-10). Richards is 1.2×, DeltaBlue 1.4× and Storage 1.5× faster
  on ns-next than on CPython's interpreter, Havlak, Json and Sieve 1.7×,
  where the programs of `SmallInteger` arithmetic and small sends are
  4.5× to 9.2× ahead. The cost per send, per allocation and per collection
  operation in the code these programs run is high relative to CPython's
  specializing interpreter, and how much of their time runs in optimized
  code rather than baseline code or the interpreter was not measured.
  What would settle it: profiles of Richards, DeltaBlue and Storage on a
  release build, with `--jit-statistics` for the share of optimized code,
  the per-send and per-allocation cost taken from them and set beside
  CPython's on the same programs ([execution.md](design/execution.md),
  "Against Java and CPython").

- **The interpreter's eight helpers `vm::run` no longer inlines are
  not measured by the final protocol** (2026-11-03). Since road step 8's
  fifth stage, the interpreted backward jump's test of
  `JitSlot::back_edge_is_armed` lowers LLVM's static frequency estimate
  of the instruction loop, so fat LTO no longer counts the call sites of
  `read_outer_local`, `write_outer_local`, `load_vector`, `store_vector`,
  `enclosing_object`, `new_array_from_registers`, `check_applied_clause`
  and `Heap::allocate_object` as locally hot and calls them
  ([execution.md](design/execution.md), "What on-stack replacement
  moved", which has the mechanism). An interim measurement under
  `NS_NEXT_NO_JIT=1` moved the geometric mean +0.03% and `bench_block.ns`
  +2.7%; left as it is with the user, since machine code runs none of
  the eight. What would settle it: a final measurement under
  `NS_NEXT_NO_JIT=1`, and if `bench_block.ns`'s cost holds, the test
  compiled out where the JIT is (`cfg!(ns_jit)`, always false on a target
  without one, today Windows on ARM) and `#[inline(always)]` on the
  helpers -- worth taking up when interpreted speed is measured on such
  a target.
- **A frame that cannot switch delays the other frames of its body**
  (2026-11-03). A body's count of back jumps is the body's, so a frame
  that cannot switch delays another frame of the same body by up to 39
  jumps; unmeasured ([execution.md](design/execution.md), "When a body
  is compiled").
- **A program in machine code measures where its bodies land in the code
  zone** (2026-10-30). `bench_loop.ns` measured 5% slower after the
  closures in machine code on machine code identical to the tree before,
  instruction for instruction, its stubs too: the stubs grew, so the body
  moved from zone offset 0x2de0 to 0x30a0. Padding the zone before the first
  body moves the same binary between 48.9 and 52.2 ms on a 5,000,000-step
  loop by its 16-byte position, and aligning every body to 64 bytes made
  `bench_loop.ns` level but `bench_while.ns` 9.6% and Richards 1.9% slower.
  The stubs' placement counts as much as the bodies': the cut stub and five
  mirror rows of part 4c moved the stubs after the cut stub by 224 bytes,
  those after the mirror-method entries by 279 and the bodies by 288, and on
  that alone `bench_send.ns` measured +2.1%, `bench_while.ns`
  +1.5% and DeltaBlueTon80 +1.2%, builds separated, every body executed
  instruction for instruction the same. The tree before, its stubs and
  bodies padded to the same offsets, runs `bench_while.ns` at the same time
  as the tree after at every body padding (5.93-6.95 ms over thirteen),
  while body padding alone leaves the two 2.4% apart pooled, since it never
  moves the closure stubs the loop calls. DeltaBlueTon80's extra samples lie
  in the zone (stubs +201, bodies +150 of 6,153), its Rust functions taking
  fewer, and two builds with other Rust code are level on it: the zone's
  offsets and the binary's layout roll together.
  The measurement protocol now lays each of its three builds' zones out at
  a padding of its own, so the pool samples the zone's placement as it
  samples `vm::run`'s ([execution.md](design/execution.md), "Measurements").
  Open: whether a placement policy (aligned loop heads, hot stubs on their
  own lines) measures better than the lottery.
- **`bench_signal.ns` 0.9% slower for the inline count of an abandoned
  body's entries** (2026-11-01), against stage 4's tree before the
  reclamation, builds separated (18.756 against 18.582 ms): every
  interpreted push of an abandoned body adds one to its slot's state, an
  increment and a store where that tree compared once and never revived
  the body, and `bench_signal` pushes its abandoned `descend:` about
  255,000 times a send. The count is what revival by interpreted entries
  needs, kept so that a run that allocates nothing does not end with
  every body that once spent its budget interpreted for good; a cheaper
  count, or none on a path the decay serves, is open.
  [execution.md](design/execution.md), "The baseline JIT's foundation"
  (the abandonment), "What reclamation moved".
- **`bench_recursion.ns` 0.7% slower than `master`** (2026-11-01, 0.670
  against 0.665 ms, builds separated): what the reifying twin leaves of
  the cost a conditional's fallback closure put on `fib:`'s Boolean path,
  which was 4.1% before the twin. The lean body runs none of the reifying
  code there; where the rest lies was not isolated.
  [execution.md](design/execution.md), "A frame reifies only in its
  body's twin", "What reclamation moved".
- **The interpreter is 1.35% slower than before stage 3** (2026-10-29),
  with the JIT off, on the geometric mean over twenty-two sources, twelve
  of them slower with their builds separated, `bench_megamorphic.ns` the
  most at +3.6%, then Sieve +3.1%, `bench_while.ns` +2.8%, Permute +2.5%,
  Json +2.2%, DeltaBlue and Bounce +2.0% ([execution.md](design/execution.md),
  "What sends in machine code moved"); with the JIT on, the programs whose
  hot code runs interpreted carry the same cost. Two causes were found and
  fixed (the miss path's store-forwarding stall, a shared accessor
  helper); a bisection of `bench_while.ns` with three builds a side puts
  the rest across tasks 5 to 8 of the stage's plan -- 50.05 ms at
  `master`, 50.50 at `69d29f57`, 51.28 at `83386841`, 51.82 at
  `c7d206bd` -- where the run loop's switch gained the resumption flag and
  the real-safepoint test, the push and the pop count the depth in the
  machine words, the return reads the caller's compiled bit, and `vm::run`
  grew from 8,065 to 8,073 instructions; none of it was isolated. A
  bisection commit by commit with `cargo asm` of `vm::run`, `push_sent`
  and `pop_activation` at each is the next step.
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
  fast path, where machine code does not run the mixed row itself
  (execution.md, "Floats in machine code"). A program keying a `Map` by
  floats, or hashing large values, would show what they cost. Noted by the numbers sub-project, 2026-10-11.
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
  started from, `9507d02`, while seventeen of the twenty-four sources got
  faster. None of the three was profiled after the inlining fix that
  left List, DeltaBlueTon80 and Richards no longer separated from the base;
  the 64-bit handle alone
  measured 0.9% to 9.9% slower on every source. Whether road steps 3a and 3b
  together are level with `5fc230b`, road step 3a's base, was not measured:
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
- **The collections' remaining cost.** With `List`,
  `Map` and `Set` written here and their modification count, measured at
  `119b1fc1` against `master` `eed93663` (release builds, three builds by
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
  `to:by:do:`" above). Those benchmarks now carry the SOM collection
  library of their own (`benchmarks/are-we-fast-yet/newspeak/Som.ns`), so
  measuring either way out needs a program that uses `List`, `Map` and
  `Set` (a microbenchmark, or the benchmarks at a commit before
  `Som.ns`). [core-library.md](design/core-library.md), "What is
  measured".
- **`StringBuilder`'s state check on every member is unmeasured.** Every
  member that reads a builder's slots sends the private `checkState:`
  first, `size` and `isEmpty` included, so that a builder an image crafted
  is refused with an `ArgumentError` (`faa9447a`); what that costs a
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
  language server does, would close it. The debugger reaches the same
  recursion on a metadata miss: it compiles a module's text again, and an
  image's text is untrusted input, so a crafted image whose text holds a
  deep chain where its code holds none aborts the process at the first
  stop in that declaration rather than answering no metadata
  ([debugger.md](design/debugger.md), "Compiling the metadata again"),
  and the language server's `newspeak/breakableLines`, which compiles
  the open document, guards against it: on a worker thread of 64 MiB, it
  answers `null` for a module nesting more than 20,000 sends, which
  closing this item removes ([editor-support.md](design/editor-support.md),
  "Breakable lines").
  Until it is closed, the overflow at
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
- **`Exception>>pass` with no outer handler returns into the handler that
  sent it** (found by a review of `Kernel.ns`'s doc comments,
  2026-11-01, reproduced by a probe, not yet pinned by a golden: a `pass`
  with no outer handler, in a subclass whose `defaultAction` answers 42,
  answers 42 to the handler, whose completion then signals 'return: sent
  to an exception that is not being handled'). With no enabled handler
  outside the running one, `pass` runs `defaultAction` and answers its
  value to the handler that sent `pass`; when that handler then completes,
  its return signals 'return: sent to an exception that is not being
  handled'. ANSI defines `pass` as `outer`, "except that control does not
  return to the currently active handler": `defaultAction`'s value should
  become the value of the `signal` that raised the exception, and the
  handler should not run on. `Kernel.ns` documents
  `pass` as it behaves today; the fix changes that comment with it. A
  golden of an exception passed with no outer handler, its expected
  output established from ANSI's definition, would pin it.
  [core-library.md](design/core-library.md).

## Known defects and gaps

- **A plugin debugger test timed out once under load** (noted
  2026-11-18, with the trace's stage 1): `NewspeakBreakpointTypesTest`'s
  `testATemporaryBreakpointIsRemovedAfterItsStop` failed with a
  `TimeoutException` in one `cargo xtask gates commit` while the machine
  was busy, and passed on the rerun; nothing it runs had changed. Not
  reproduced since: 20 runs while `ns-runtime`'s suite loaded the
  machine passed in about 4 s each, against the 30 s wait. The removal
  of a temporary breakpoint is ordered before the resume -- IntelliJ
  removes it in `sessionResumed`, which runs before
  `XDebugProcess.resume`, and the handler sends the emptied set before
  the `continue` on the one connection -- and the stop's breakpoint ids
  are known before the program starts. Each of the test's waits now
  says which it was, with the stops made and the program's output
  (`Debugging.awaitEnd`), so that a recurrence names where the run
  stood ([debugger.md](design/debugger.md), "The JetBrains plugin", "Hit
  counts and temporary breakpoints").
- **A debugger test runs close to its event wait under MOVES** (noted
  2026-11-20, with the developer tool's stage 2): `debugger`'s
  `evaluation::a_collection_during_an_evaluation_keeps_the_stop_s_values`
  (reference lowering) hit the 10 s event wait at `tests/debugger.rs:597`
  once in the `stress` gate's MOVES suite, and passed three times alone
  in about 7 s each and in a full rerun of that suite; nothing it runs had
  changed. Its margin under MOVES is too small for a loaded machine: the
  wait, or the work the test makes MOVES move, wants a look.
- **`ns-next load` walks the whole heap for the manifests an image kept**
  (noted 2026-11-19, unmeasured): to warn of an ambiguous `at:version:`
  that `main:args:` makes, as `run` does ([modules.md](design/modules.md),
  decision 4), `load` finds the instances of the manifest's class by one
  walk of the occupied object space (`program::kept_manifests`,
  `Heap::instances_of`), on every load of a project image, also where the
  application kept no manifest. The decoder fills every graph record in its
  graph pass already and could note the instances of the manifest's root
  class there at no extra pass; measure the walk before choosing.
- **The manifest quotes a range with a control or format character raw**
  (noted 2026-11-19): `Manifest.ns`'s `rangeDefect:` follows
  `VersionRange::parse`'s rules but for its first, `check_printable`, so a
  range holding such a character is answered "major `…` is not a number"
  with the character unescaped, where the loader says "holds a control
  character". Only the program's own string reaches the message.
None of these changes a correct program's output; each is wrong but safe, or
reachable only from a crafted artifact, except the first four, which a
program can reach and which end the process, and six a program meets as
a gap: `Object>>printString`'s one
text, `to:by:do:`'s zero step, `String withAll:`'s nil for an argument
that only answers `isKindOfArray` or `isKindOfString`, the refusals of a
program's subclass of `String` as an argument, the error of a
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
  id and path would fix it. [image-format.md](design/image-format.md), "Reading
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
- **An instance of a program's subclass of `String` passes the String
  checks it then fails.** Such an instance holds no code points, and the
  leaves decline it, but `isKindOfString` answers true for it, so the
  members that check an argument with it let it through:
  `StringBuilder>>add:` ends in a `MessageNotUnderstood` of `utf8Size`
  (`String>>utf8Size`'s leaf declines and `super` has none), and
  `String>>startsWith:` reaches `lastIndexOf:startingAt:`, whose
  `ArgumentError` names `lastIndexOf:startingAt:` rather than the
  'startsWith: requires a String argument' its comment says
  (`Kernel.ns`). `String>>indexOf:` refuses it with its own
  `ArgumentError`, as for any argument that is no String. Checked with a
  probe, 2026-11-01; found by a review of the doc comments.
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
- **The core library's breakable lines need the analyzer and `ns-next` to
  be one build.** The plugin bundles `ns-next-analyzer` and takes
  `ns-next` from the settings; the library's files are the runtime's
  shipped sources, and the analyzer answers a file as a shipped module
  only when its text is, byte for byte, one of its own shipped texts. When
  the two binaries are different builds whose shipped sources differ,
  `Kernel.ns` and the others are answered as ordinary modules: their
  breakable lines are approximate, a line breakpoint may be refused where
  the runtime would take it, or offered where it moves. The analyzer
  would need the runtime's shipped sources -- the fingerprint directory
  the plugin shows, or the runtime's binary itself -- to compile a
  library file as that build's shipped module.
  [editor-support.md](design/editor-support.md), "Breakable lines", "The
  core library". Noted 2026-11-15.
- **A project the plugin once took for a Newspeak project keeps its
  core library.** `NewspeakCoreLibrary`'s `newspeak` flag is set when the
  project's files say so, or when a debug session starts, and never reset:
  a project whose `newspeak.json` is deleted, or whose last `.ns` file is
  removed, shows the library until the IDE restarts. The flag would have
  to be decided again at every refresh.
  [editor-support.md](design/editor-support.md), "The core library".
  Noted 2026-11-15.
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
- **A program-made symbol an image carries becomes permanent at load**
  (2026-10-31). The decoder interns every text of an image's symbol
  table, so a symbol a program made, which takes no number and dies with
  its object, takes a number for good once an image carrying it is
  loaded, bounded by the images loaded ([memory.md](design/memory.md),
  "Weak references", "A program-made symbol takes no number and dies
  with its object"). Making it weak there would need the decoder's first
  pass to tell a symbol record's text from a text the code names.
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
- **A caller entered once and compiled by on-stack replacement can be
  abandoned for good in practice** (2026-11-02): while a callee it calls
  in its loop is abandoned, every call is refused, the caller exits at
  each and switches back in at its next backward jump, so it spends its
  own exit budget and is abandoned too. Its revival waits for its
  interpreted entries or for collections, and a caller entered once in a
  run that allocates nothing gets neither, so its loop runs interpreted
  to its end, calling the revived callee's machine code through the call
  stub. `crates/ns-runtime/tests/jit.rs`,
  `a_rare_exit_without_allocation_does_not_end_a_long_run_interpreted`,
  shows it (`run:exitEvery:`).
  [execution.md](design/execution.md), "When a body is compiled".
- **A compaction that frees a live body forgets the optimization budget
  it spent** (2026-11-05): a compaction's freeing by usage -- and the
  zone stress mode's, which frees every body no activation runs -- frees
  bodies that still live, and clears their budget rows
  (`Heap::forget_optimization_budget`, `jit::zone`), so that the next
  compile arms a full budget. A body called a little between every two
  compactions so never runs its budget out, where a full collection
  carries what was left of it over (`jit::budgets_before_collection`).
  Carrying the budget over a compaction as a full collection does would
  close it; `a_full_collection_keeps_what_is_left_of_a_budget` and
  `a_hot_loop_requests_its_body_s_optimization`
  (`crates/ns-runtime/tests/jit.rs`) are exempt under
  `NS_NEXT_STRESS_ZONE` for it, and the exemption's comment names this
  item, so that it is removed, and the tests hold under the zone stress
  mode again, with the fix.
- **Release binaries hold the building machine's paths** (found on
  2026-11-19): `target/release/ns-next.exe` holds 29 absolute paths of the
  build machine, the user's home directory among them -- the panic
  locations of the dependencies under Cargo's registry and of the
  toolchain's library -- and a debug build some 480; the workspace's own
  paths are relative. Cargo's `trim-paths` profile setting would remove
  them, but is unstable in Cargo 1.100 ("feature `trim-paths` is
  required"); `rustc --remap-path-prefix` is stable but takes the
  machine's own prefixes, so it belongs to whatever builds a release, not
  to `.cargo/config.toml`. Nothing builds a release for distribution yet.
- **A module on another drive or share than its project is refused**
  (found on 2026-11-19, while building module identity): a module's
  identity is its path relative to the project
  ([modules.md](design/modules.md), decision 1), and a file on another
  Windows drive or UNC share has none, so `program::read_program` refuses
  it before anything runs ([image-format.md](design/image-format.md),
  "Packaging"; `crates/ns-cli/src/program.rs`,
  `a_module_on_another_drive_has_no_path_in_the_project`). `run-corpus`
  copies each corpus module into its scratch directory beside the
  generated project for that reason. Open is whether such a module needs
  a representation -- an absolute path, which an image would carry as the
  building machine's directory decision 10 keeps out of it, or a name of
  the drive relative to something else -- or whether the refusal is the
  rule; that is decision 1's to settle with the user.

## Missing tests

- **A frame's code under the stress modes** (noted 2026-11-20, with the
  developer tool's stage 2). The pause and step stress modes' readers
  never issue `Command::FrameCode`, so `jit::frame_code_listing`'s check
  that a body assembled again is the body in the zone -- a debug
  build's `debug_assert_eq!` over every byte up to `code_end` -- runs
  only in the debugger's and the DAP agent's few tests, never where the
  stress modes compact the zone, move objects and free bodies. A reader
  that asks every stop's frames for their code would hold the
  reassembly to the zone under every mode
  ([developer-tool.md](design/developer-tool.md), "A stopped frame's
  code"; [debugger.md](design/debugger.md), "The pause stress mode").
  The check is a debug build's only: a release build would pair the
  zone's bytes with the positions of a diverging layout, where an
  answer of `None` on a mismatch would hold in release too.
- **What the reviews of the developer tool's stage 2 left** (noted
  2026-11-20), none of them a wrong listing: no System V or AArch64
  listing is pinned whole, only Windows x64's
  (`a_leafless_body_pins_its_listing`), so another platform's layout is
  held by the lines' shape alone;
  `frame_code_marks_where_a_compiled_caller_returns`
  (`tests/debug_adapter.rs`) asserts only when `machineCode` is present,
  so a change of the JIT's thresholds would leave it checking nothing,
  and it does not assert where the mark stands;
  `an_optimized_copy_lists_its_scopes`'s straight-line check is weaker for the earlier fast paths than for the last; no test
  reaches an inlining listing's arm whose receiver's id a collection
  reclaims before the flush (`name_freed_keys`);
  `a_refused_optimization_prints_its_plan` sizes its zone by
  subtraction, which alignment padding past half the smallest copy
  would make flaky; `record_plan` records nothing, unasserted, when a
  scope's body is missing; an `Inline` arm whose callee is not laid out
  is left out while an `Always` arm the graph does not express is
  listed; and the queue's length counts listings where `recorded` does
  not ([developer-tool.md](design/developer-tool.md), "Stage 2: printing
  code").
- **The trace's rarer events end to end** (noted 2026-11-20, with the
  trace's stage 1). No test runs a program to `[optimize] … not-due`:
  the walk's `recompilable` and `optimization_due` read the same
  settled state, and nothing between them settles a copy, so no
  program reaches it today. Nor to `[osr] … osr-refused` for a reason
  other than `twin-held` (`other-feedback-base`, `no-machine-code`,
  `no-entry`, `native-stack-full`), nor to `[optimize] … dropped` for
  an edit -- the install leaf is reached only through sends, each a
  safepoint that compiles the pending requests first -- or, in a build
  without debug assertions, for a reclaimed id: only a host's
  collection between runs (`Interpreter::collect_now`, debug builds
  only) falls between the run-out that records a request and the
  safepoint that compiles it. These are pinned only by the line
  format's unit test. Nor does a plugin test cover the runtime options
  with the Benchmark command, or a blank field stored as none.
  On a target without a JIT the CLI's trace tests return without
  asserting anything and without saying so; the MCP server's assert
  what a binary without a JIT answers there (`installs: none`), but no
  gate runs a target without a JIT, so those branches have run in no
  gate, one of them only once, forced, while it was written
  ([developer-tool.md](design/developer-tool.md), "Tests").
- **The core library as the IDE draws it** (noted 2026-11-15, with the
  header stops and the library). No `runIde` check was made of the
  "Newspeak Core Library" node under External Libraries -- its name, its
  three modules and `platform/` directly under it -- nor of the library's
  editors being read-only; the tests check the provider, the file index,
  the project view's nodes and the files' writability on disk. A line breakpoint in a
  `LightVirtualFile`, the module a source shown by reference opens when no
  library file has its text, is untested as well. A check under `runIde`
  recorded in [editor-support.md](design/editor-support.md) beside the
  earlier ones would close the first two.
- **The step filters as the IDE draws them** (noted 2026-11-15, with the
  step filters' settings). No `runIde` check was made of the toggle "Step
  Into Kernel and Platform Code" in the debug tool window's ⋮ | Debugger
  Settings, shown for a Newspeak session and hidden for another
  language's, of Newspeak's section on Debugger | Stepping, nor of the
  page "Newspeak Step Filters": its chooser's tree of the project's and
  the core library's files and classes, a node's expansion asking its
  classes and the speed search, Add and Remove, and an invalid entry
  drawn as one. The tests run the action, the configurables and the
  chooser's model directly ([editor-support.md](design/editor-support.md),
  "Plugin testing and gates").
- **The play button's Debug through the platform's own launcher** (noted
  2026-11-15, with the debugger's IDE fixes). `NewspeakRunAction` starts a
  configuration through `ProgramRunnerUtil.executeConfiguration`, and its
  test (`NewspeakRunLineMarkerContributorTest`) replaces that launcher with
  a recording one, so that it shows the Debug executor and the
  configuration Run starts handed over but not that the platform then
  starts the debug session; no `runIde` check of the button's Debug is
  recorded either. A check under `runIde` -- Debug from the play button
  stopping at a line breakpoint -- recorded in
  [editor-support.md](design/editor-support.md) beside the earlier ones,
  or a test that runs the platform's launcher with a stand-in runner,
  would close it.
- **A model test of the poll words' ordering** (noted 2026-11-11, with
  the debugger's first stage). The debugger's agent sets its bit in the
  request word and then stores 0 into `DepthLimit`, both `SeqCst`, from
  its own thread, while the VM's writes of the limit store it, issue a
  `SeqCst` fence and read the request word again
  (`Heap::synchronize_depth_limit`); the argument that no pause is lost
  to a store-buffering reordering rests on that fence. It is held by
  review and by the threaded race test
  `no_pause_is_lost_to_a_racing_depth_limit_write` (`heap.rs`), which
  samples interleavings on the processor that runs it and cannot show
  that none loses the pause; the `aarch64` gate runs it under QEMU's
  user mode, which does not reproduce AArch64's weak ordering. A model
  test of the agent and the VM's two writers in the style of `loom`,
  which explores every interleaving the memory model allows, would; it
  needs `loom` as a dev-dependency, a new external dependency whose
  reason has to survive review, and the machine words behind a seam
  `loom`'s atomics can replace in the test build. It would cover the
  arming lock's argument too, which no test drives into its race
  ([debugger.md](design/debugger.md), "The poll words as atomics", "The
  arming lock", "Testing and gates").
- Road step 8's fifth stage's (noted 2026-11-02 to 2026-11-03): the
  wait of a body whose frame could not switch at a backward jump, or
  whose compile the exhausted zone held (`jit::back_edge_ready`), has no
  test of its own; `a_compiled_call_at_a_requested_safepoint_is_refused`
  (`crates/ns-runtime/tests/jit.rs`) passes with the prologue's separate
  check of the request as with the one compare, a guard against a
  regression only; `compiling_takes_time_that_is_counted` was never seen
  failing; the compile at a backward jump of a body compiled into
  another zone, at once as at an entry, has no test: it needs a frame
  that runs on across its zone's replacement, or a body two
  interpreters run; and the stub tests' harness writes `SafepointRequested` without
  `DepthLimit` (`jit/stubs.rs`), which no test reads today.
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

- The inlining plan's decisions are collected by every optimizing compile
  (noted 2026-11-19): `optimizing::compile_tree_explained` fills a
  `PlanDecisions` -- small `Vec`s of the scopes the plan inlined and
  the callees it declined, and the laid-out scopes and the type-case
  arms, sorted, that `--print-inlining` lists -- whether the `inlining`
  category is traced, or the plan printed, or not, and drops it unread
  when not. A flag
  telling the planner to keep its decisions would save the allocation,
  once per optimization ([developer-tool.md](design/developer-tool.md),
  "The events and their lines").
- The codegen gate does not see what fat LTO inlines (noted 2026-11-16).
  `xtask/src/codegen.rs` reads `cargo asm`'s listing of `ns-runtime`'s
  lib, which is compiled without the binaries' cross-crate LTO. Under
  LLVM 23, `builtins::execute` saved two XMM registers in that listing
  and all ten in `ns-next.exe`, so the gate passed a 3.4% slowdown of
  Json that only a measurement found ([execution.md](design/execution.md),
  "What Rust 1.100.0 moved"). Holding the binary's listing, and counting
  a hot function's saved XMM registers, would catch it.
- The AArch64 encoder's sources are unfetched (noted 2026-11-03): every
  constant of `crates/ns-assembler/src/aarch64.rs` cites a
  support.arm.com page, as the convention asks, but the pages' paths
  were written from the site's naming scheme and never opened, so a
  citation may name a page that does not exist; `tests/encoding_sources.rs`
  checks only the prefix.
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

- The developer tool's stage 2 leaves two small things in its code
  (noted 2026-11-20, in the final review): `Leaf::ALL`'s const assertion
  (`jit/leaves.rs`) does not catch a leaf added after `Budget`, which
  would compile and be listed as `leaf unknown`, so it is tied to the
  leaf table's own count; and `AlwaysInlined::name` answers `primitive`
  for a primitive kind no leaf takes by a fallback arm rather than an
  exhaustive one. An `OutOfLine` position may also stand before
  alignment padding.

## Goldens and documentation

- **A golden cannot name the stress modes it is written for**
  (2026-11-08). Only a directory binds a mode today: every golden under
  `goldens/runtime/optimizer/` runs a second time in a debug build as
  `golden::optimizer::<name>` with `NS_NEXT_STRESS_OPTIMIZE=1`
  ([README.md](design/README.md), "The goldens"); every other mode
  reaches a golden only through the `stress` gate at the end of a batch.
  A golden written for a mode -- an exit inside an inlined scope for
  `DEOPT`, a freed copy for `INVALIDATE`, a moved frame for `MOVES` --
  should say so in its `.toml` (a key such as `stress_modes = ["DEOPT",
  "INVALIDATE"]`), and the default suite in a debug build should run it
  under each mode it names, so that a defect in the very scenario it pins
  is found at the commit, not at the batch's end, and which golden pins
  which mode is recorded rather than left to comments. It adds to the
  `stress` gate's run of every golden under every mode, never replaces
  it: most defects the modes found were in code not written with that
  mode in mind (24 `fix:` commits name a mode, a lower bound, since a
  defect found during a feature is fixed in its own commit). There is no
  opt-out key: a golden's outcome is the same under every mode, and a
  mode that makes its scenario impossible still checks its output. The
  directory rule then goes: each of the 33 goldens under
  `goldens/runtime/optimizer/` names `OPTIMIZE` (several also `DEOPT` or
  `INVALIDATE`), `crates/ns-cli/tests/run_goldens.rs` drops its prefix
  test, and a test holds every golden under that directory to naming
  `OPTIMIZE`, so that a new one cannot lose the run by forgetting the key.

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
