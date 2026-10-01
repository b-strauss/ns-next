# ns-next Progress

Milestone chronology: what was completed and when. **How the system is designed,
and how much of each aspect exists, is in `docs/design/` — start at its
[README](design/README.md).** This file is the schedule, not the design.

| Milestone | Status | Completed | Plan |
|---|---|---|---|
| M1 — Frontend | Complete | 2026-09-01 | [frontend](superpowers/plans/2026-09-01-2308-ns-next-frontend.md) |
| M1 — Runtime | Complete | 2026-09-04 | [runtime](superpowers/plans/2026-09-04-1351-ns-next-runtime.md) |
| M2 — Bytecode + VM | Complete | 2026-09-05 | [bytecode-vm](superpowers/plans/2026-09-04-2353-ns-next-bytecode-vm.md) |
| M3 — Nested classes + source-free images | Complete | 2026-09-07 | [nesting/mixins/source-free-images](superpowers/plans/2026-09-06-1720-ns-next-nesting-mixins-source-free-images.md) |
| M4 — Representation: arena, checked roots, stack activations, a moving collector | Complete | 2026-09-20 | [heap-seam](superpowers/plans/2026-09-08-2034-ns-next-heap-seam.md), [object-and-frame-arena](superpowers/plans/2026-09-09-1952-ns-next-object-and-frame-arena.md), [activation-stack](superpowers/plans/2026-09-10-2145-ns-next-activation-stack.md), [retire-tree-walker](superpowers/plans/2026-09-11-1848-ns-next-retire-tree-walker.md), [clean-up-with-one-engine](superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md), [reference-lowering](superpowers/plans/2026-09-12-1157-ns-next-reference-lowering.md), [tag-named-identity-hashes](superpowers/plans/2026-09-12-1604-ns-next-tag-named-identity-hashes.md), [trampoline](superpowers/plans/2026-09-12-1619-ns-next-trampoline.md), [no-reentry](superpowers/plans/2026-09-13-0149-ns-next-no-reentry.md), [checked-roots-2b](superpowers/plans/2026-09-13-1748-ns-next-checked-roots-2b.md), [lifetime-brand-2c](superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md), [spec-conformant-lookup](superpowers/plans/2026-09-15-0108-ns-next-spec-conformant-lookup.md), [primitive-method-entries-3a](superpowers/plans/2026-09-15-2345-ns-next-primitive-method-entries-3a.md), [inline-caches-3b](superpowers/plans/2026-09-16-0950-ns-next-inline-caches-3b.md), [flat-metaclasses-factory-and-cache-refinements](superpowers/plans/2026-09-18-0211-ns-next-flat-metaclasses-factory-and-cache-refinements.md), [goldens](superpowers/plans/2026-09-18-1355-ns-next-goldens.md), [dispatch-id-place-3c](superpowers/plans/2026-09-18-2215-ns-next-dispatch-id-place-3c.md), [specification-gaps](superpowers/plans/2026-09-19-0300-ns-next-specification-gaps.md), [remove-assign](superpowers/plans/2026-09-19-2125-ns-next-remove-assign.md), [closure-conversion](superpowers/plans/2026-09-19-2344-ns-next-closure-conversion.md), [moving-collector](superpowers/plans/2026-09-20-1746-ns-next-moving-collector.md) |
| M5–M6 — JIT | Not started | — | — |

## Where the next session starts

**Next: the Are We Fast Yet benchmarks, stage 3, before road step 2's
second plan** ([OPEN-WORK.md](OPEN-WORK.md), "The road to a
high-performance VM"): DeltaBlue, Havlak, Json and CD from
`prior-art/somns/core-lib/Benchmarks` join the eleven programs of stages 1 and
2 in `benchmarks/are-we-fast-yet/` ("Are We Fast Yet, stage 1" and "stage 2"
below; [execution.md](design/execution.md), "The Are We Fast Yet
benchmarks"), widening the kernel and the platform on demand. Its plan is
still to be written, and whether it follows Primordial Soup's protocol, as
stages 1 and 2 do, is for that plan to settle. Decided with the user on
2026-09-29, so that the eight-byte value and every later step are measured
on real programs.

**After it: road step 2's second plan, the eight-byte value**
([object-model.md](design/object-model.md), "Planned: an eight-byte value"):
one tagged 64-bit word with a 61-bit `SmallInteger`, on today's arena. Its
first plan, large integers, is built ("Large integers" below;
[core-library.md](design/core-library.md), "Large integers"), so the word
moves the `SmallInteger`/`LargeInteger` boundary rather than introducing it.
The whole road, decided with the user on 2026-09-29, is in
[OPEN-WORK.md](OPEN-WORK.md), "The road to a high-performance VM".

**Everything else still to be done is listed in [OPEN-WORK.md](OPEN-WORK.md)**,
each item with where it is explained. The sections below record the
milestones, oldest first.

**Measuring.** The protocol is [execution.md](design/execution.md)'s "What
removing the re-entries moved"; every compared binary, the current tree
included, is built with the same explicit `RUSTFLAGS` (`CLAUDE.md`,
"Working in this repository"). Four of the eight measurement sources are
goldens under `goldens/bench/`; the other four -- `bench_new.ns`,
`bench_new2.ns`, `bench_newarg.ns` and `bench_do.ns` -- are not in the tree
and must be written out from [execution.md](design/execution.md), "What
spec-conformant lookup moved", never reconstructed from a prose
description: a reconstruction runs and prints the right answer while
measuring something else. The Are We Fast Yet benchmarks are in the tree
and measured as [execution.md](design/execution.md), "The Are We Fast Yet
benchmarks", describes.

## Notes

**Milestone renumbering (2026-09-06).** Language completeness was pulled ahead of
the performance work, so M3 became nested classes and source-free images, inline
caches and the GC became M4, and the JIT tiers became M5–M6. Plans written before
that date use the old numbering.

## M1 — Frontend (2026-09-01)

`ns-syntax`: hand-written lexer, recursive-descent and Pratt parser, and AST for
the official Newspeak3 textual syntax, plus `ns-next parse` and `parse-corpus`.
Validated against the vendored corpus at 210 of 211 files; the last one
parses since "The syntax is the specification's grammar" (2026-09-27).

## M1 — Runtime (2026-09-04)

`ns-runtime`: name resolver implementing Newspeak's real lexical, `outer` and
`super` scoping; the object model; the three access-control-aware dispatch
algorithms; a tree-walking interpreter with reified heap-reachable activations;
class instantiation; a small Rust-native core library; and `ns-next run`.
57 commits.

## M2 — Bytecode + VM (2026-09-05)

A register-based bytecode compiler and VM, with the M1 tree-walker kept
permanently as `--engine=tree-walk` — a byte-for-byte differential oracle over
stdout, stderr and exit status. Adds baseline control-flow inlining, each
occurrence guarded at run time with a fallback to a real virtual send;
`ns-next disasm`; and the first image format, exposed as `snapshot` and `load`.
Carries the statement→program-counter safepoint table and a reserved inert inline-cache slot
per send from the start, so Milestone 4 needs no bytecode format break.

A whole-branch review before the merge found two engine divergences the per-task
reviews had missed — a local's register aliased as an expression's value, and a
bare `super` panicking the compiler where the tree-walker raises a Newspeak
error. Both were fixed on the branch.

## M3 — Nested classes + source-free images (2026-09-07)

Replaces the Milestone 1 class model with the three-layer model §6 requires — a
shared mixin per declaration, an application cached per (mixin, superclass
application), and a class object per enclosing instance — and makes the image
format source-free: a symbol table and a mixin table replace the embedded source
text and the re-parse on load. Class sides become metaclass behaviour, the
superclass clause is evaluated, and each inheritance level's initializer runs in
its own activation.

Adds the `run-corpus` gate over `corpus/runnable.toml`: real corpus modules run
end to end under both engines and checked against recorded output — **ran 6,
failed 0, skipped 205**, with `parse-corpus` unchanged at **210/0/1/0**. Adds
`ns-next bench`: on a release build, over a 50,000-iteration loop, bytecode
median ~19.6 ms against tree-walk ~37.4 ms — not a pure dispatch comparison,
since the compiler inlines `whileTrue:`, but the bytecode number is Milestone 4's
before-figure.

Six silent defects were found by review that the green suite never showed; four
of them were identically wrong under both engines, where the differential harness
is structurally blind.

## Artifact inspection and the stateless binary (2026-09-08)

Plan: [artifact-inspection-and-stateless-binary](superpowers/plans/2026-09-08-0044-ns-next-artifact-inspection-and-stateless-binary.md).

Finishes the deployment-artifact half of M3's image-format work rather than
opening a milestone of its own. `ns-next build` compiles a module into a
stateless binary — code and structure, no instance state — which `load` starts
by instantiating against its own loading platform and sending the entry the
artifact recorded. `load` now also refuses trailing bytes after an artifact's
last section, closing the confusion vector two artifact shapes sharing one
format would otherwise open.

An artifact is also readable as text, which it was not before. `disasm` accepts
one in place of a `.ns` file, decoding rather than compiling — the path a
deployed binary needs, since it ships no source. `ns-next inspect` prints the
file: header flags, section offsets, the symbol table, the code table with
every body attributed to the declaration that owns it, and the mixin table.
`--full` prints everything, in the sense `javap -v` does — mixin rows expanded
and every body disassembled — and for an image it walks the object graph record
by record rather than rebuilding it, so a viewer never bootstraps an
interpreter. The graph section's five wire-tag families moved into
`image/opcodes.rs` first, under the same assigned-once rule as the instruction
tags, so that third reader cannot disagree with the loader about what a byte
means.

Gates: `build` then `load` reproduces `run --engine=bytecode`'s stdout,
stderr and exit status byte for byte over every fixture; `build` and
`snapshot` agree on their shared sections' bytes; `run-corpus`'s six entries
now also run a third way, built and loaded — **ran 6, failed 0, skipped
205**, with `parse-corpus` unchanged at **210/0/1/0**; and mutation and
truncation tests cover the new header flag, the entry field, and the
trailing-byte check.

## M4 — The heap seam (2026-09-09)

Plan: [ns-next-heap-seam](superpowers/plans/2026-09-08-2034-ns-next-heap-seam.md).

The first piece of M4's representation change, done ahead of the arena
itself: every read and write of an `Object` or a `Frame` across the workspace
now goes through one `Heap` (`crates/ns-runtime/src/heap.rs`), replacing the
193 call sites (109 under `src`, 84 in the integration tests) that used to
reach `Rc<RefCell<_>>` directly. `ObjectReference` and `FrameReference` became opaque
newtypes with a `pub(crate)` inner field: the crate boundary was
compiler-enforced (`ns-cli` and the integration-test crates under
`crates/ns-runtime/tests/` could not reach it at all), while containment
inside `ns-runtime`'s own `src/` was the grep gate this migration ran
throughout, since `pub(crate)` is visible from every module in the crate.
The arena below closed that half too, by moving the handles into `heap.rs`
with a private field. Each handle also gained a hand-written, non-recursive
`Debug` that prints identity only.

It did not come free. Migrating `interpreter/eval.rs` turned
`dispatch_lexical_send`'s single borrow yielding both the receiver and the
current class into two separate accessor calls, and at roughly 250,000
lexical sends per benchmark run that measured as a real 13% regression —
`bench_loop.ns` from 20.07 ms to 22.77 ms. A batched accessor
(`receiver_and_current_class`) recovered it in full. The companion result —
the identical extra-borrow shape in `dispatch::walk`'s per-inheritance-level
lookup cost nothing measurable — is what turned the incident into a lesson
rather than a scare: the cost is executions per run, not borrows being
expensive. See [execution.md](design/execution.md)'s measurements section.

Confirmed by measurement, not assumption: three fresh release-build runs of
`bench_loop.ns` after the full migration gave bytecode medians of 19.71,
19.59 and 19.72 ms, against the pre-plan session baseline of 19.91 ms (min
19.80, max 20.07) — within noise, so the seam costs nothing once the one
regression it introduced was found and fixed.

## M4 — The object and frame arena (2026-09-09)

Plan: [ns-next-object-and-frame-arena](superpowers/plans/2026-09-09-1952-ns-next-object-and-frame-arena.md).

`Rc<RefCell<_>>` is gone. One `Heap` owns `objects: Vec<Object>` and
`frames: Vec<Frame>`; an `ObjectReference`, a `FrameReference` and the `ClassReference` alias are
32-bit indices into them, declared in `crates/ns-runtime/src/heap.rs` with a
**private** field — so the compiler, not a grep gate, refuses every read of a
handle from anywhere else. The grep gate `CLAUDE.md` used to carry is removed.
`ObjectReference`, `FrameReference` and `Value` are now `Copy`. Converted in two steps —
activations first, then objects — so that each landed against a green suite and
a measurable benchmark rather than as one unreviewable change.

The seam paid for itself exactly as designed: **no call site outside `heap.rs`
and `types.rs` changed**, for the frame conversion or the object one.

What it deleted rather than moved:

- `types.rs`'s ~176-line iterative-teardown section — `Dying`, the four `push_*`
  helpers, `drain_object_data`, `drain_frame`, `take_object_data`,
  `run_teardown`, and `impl Drop` for both `Object` and `Frame`. An arena entry
  holds indices, so there is no reference chain to unwind.
  `mixin.rs`'s `impl Drop for MixinApplication` remains, and is now the only
  hand-written iterative `Drop` in the crate — not the only recursive graph:
  `Mixin`'s `nested`/`class_side` links still drop recursively, bounded by
  `image/mixins.rs`'s `MAX_MIXIN_DEPTH` rather than by iteration.
- `bootstrap_self_referential_object`'s `Rc::new_uninit`/`Rc::from_raw`
  construction, then the crate's only `unsafe`. An arena knows a slot's index
  before it fills the slot, so an object can name itself in safe Rust.
  `grep -rn "unsafe" crates/ns-runtime/src/heap.rs` is empty, and since 2c
  the whole crate forbids `unsafe` code.

What it added: a `debug_assertions`-only origin guard. Each `Heap` carries an id,
each handle records the id of the heap that minted it, and all 46 accessor sites
assert the two match. This exists because reading a handle through the wrong heap
is the one new failure mode the arena introduces and it is *silent* — an index
from another heap is in range and names a different, perfectly valid object. In a
release build neither the field nor the check exists, so a handle is exactly its
32-bit index. The same reasoning retired a proof `interpreter/nested.rs` had been
relying on: `cache_slot` used `0` as the sentinel for a non-object receiver,
justified by "0 is an address no live `Rc` has", and under the arena index 0 is
the first object allocated. It now returns `Option<(ObjectReference, usize)>`, so the case
cannot be expressed rather than being commented differently.

Measured before and after in one sitting, on a release build, alternating three
runs of each binary — the "before" built from this branch's merge-base in a
throwaway worktree, because a figure carried across sessions measures the
machine's load rather than the code:

| | bytecode medians | tree-walk medians |
|---|---|---|
| Before | 19.38, 19.54, 19.60 ms | 37.04, 37.50, 37.13 ms |
| After | 16.45, 16.82, 16.53 ms | 36.78, 37.23, 37.12 ms |

**About 15% off the bytecode engine**, with the two groups not overlapping. What
moved is the `Rc` box, its reference-count traffic, and the `RefCell` borrow on
every register read and write. What did **not** move is the rest of the
activation cost — `arguments` and `temporaries` were still a `Vec` allocation
apiece per activation, which the activation stack took next. The tree-walker was
unchanged within noise, as expected.

Until the moving collector (2026-09-20) nothing reclaimed a slot, so Windows
peak working set over `bench_loop.ns` went
from 4.6 MB to 25.4 MB for one tree-walk run, and from 4.6 MB to 340.7 MB for
`bench`'s twenty-two entry sends. Retention is proportional to every activation
ever created, not to what is live. The tree-walk maximum in the same ten timed
runs also went from ~38 ms to ~117 ms, which the arena's `Vec` doubling would
explain — an inference from the shape of the growth curve rather than a measured
cause, and one the next step contradicted for this benchmark (below). The
conditions the peak figures were taken under are in [memory.md](design/memory.md),
"The interim cost, and the one question still open"; the tree-walk figures,
retired with the engine, are recorded only here.

## M4 — The activation stack (2026-09-10)

Plan: [ns-next-activation-stack](superpowers/plans/2026-09-10-2145-ns-next-activation-stack.md).

Step 1a of M4. Every Newspeak activation runs on one activation stack the `Heap`
owns — `ActivationStack` in `crates/ns-runtime/src/heap.rs`, one contiguous
`Vec<Value>` of registers and one `Vec<StackEntry>` — addressed by a new handle,
`ActivationReference`, which is 4 bytes in a release build. A `Frame` exists only when
a block is created: `Heap::reify` builds one *married* to its stack entry, so a
block reads its method's variables through the live stack slot, and
`Heap::pop_activation` *divorces* it when the activation returns, copying the
registers out, because ns-next's blocks read those variables by reference. The
frame arena holds `FrameSlot`s — a `Frame` plus the entry it is married to,
private to `heap.rs`. The four sites that build an activation (`invoke_method`,
`invoke_closure`, `run_initializer_level`, `run_clause_body`) push and pop under
both engines, and `Interpreter::send` and `instantiate` debug-assert that they
leave the stack balanced. Every function that means *the running activation*
takes an `ActivationReference`; closures, frames and image records take only a
`FrameReference`, so storing a stack handle is a compile error. Accessors are
`activation_*` for a live activation and `frame_*` for a reified frame. The
design is in [execution.md](design/execution.md), "Activations".

What it deleted rather than moved:

- The `sender` parameter of `Interpreter::send`, `invoke_method`,
  `invoke_closure`, `builtins::dispatch_native`, `builtins::call_block` and the
  `builtins/` modules. The design had it retyped to an `ActivationReference`; it was
  removed instead, because its only use was to fill `Frame.sender`, and once a
  live activation's sender is the entry below it, nothing read it.
- `FrameReference`'s `receiver_and_current_class`, `local`, `set_local`,
  `frame_slot_base` and `frame_is_method`, replaced by their `activation_*`
  counterparts, and `mark_uncontinuable`, whose job is now part of
  `pop_activation`'s divorce; `receiver` and `grow_temps` became
  `frame_receiver` and `frame_grow_temporaries`. Outside tests, `Heap::allocate_frame`
  has two callers left, `reify` and the image decoder.
- An arena entry and two register vectors per activation. An activation that
  nothing captures allocates nothing once the stack has grown to the program's
  depth, and leaves nothing behind when it returns.

Two observable changes, both identical under both engines:

1. A frame whose activation has returned has no sender — Smalltalk's rule, and
   Cog's and Primordial Soup's — so a snapshot no longer carries its closures'
   callers.
2. A block created during initialization whose `^` runs after initialization has
   finished raises `cannotReturn`, as `newspeak-spec.pdf` §7.2 requires,
   instead of escaping as "internal: non-local return escaped the entry send". A
   `^` into an initializer that is still running still escapes.

`tests-fixtures/closure_variables.ns` landed ahead of the change as its guard —
blocks sharing, and outliving, their method's variables, under both engines —
and `bench_send.ns` and `bench_block.ns` after it. 604 tests pass, with one
ignored; `parse-corpus` stays at **210/0/1/0** and `run-corpus` at
**6/0/205**.

Measured before and after in one sitting on a release build, the "before" built
from the branch's merge-base — the arena — in a throwaway worktree: three rounds
of `ns-next bench` over five sources, alternating the two binaries within each
round. Rows one and two of the send decomposition are scratch sources that
differ from `bench_send.ns` by one statement each. Medians in ms, rounds 1–3:

| Source | Bytecode, before | Bytecode, after | Tree-walk, before | Tree-walk, after |
|---|---|---|---|---|
| `bench_loop.ns` | 16.27, 16.50, 16.43 | 16.07, 16.22, 16.57 | 37.83, 37.12, 38.29 | 31.85, 32.24, 32.53 |
| `bench_send.ns` | 23.49, 23.46, 23.58 | 20.68, 20.51, 20.51 | 40.64, 40.96, 41.14 | 37.41, 37.51, 37.49 |
| `bench_block.ns` | 38.83, 38.99, 39.16 | 38.46, 40.84, 39.24 | 75.16, 74.81, 75.74 | 75.32, 74.98, 74.74 |
| send row one | 16.16, 16.18, 16.24 | 15.97, 15.98, 16.26 | 34.85, 35.52, 34.63 | 31.87, 32.14, 32.14 |
| send row two | 19.18, 20.07, 19.28 | 17.84, 17.83, 17.90 | 37.00, 37.24, 38.29 | 33.64, 34.02, 33.67 |

**A user-method send under the bytecode engine fell from ~146 ns to ~91 ns,
about 38%** (`bench_send.ns` minus row one, per iteration, from the middle
medians), and what it costs beyond a native send from ~62 ns to ~37 ns.
`bench_send.ns` itself went from 23.5 ms to 20.5 ms. **The tree-walker's
`bench_loop.ns` fell about 15%**, from 37.8 ms to 32.2 ms, because it does not
inline `whileTrue:` and each iteration's two block activations are now pushes
rather than arena entries. The table with ns per iteration, the re-taken
decomposition and one caveat it turned up — under the arena, row two's method
had no temporaries and so skipped an allocation row three paid — are in
[execution.md](design/execution.md), "What the activation stack moved".

Peak working set, Windows 11, release build, read through
`psapi!GetProcessMemoryInfo` on the still-open process handle:

| Command | Before | After |
|---|---|---|
| `run --engine=bytecode`, `bench_loop.ns` | 4.6 MB | 4.6 MB |
| `run --engine=tree-walk`, `bench_loop.ns` | 25.4 MB | 4.6 MB |
| `bench --iterations 10`, `bench_loop.ns` | 340.7 MB | 4.6 MB |
| `bench --iterations 10`, `bench_send.ns` | 324.9 MB | 4.6 MB |
| `bench --iterations 10`, `bench_block.ns` | 966.9 MB | 446.0 MB |

The tree-walk and `bench` figures fell all the way to the 4.6 MB baseline, as the
design predicted: an activation nothing reified leaves nothing behind.
`bench_block.ns` still grows, as expected — every iteration reifies one frame
and allocates one closure, and nothing reclaims an arena slot — but by less than
half as much, since the block's own activation no longer takes an arena entry.

What did **not** move: the bytecode engine's `bench_loop.ns`, whose inlined
loop builds no activation; `bench_block.ns`'s time under either engine — the
stack took the block's own activation out of the arena, and what offsets that
saving was attributed later ("M4 — The trampoline"); and three costs the design
assigned to later steps — the argument vector `collect` built for every send
and recursion on the Rust stack with no depth limit, both taken by the
trampoline, and the `HashMap` lookup per invocation that found a compiled body,
taken by the inline caches. And the tree-walker's ~112 ms maximum on `bench_loop.ns`, which
the arena's `Vec` doubling was thought to explain, survived under a binary that
retains nothing for that benchmark — 4.6 MB across all twenty-two entry sends —
so that explanation does not hold for it; its cause was never found. What the
bytecode maxima do and do not show is in [memory.md](design/memory.md), "The
interim cost, and the one question still open".

## M4 — Retiring the tree-walker (2026-09-11)

Plan: [ns-next-retire-tree-walker](superpowers/plans/2026-09-11-1848-ns-next-retire-tree-walker.md).

Step A of the three inserted ahead of 1b. The bytecode VM is the only engine;
why the tree-walker went, and what protects behaviour until the reference
lowering exists, is in [execution.md](design/execution.md), "One engine".

**What was harvested first.** Loading a module now compiles and installs its
code (`load_module`; a second load adds to the first's), which moved every
runtime test that never installed an engine — and so had run under the
tree-walker by default — onto the VM. Exactly four tests failed, all four as
predicted and none a VM divergence: two image tests that deliberately built
tree-walker closures, one `vm.rs` test asserting the tree-walker was the
default, and `instantiate.rs`'s two-module test, which needed the second load's
code added rather than replacing the first's. Tests whose point is a
control-flow native now also pass blocks through variables, which the compiler
does not inline, so the native path stays covered. Helpers that took a
`bytecode: bool` ran the VM for both halves from then on; their agreement had
been established before, when each half ran its own engine.

Then `differential.rs`'s inline cases became thirteen golden outcomes in
`crates/ns-cli/tests/program_outcomes.rs` — exit status, stdout and stderr,
captured from both engines, which agreed on every one — and
`run_fixtures.rs` now also pins each fixture's empty stderr. **No divergence
was found anywhere in the harvest.** A review probe on the pre-deletion code,
with the tree-walker switched on explicitly, found that a `super`-prefixed
superclass clause answers a class object under both engines: the test doc that
claimed it was an error was wrong, and was corrected.

**What was deleted.**

- From the CLI: `--engine`, the tree-walk line of `bench`, and
  `crates/ns-cli/tests/differential.rs`. `run-corpus` runs each row from
  source and as a built binary.
- From the image format: the frame record's program-counter-kind byte, so the minimum frame
  record is 18 bytes rather than 19. `PROGRAM_COUNTER_KIND_*` retired its two numbers,
  which stay pinned. A one-frame record in the old layout is refused
  (`crates/ns-runtime/tests/image.rs`,
  `a_frame_record_in_the_old_layout_is_refused`); in a multi-frame image the
  misaligned stream is very likely, but not provably, refused.
- From the runtime: `Engine`, `set_engine`, `EngineUnavailable`,
  `decoded_from_image`, the AST evaluator, the tree-walk halves of the three
  body-execution hooks, `ClosureBody`, `PcKind`, `run_own_slots` and
  `run_own_init_exprs`, and `Heap::activation_is_method`. The `Interpreter`
  holds one code table. `crates/ns-runtime/tests/interpreter.rs`'s tests,
  which built their classes by hand, now load real modules, and 18
  `_under_bytecode` duplicates in `nested_classes.rs` folded into the tests
  they duplicated.
- The resolver's bindings map from every run-time signature: `send(receiver,
  selector, arguments)`, `instantiate(class, arguments)`,
  `invoke_closure(interpreter, closure, arguments)`,
  `run_code(interpreter, activation, code)`. `ClosureData`,
  `CompiledModule` and `LoadedImage` no longer carry one.

**What replaced the differential** was recorded answers: every fixture
against its `.expected` stdout and an empty stderr, the golden outcomes, and
`run-corpus` — `CLAUDE.md`'s "Program outcomes are pinned, not re-derived".
Step C's reference lowering became the differential oracle; see "M4 — The
reference lowering" below.

Measured before and after in one sitting on a release build, the "before" built
from the branch's merge-base in a throwaway worktree: three rounds of
`ns-next bench` over the three fixtures, alternating the two binaries within
each round. Bytecode medians in ms, rounds 1–3:

| Source | Before | After |
|---|---|---|
| `bench_loop.ns` | 16.58, 16.53, 16.43 | 16.57, 16.38, 16.51 |
| `bench_send.ns` | 20.75, 20.68, 20.83 | 20.67, 20.48, 20.44 |
| `bench_block.ns` | 39.40, 39.91, 39.04 | 37.65, 37.61, 38.48 |

Nothing got slower. `bench_loop.ns` did not move and `bench_send.ns` moved
about 1%, at the edge of noise. `bench_block.ns`, which creates and invokes a
block every iteration, got about 4% faster: a closure no longer carries a
bindings `Rc` or a body enum, and an `Object` shrank from 64 to 56 bytes in a
release build (80 in a debug one, from 88) — an inference from what was
removed, not a profile. The smaller `Object` and the one-byte-shorter frame
record also lower the image decoder's worst-case amplification, from ~17.8x to
~16.9x in a release build and from ~22.2x to ~21.3x in a debug one
([memory.md](design/memory.md)).

583 tests pass, with one ignored; `parse-corpus` stays at **210/0/1/0** and
`run-corpus` at **6/0/205**. Step B cleared what the retirement left.

## M4 — Cleaning up with one engine (2026-09-12)

Plan: [ns-next-clean-up-with-one-engine](superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md).

Step B of the three inserted ahead of 1b: behaviour-preserving cleanup now that
no code serves a second engine, three measured hot-path changes, and one design
change. No `.expected` fixture, golden outcome, `corpus/runnable.toml` row or
snapshot changed.

**Two decisions**, settled with the user on 2026-09-11 and grounded in
`prior-art/`:

1. **Compiled code lives on the declaration it belongs to** — a method's on its
   `MixinMethod`, an initializer's and a superclass clause's on the `Mixin` —
   instead of in the interpreter's `HashMap`s keyed by `Rc` address. Every
   system in `prior-art/` keeps a method's code on the method: Primordial Soup's
   `Method::bytecode()`, SOMns's `SInvokable.invokable`, Squeak's
   `CompiledMethod` in the method dictionary. It removed a hashed lookup from
   every user-method send and every initializing level of an instantiation,
   the collector hazard of a table keyed by `Rc` address, and the possibility
   of a method with no compiled body. This one is a design change, made with
   its documents ([compiler.md](design/compiler.md), "Compiled code").
2. **A native is bound as an enumerated primitive, not a function pointer** —
   decided here, built with the inline caches. Native dispatch keeps its
   `match selector.as_str()` until then; a lookup that reaches a native will
   bind a Rust enum naming the operation, a native inline-cache entry will
   carry it, and the JIT can emit it inline, where a function pointer could
   only be called — the ceiling a Rust callback imposes. That is Cog's and
   Primordial Soup's model: a method's primitive number indexes the
   interpreter's primitive table and Cog's `primitiveGeneratorTable`, whose
   generators (`genPrimitiveAdd`) emit machine code, and
   `genSpecialSelectorArithmetic` emits SmallInteger arithmetic inline at the
   send site. So B pre-interned only the symbols the runtime names in its own
   code. Recorded in [dispatch.md](design/dispatch.md), "Inline
   caches", and [core-library.md](design/core-library.md).

**What changed**, in the plan's order:

1. **Tests pin exact output.** Each fixture's stdout is compared byte for byte,
   not after `trim_end()`; the retirement's leftover `_under_bytecode` names,
   dead code and one-caller helpers are gone, and the malformed-integer test
   pins its error's selector again.
2. **`interpreter/` without the tree-walker's leftovers.** `eval.rs` is split
   by what it held into `sends.rs` (lexical and super sends), `locals.rs`
   (local and outer-local access) and `literals.rs` (the errors a deferred
   literal raises); `invoke_closure` is an `Interpreter` method.
3. **Builtin classes by index.** A `Vec` indexed by `BuiltinClass`
   replaces a hashed map; measured faster on all three benchmarks.
4. **The runtime's own symbols interned once**, as `Symbol` constants seeded
   into the interner, instead of an intern at every call site; measured
   unchanged, since no benchmark reaches those sites in its timed send.
5. **A closure behind an `Rc`.** `ObjectData::Closure` holds an
   `Rc<ClosureData>`, and its parameter names are shared with the instruction
   that made it, so invoking a block bumps a reference count instead of
   copying the closure; `bench_block.ns` about 17% faster. Two encoders that
   could never fail, `encode_closure_record` and `encode_frame`, lost their
   `Result`. `Object` stayed 56 bytes in a release build (80 in a debug one),
   so the image decoder's amplification figures stand.
6. **Compiled code on the declaration** (decision 1). Decoding an image
   attaches each body a mixin-table row names, and `image::decode::verify_mixins`
   refuses a declaration whose body is missing or the wrong shape before
   anything is built on the code. Its review found a panic reachable from a
   crafted artifact: `disasm --initializer` on an artifact whose root row names
   no initializer now refuses with an error, pinned by a CLI test.
7. **Handles by value.** Every function taking an `ObjectReference`, `ClassReference`,
   `FrameReference` or `ActivationReference` takes it by value — 143 sites — and
   `clippy::trivially_copy_pass_by_ref`, sized by a new `clippy.toml` to see
   the debug-build `ActivationReference` and `pub` functions too, keeps it so. Four
   `Option<&_>` parameters remain by reference, outside the lint's reach
   (`dispatch.rs`, `decode.rs`, `encode.rs` ×2).
8. **One module per CLI command.** `ns-cli/src/main.rs` keeps the argument
   parser; each command is a module under `commands/`, shared loading is
   `source.rs`, and the entry send is `entry.rs`, whose `instantiate_root`
   replaces three copies of the same instantiation.
9. **Comments state decisions, not history.** A sweep over `ns-runtime`,
   `ns-cli` and `Cargo.toml` took 127 lines that narrated milestones, reviews
   or what code used to do down to 8 deliberate keeps (two error strings a
   program observes, a
   retired tag's name and its pins, a citation, a Newspeak slot name, and one
   "used to" meaning "used in order to"). `Cargo.toml`'s release-profile
   comment now gives LTO's real reason: `ns-cli` calls into two other crates.

**What was deleted:** `interpreter/eval.rs`, `raise_return_pub`,
`Interpreter::new` (and `Default`), `CompiledModule`, `method_key`,
`compile_module`, `compile_initializer`, `install_code`, `build_module`, and
three tests whose property is now structural — two image tests of a method
without a body and `bytecode.rs`'s test of the `Rc`-address key.

Measured against `master` in one sitting on a release build, the "before"
built from the branch's merge-base in a throwaway worktree: three rounds of
`ns-next bench` over the three fixtures, alternating the two binaries within
each round. Bytecode medians in ms, rounds 1–3:

| Source | Before | After |
|---|---|---|
| `bench_loop.ns` | 16.08, 16.17, 16.12 | 15.31, 15.32, 15.49 |
| `bench_send.ns` | 20.55, 20.28, 19.95 | 18.65, 19.06, 19.15 |
| `bench_block.ns` | 37.38, 38.29, 38.68 | 29.94, 29.74, 30.02 |

On every source the after-group lies entirely below the before-group:
`bench_loop.ns` about 5% faster, `bench_send.ns` about 6%, **`bench_block.ns`
about 22%**, from ~38.3 ms to ~29.9 ms. Each hot-path change was also measured
on its own against its parent commit, in its own sitting (middle medians,
`bench_loop.ns` / `bench_send.ns` / `bench_block.ns`, in ms):

- Builtin classes by index: 16.59 / 20.70 / 38.93 → 16.04 / 19.90 / 37.44 —
  3–4% faster on all three.
- Symbols interned once: 15.95 / 20.16 / 37.46 → 16.03 / 20.07 / 37.71 —
  nothing moved.
- A closure behind an `Rc`: 16.06 / 19.96 / 37.47 → 15.71 / 19.89 / 31.26 —
  `bench_block.ns` about 17% faster, `bench_loop.ns` about 2%.
- Compiled code on the declaration: 15.86 / 19.81 / 31.26 → 16.14 / 19.56 /
  30.59 — `bench_send.ns` and `bench_block.ns` 1–2% faster; `bench_loop.ns`'s
  first three rounds did not separate the groups, and three extra rounds put
  the after-binary lower each time, not slower.
- Handles by value: 15.58 / 18.75 / 29.67 → 15.41 / 19.13 / 29.93 —
  `bench_loop.ns` about 1% faster, the groups just failing to overlap;
  `bench_send.ns` and `bench_block.ns` did not move (three extra
  `bench_send.ns` rounds narrowed its gap to 0.17 ms, inside the
  before-group's own spread).

The per-change figures come from different sittings and are not compared with
each other; [execution.md](design/execution.md), "What step B moved", has
them with what each change removed.

**Byte identity.** The two binaries of that sitting build and snapshot every
fixture to identical bytes: `compared 44 artifacts, 0 differing`. The closure
change and the code-on-declaration change, which alter what the encoder and
decoder read, were each checked the same way against their parent commit, with
the same result.

581 tests pass, with one ignored; `parse-corpus` stays at **210/0/1/0** and
`run-corpus` at **6/0/205**.

## M4 — The reference lowering (2026-09-12)

Plan: [ns-next-reference-lowering](superpowers/plans/2026-09-12-1157-ns-next-reference-lowering.md).

Step C, the last of the three inserted ahead of 1b. The compiler has a second,
deliberately naive lowering, `compiler/reference.rs`, and every program must
behave byte for byte the same under both: the differential oracle the retired
tree-walker used to be, meant to stay, so that every later optimization — and
eventually a JIT — is checked against a baseline that does not move
([compiler.md](design/compiler.md), "The reference lowering"). The design was
written and committed before the plan.

**The harness found no divergence; the final review, probing with the
oracle, found one.** Every fixture, every golden outcome, every build/load
round trip and every `run-corpus` row produces its recorded answer under both
lowerings, and every corpus module the resolver accepts compiles under both
with the reference lowering's shape check passing; no recorded answer changed.
The whole-branch review then ran programs written to probe where the two
lowerings differ, and one answered differently: `^x := y`, with a setter `x:`
that assigns `y` through a closure built earlier, answered `y`'s new value
under the optimized lowering and the value `y` had under the reference one. The
rule decided which was right — `x := e` answers the value of `e`
([dispatch.md](design/dispatch.md), "Setter sends") — so the defect was the
optimized lowering's, and it predates this step. It skipped the copy that
protects an assignment's value from its setter send wherever the value was
consumed by the next instruction — a `^`, a slot initializer, a block's last
statement, an inlined block's result — on the reasoning that nothing runs in
between; but the setter send itself runs user code before that instruction
reads. The fix has every position state whether anything reads its value, and
skips the copy only where nothing does, so the common case keeps its cheap form:
every fixture and every corpus module that builds compiles to the same bytes as
before, and the three benchmarks moved within noise. A new golden outcome in
`program_outcomes.rs` pins seven positions under both lowerings
([compiler.md](design/compiler.md), "The value-register invariant").

**Three decisions**, settled with the user on 2026-09-12 and recorded in the
design:

1. **An independent module, not a mode of `Lowerer`.** It shares with
   `lower.rs` only what must have one definition or is purely mechanical — the
   bytecode types, `lower_literal`, `selector_of`, the parameter counting, the
   span helpers, `builtin_kind_from_name` and the two superclass-clause
   expression builders in `interpreter/nested.rs` — because whatever the two
   share, the comparison cannot check. A flag inside `Lowerer` would have
   checked only the four things the lowerings do differently (register reuse,
   inlining, the lexical-depth rewrite, `^` inside an inlined block), and every
   later optimization would have needed a "not in reference mode" guard whose
   omission silently compares the optimized lowering with itself. The price is
   that every new construct is lowered twice.
2. **The recursion-depth limit exempts exactly the activations inlining
   removes.** It counts every activation except a block activation that a
   builtin control-flow native performs for a `SendInlinable` — an ordinary
   public send only the reference lowering emits, for exactly the sends the
   optimized one inlines. So the count agrees at every point of every program,
   depth stays bounded (every recursive cycle contains a counted activation),
   the optimized lowering pays nothing beyond the counter, and the oracle
   compares even an overflow byte for byte. Counting every activation would
   trip at different depths under the two lowerings; counting only method-like
   activations would leave a recursion through blocks to a memory bound that
   does not agree. The Smalltalk lineage bounds depth by memory and Nova by a
   fixed count of contexts ([execution.md](design/execution.md), "How other
   systems bound recursion depth"); none has two lowerings that must agree, so
   the rule is ns-next's own. `SendInlinable` and the counter were built
   together in 1b, the counter being its only consumer
   ([compiler.md](design/compiler.md), "How the limit counts").
3. **Identity hashes are assigned lazily**, the first time `hash` asks, as in
   Cog/Spur (`hashBitsOf:`, which calls `newHashBitsOf:` when the bits are 0)
   and Primordial Soup (`Object_identityHash`, which sets `header_hash` when it
   is 0). A hash drawn at allocation made every allocation observable, and the
   reference lowering allocates a closure for every block the optimized one
   inlines; closure conversion and a JIT's escape analysis will add and remove
   allocations the same way ([object-model.md](design/object-model.md)).

**What changed**, in the plan's order:

1. **Lazy identity hashes** — a behaviour change of its own. An object carries
   0 until `Object>>hash` asks, through `Heap::ensure_identity_hash`; the
   encoder writes what is stored without assigning one, and the decoder keeps
   every value, 0 included. `hash` values change, which the specification
   leaves open and no recorded outcome pinned. A new fixture,
   `tests-fixtures/identity_hash.ns`, pins the rule — its answers (1, 2, 1,
   `false`) were derived from the rule, not recorded from a run.
   `crates/ns-runtime/tests/fixtures/golden-image.bin` was regenerated and
   differs from before in exactly one byte, checked byte by byte: the
   snapshotted instance's identity-hash field, now 0 because nothing asks for
   it.
2. **The reference lowering.** `compiler/reference.rs`, `Lowering { Optimized,
   Reference }` and `Interpreter::bootstrap_with`: an interpreter compiles
   every module it loads with the lowering it was bootstrapped with, so one
   never mixes the two, and a decoded artifact runs the code it carries.
   Nothing is inlined, every scratch register is written by exactly one
   instruction, every read of a local is a copy, and `check_shape` asserts
   that in a debug build. Eight unit tests, and
   `crates/ns-runtime/tests/reference_lowering.rs`: programs answer alike, the
   initializer's shape, and every corpus module the resolver accepts — at
   least 189 of 211, a floor that is never lowered — compiles under both.
3. **`--lowering <optimized|reference>`** on `run`, `snapshot`, `build` and
   `disasm`, defaulting to optimized. `disasm` refuses it for an artifact,
   which carries the code it was built with; `bench` keeps the optimized
   lowering, since timing the oracle means nothing. Two new `disasm` snapshots
   pin the loop and the fallback shapes under the reference lowering.
4. **The harness.** Every fixture, golden outcome and build/load round trip
   runs under both lowerings against the same recorded answer, and
   `run-corpus` runs every row four ways — from source and as a built binary,
   under each lowering — with no switch, naming the lane that disagrees. A
   review finding followed: `corpus/runnable.toml`'s header now describes the
   four lanes.
5. **The final review's fixes.** The setter-value fix above, in a commit of
   its own. `check_shape` now also asserts that a named local is read only by
   `Move` and written only by `StoreLocal` — the property that kept the
   reference lowering immune to that defect — and the reference lowering's
   ceilings on temporaries, block parameters and named locals are checked in
   every build; four more unit tests pin the shape check itself.
   `crates/ns-cli/tests/lowering.rs` checks that `build` and
   `snapshot` compile with the lowering they are given, by reading the code
   the artifact carries, since no program's output would show a dropped
   `--lowering`.

**A known regression: identity hashes of tag-named objects across a
snapshot.** Lazy hashes lose the hash of an object an image names by tag
rather than by record — a builtin class, the module's root class, the
`platform` singleton. Nothing records it, so a hash one of them was asked for
before a snapshot is replaced by a fresh one after the load. Before this step
those hashes survived only because the bootstrap allocated the same objects in
the same order every time. Fixing it needs a design decision — a new wire tag
that records those hashes, or a deterministic hash rule for tag-named
objects. The user decided on 2026-09-12 to record them
([image-format.md](design/image-format.md), "Identity hashes are preserved
verbatim"). Fixed; see "M4 — Tag-named identity hashes" below.

**What was deleted:** `Heap::alloc_placeholder_object`, whose callers now use
`allocate_object`, since every object starts at hash 0;
`Interpreter::allocate_identity_hash` (the `Heap`'s own `allocate_identity_hash` is
now private, with `ensure_identity_hash` its only caller outside tests); and
the discarded `Object` allocation in
`build_builtin_classes`, kept by step B only because removing it shifted every
later hash.

Measured in two sittings on a release build, each with three rounds of
`ns-next bench` over the three fixtures, alternating the two binaries within
each round, the "before" built in a throwaway worktree. Bytecode medians in ms,
rounds 1–3.

Lazy identity hashes, against their parent commit, because they touch
allocation:

| Source | Before | After |
|---|---|---|
| `bench_loop.ns` | 16.34, 15.57, 15.42 | 15.58, 15.49, 15.76 |
| `bench_send.ns` | 18.79, 19.08, 19.33 | 18.98, 18.83, 19.04 |
| `bench_block.ns` | 29.94, 29.74, 30.85 | 29.83, 30.20, 30.99 |

The whole step, against `master`:

| Source | Before | After |
|---|---|---|
| `bench_loop.ns` | 15.58, 15.29, 15.32 | 15.48, 15.68, 15.48 |
| `bench_send.ns` | 18.97, 19.03, 19.00 | 18.96, 18.87, 19.03 |
| `bench_block.ns` | 29.89, 30.61, 29.92 | 30.60, 29.95, 30.49 |

Nothing moved beyond noise in either sitting: on every source the two groups
overlap, and no "after" median is slower than its "before" by more than the
spread within either group. Three extra whole-step rounds of the two sources
whose "after" sat higher settled `bench_loop.ns` — 15.82, 15.38, 15.62 before
against 15.57, 15.55, 15.66 after, no direction — and left `bench_block.ns`'s
"after" higher in five rounds of six — 29.83, 29.95, 29.81 against 29.93,
30.15, 30.01 — by about 0.5% on the six-round medians (29.92 against
30.08 ms), inside both groups' spread (0.79 and 0.67 ms). Not attributed; no
profile was taken. The optimized lowering's output is unchanged (below), so
the timed code is the same bytecode, and the one change on its path is that
an allocation no longer draws from the counter.

**Byte identity.** Lazy identity hashes build every fixture into a stateless
binary identical to their parent commit's: `compared 23 artifacts, 0
differing` (a snapshot changes on purpose, so only binaries were compared).
The reference lowering and the CLI switch each left every `build` and
`snapshot` artifact of the optimized lowering identical to their parent
commit's: `compared 46 artifacts, 0 differing`, twice. Against `master`, all
23 stateless binaries are identical, and all 23 snapshots differ in one to
three bytes at the same length — in each, `inspect --full` shows nothing but
the identity-hash fields of objects nothing asked for their hash, now 0.

Suite running time (`cargo test --workspace`, debug build) went from 6.43 s to
9.09 s with the harness: every fixture, golden outcome and build/load pair runs
twice, and `run-corpus`'s rows four ways instead of two — more process spawns
and compiles, not a slower lowering.

609 tests pass after the final review's fixes (600 before them), with one
ignored; `parse-corpus` reports **parsed 210, failed
0, expected-fail 1, stale 0** and `run-corpus` **ran 6, failed 0, skipped
205**.

## M4 — Tag-named identity hashes (2026-09-12)

Plan: [ns-next-tag-named-identity-hashes](superpowers/plans/2026-09-12-1604-ns-next-tag-named-identity-hashes.md).

The small fix ahead of 1b, closing the regression left by step C above: a
builtin class, the module's root class and the `platform` singleton are named
by tag rather than by record, so nothing about a record carried a hash one of
them was given, and a hash asked for before a snapshot came back as a fresh
one after the load ([image-format.md](design/image-format.md), "Identity
hashes are preserved verbatim").

The graph section now closes, after its roots, with a table of exactly the
tag-named objects whose stored hash is not 0, each entry a tag from a new
family — `HASHED_BUILTIN_CLASS` / `HASHED_ROOT_CLASS` / `HASHED_PLATFORM`
(0/1/2) in `crates/ns-runtime/src/image/opcodes.rs`. `load` restores each
hash onto the object the bootstrap built, refuses a 0, a duplicate entry or
`u32::MAX` like any other malformed hash, and advances the identity-hash
counter past the larger of this table's maximum and the object table's.
`inspect` reads it as part of the graph section.
`crates/ns-runtime/tests/fixtures/golden-image.bin` grew by four bytes (1219
to 1223) — the empty table's own zero count, since none of that fixture's
tag-named objects had a hash recorded when it was built.

`a_hash_given_to_a_tag_named_object_survives_a_snapshot`
(`crates/ns-cli/tests/snapshot_load.rs`) pins the fix end to end: a module
asks a builtin class, its own class and the platform for their hash while
being instantiated, snapshots, reloads, and asks the same three questions
again. Checked against the pre-fix parent commit (`1e57a47`, Task 1's own
parent) in a throwaway worktree: the task brief's literal program passed
there too, unchanged — its three hash requests happen to be the very first
the process ever makes, both while snapshotting and again after the load, so
the stale counter restarts at the same value both times and coincidentally
redraws the same three numbers, the same accident this document already
named for the pre-lazy hashes, surviving into this one minimal case even with
laziness. A fourth, unchecked slot, `selfHash = self hash`, requested last,
gives the counter an ordinary record to land on after the three tag-named
draws and turns the coincidence into a real test: with it, the test prints
`false` three times against the parent commit and `true` three times against
this fix.

617 tests pass, with one ignored; `parse-corpus` stays at **210/0/1/0** and
`run-corpus` at **6/0/205**.

## M4 — The trampoline (2026-09-12)

Plan: [ns-next-trampoline](superpowers/plans/2026-09-12-1619-ns-next-trampoline.md).

Step 1b. Sends no longer recurse on the Rust stack: one run loop, `vm::run`,
pushes, pops and resumes every activation a send makes, and the activation
stack is the only place a running computation lives, apart from three paths
that re-entered the loop from Rust until 2a — `do:`, instantiation and the
nested-class accessor. Recursion depth is an ordinary Newspeak error under two
counted bounds, the same under both lowerings
([execution.md](design/execution.md), "The trampoline"). The design was settled
and recorded in the design documents before the plan.

**Four decisions**, settled with the user on 2026-09-12:

1. **The control-flow natives are trampoline steps.** `value` and the four
   `ifTrue:` forms return the block to activate; `and:` and `or:` return it
   with a continuation that coerces its value; `whileTrue:` and `whileFalse:`
   return the condition with a continuation that activates the body or ends
   the loop. No control-flow native calls back into Newspeak, so no Rust
   re-entry happens under one lowering and not the other. `do:` stays a
   callback until it moves to Newspeak.
2. **Two bounds**, the way Lua and CPython bound depth once their calls left
   the native stack: L = 100,000 activations, counted by
   [compiler.md](design/compiler.md)'s rule, and N = 1,000 re-entries of the
   loop from Rust. The error texts, `stack overflow: recursion too deep` and
   `stack overflow: too many nested native calls`, name no number.
3. **The `SendInlinable` exemption also requires the block's
   `captured_frame` to be the sending activation's reified frame**, so a
   crafted artifact cannot use the exemption to recurse uncounted.
4. **The CLI runs every command on a thread it spawns**, with an explicit,
   measured stack — the whole command, not only the send, because the symbol
   interner is thread-local.

**What changed**, in the plan's order. The plan's first task, the profile of
`bench_block.ns`, was taken last, with the user, on a binary built from
`04e5fc0` ([execution.md](design/execution.md), "What the profiles
show").

1. **`SendInlinable`**, wire tag 20, pinned in `opcode_tags.rs`. The reference
   lowering emits it for exactly the sends `control_flow::recognize` accepts,
   and the VM decides it exactly as `SendPublic`. Two reference-lowering
   `disasm` snapshots changed, in exactly the marked sends; no
   optimized-lowering snapshot changed.
2. **Deciding before running.** Every send is split into a decision —
   `Interpreter::decide_send`, `sends::decide_lexical_send` and
   `sends::decide_super_send`, answering a `Step` — and running what was
   decided. On its own, still recursive, the split measured 6–11% slower;
   answering a `Step::Value` in place recovered part of that, and the rest was
   accepted as the cost of a shape the loop was about to consume.
3. **The loop.** `vm::run` runs an activation and everything it pushes, and
   pops all of it on every path out. `Interpreter::push_step` pushes what a
   send decided; `Interpreter::reenter` is the one way Rust enters the loop; a
   stack entry carries its own code, so a return resumes its caller from the
   `program_counter` its send stored; a send's arguments go through one reusable buffer
   instead of a vector per send. `Signal::Return` is an unwind of the
   activation stack (`vm::unwind`, `Interpreter::catches_return`). The plan's
   unwind had a defect the task found: a `^` into a running initializer made it
   read a target an inner run had already popped — a panic where the parent
   commit reported the known gap. `Heap::activation_is_live` is the one
   question a popped handle may be asked, and the gap is reported as before,
   pinned under both lowerings.
4. **Blocks as steps.** `Step::Block` carries a `Continuation` — `Deliver`,
   `Truthy`, `WhileCondition` or `WhileBody` — that rides on the block's stack
   entry and is applied, through `Interpreter::resume`, when the loop pops the
   block, so a native loop's condition and body replace each other on the
   stack. `value` and the control-flow natives answer steps;
   `invoke_closure` remains only for `do:`. A non-Boolean loop condition's
   `mustBeBoolean` is decided and pushed by the loop instead of re-entering it.
   Three golden outcomes were added first and passed before the change, pinning
   the behaviour it had to keep.
5. **The two bounds.** `MAX_ACTIVATION_DEPTH` and `MAX_NATIVE_DEPTH`;
   `Interpreter::push` is the one checked push, the heap keeps the counted
   depth, `push_step` decides the exemption — through `push_continuation` for
   a native loop's next block — and `reenter` counts N. **A
   deviation:** the mark of a control-flow native's block is
   `StepArguments::ControlFlow`, not a `bool` field on `Step::Block` as the plan had
   it — the field measured 4–8% slower on every benchmark even with nothing
   reading it. `&` and `|`, which share `and:`'s and `or:`'s native, carry the
   mark too, harmlessly: nothing emits a `SendInlinable` for them. The
   superclass chain is walked iteratively (`run_initializer_levels`, with
   `run_own_initializer` per level). The CLI's thread has 32 MiB, from
   measured debug-build re-entry costs of 9,280 bytes through `do:`, 8,256
   through instantiation and 6,816 through the nested-class accessor.
   `crates/ns-runtime/tests/trampoline.rs`, eight tests over the branch, pins
   deep recursion on the stack, the unwind, the exact boundary under both
   lowerings, the crafted artifact and the continuations at the floor of a
   run; five golden outcomes pin a thousand-deep recursion, an unbounded one,
   and recursion to N through `do:`, through instantiation and through the
   nested-class accessor. Every new answer was derived from the rule, not
   recorded from a run.
6. **Caching the running activation's register base in the loop** — measured
   over six rounds, no source faster and none slower, so it was reverted by its
   keep rule and not committed ([execution.md](design/execution.md), "What the
   trampoline moved").
7. **The documents**, and a sweep of the comments that still named functions
   the branch removed or renamed.
8. **The whole-branch review's fixes.** The decoder caps how deeply a code
   table's blocks nest at the parser's own ceiling of 128
   (`DecodeError::BlockNestingTooDeep`): the exempt activations above a
   counted one are bounded by that nesting, which nothing capped in an
   artifact, and dropping a deep chain of decoded bodies overflowed the stack
   ([image-format.md](design/image-format.md), "Verification"). The run loop
   no longer decides a native loop's exemption itself
   (`Interpreter::push_continuation`). New tests pin the iterative superclass
   walk under instantiation, the crafted artifact with a reified sender, and
   the continuations at a run's floor.

**What was deleted:** `vm::run_code` and its `collect`, `Interpreter::run_step`
and `invoke_method`, `Heap::grow_activation_temps` (every activation is pushed
with its whole register file), `builtins::block::run_while`, and
`builtins::boolean::truthy_arg` (now `truthy`). `dispatch_send`,
`dispatch_lexical_send`, `dispatch_super_send` and `does_not_understand` are
`decide_send`, `decide_lexical_send`, `decide_super_send` and
`decide_does_not_understand`.

Measured whole against `04e5fc0` in one sitting on a release build, three
rounds of `ns-next bench` over the three fixtures, alternating the binaries
within each round, the "before" built in a throwaway worktree. Bytecode medians
in ms, rounds 1–3:

| Source | Before | After |
|---|---|---|
| `bench_loop.ns` | 16.96, 16.40, 16.53 | 11.23, 11.27, 11.28 |
| `bench_send.ns` | 20.48, 20.33, 20.20 | 14.13, 13.95, 13.87 |
| `bench_block.ns` | 30.77, 30.17, 31.14 | 23.66, 24.38, 23.97 |

On every source the after-group lies entirely below the before-group:
**`bench_loop.ns` about 32% faster, `bench_send.ns` about 31%, `bench_block.ns`
about 22%.** Each task on the hot path was also measured against its parent;
those tables, and what each attributes, are in
[execution.md](design/execution.md), "What the trampoline moved".

**Byte identity.** Every `build` and `snapshot` artifact of the optimized
lowering is identical between `04e5fc0` and this step: `compared 46 artifacts,
0 differing`. A reference-lowering artifact differs by construction, since it
carries `SendInlinable`.

Suite running time (`cargo test --workspace`, debug build) went from 9.36 s to
12.82 s, 3.1 s of it `crates/ns-runtime/tests/trampoline.rs`, whose boundary
test runs three recursions to 100,000 counted activations under each lowering.

644 tests pass, with one ignored (617 before); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**.

## M4 — No re-entry from Rust (2026-09-13)

Plan: [ns-next-no-reentry](superpowers/plans/2026-09-13-0149-ns-next-no-reentry.md).

Step 2a, the first part of the root discipline. Rust enters the run loop only
as the host, its outermost caller: `do:`, instantiation and the nested-class
accessor, the three paths that ran Newspeak code in nested runs of the loop,
are trampoline steps, so no Rust frame is live below a safepoint — the ground
2b and 2c stand on ([execution.md](design/execution.md), "No re-entry from
Rust"; [memory.md](design/memory.md), "No Rust frame is live below a
safepoint"). The design was settled and recorded in the design documents
(`be3eb40`) before the plan (`6f940ba`); the step's commits follow the plan on
`feature/root-discipline-2a`, from `aa8323a`.

**Four decisions**, settled with the user on 2026-09-13:

1. **The root discipline removes the re-entries rather than rooting them.**
   Step 2 is split into 2a, 2b (safepoint, tracing, host roots, stress mode)
   and 2c (the lifetime brand), each planned once the one before it has
   merged.
2. **`do:` becomes a continuation step now**, like `whileTrue:`; it moves to
   Newspeak only after the inline caches.
3. **N goes**, and the three goldens that ended at it end at L.
4. **`INTERPRETER_STACK_BYTES` is re-derived** from what still recurses on the
   Rust stack. The thread stays, because the symbol interner is thread-local.

**What changed**, in the plan's order:

1. **`do:` as a step.** `do:` answers its block for the first element with
   `Continuation::DoElement`, whose `DoLoop` carries the snapshot and the next
   index, and `builtins::collections::after_element` activates the block for
   each next element. **A deviation:** the plan routed `do:` through a guarded
   arm in `dispatch_native`; `collections::dispatch_array` and
   `dispatch_ordered_collection` check for `do:` themselves and answer its step,
   so `dispatch_native` keeps one arm per class. It measured no worse, and it
   keeps that match uniform. An `#[inline(never)]` on `do_step`, tried while
   chasing the shift below, helped nowhere and was dropped.
2. **The nested-class accessor as a step.** The accessor answers its clause as
   a `Step::SuperclassClause` with `Continuation::NestedClass`, and
   `nested::after_clause` builds, caches and answers the class when the clause
   returns. The new `Interpreter::abandon`, to which `vm::unwind` hands every
   continuation it pops, is `resume`'s counterpart.
3. **Instantiation as a state machine, and one host entry.**
   `decide_instantiation`, `climb_levels`, `descend_levels` and
   `advance_instantiation` behind `Continuation::Instantiate`;
   `try_factory_send` is an ordinary decision, and `Interpreter::send` and
   `instantiate` both enter the loop through `Interpreter::run_from_host`,
   which debug-checks that the loop is never entered while it runs.
   `reenter`, `invoke_closure`, `builtins::call_block`, the native-depth count,
   `MAX_NATIVE_DEPTH` and its error went. **A deviation:** the walk's last
   initializer delivers its own value, since an instance initializer always
   answers `self` under both lowerings
   (`new_answers_the_instance_its_initializers_ran_on` pins that), so it needs
   no continuation.
4. **The interpreter thread's stack**, 32 MiB → 8 MiB: four times the 2 MiB
   libtest threads on which the suite runs the deepest nesting the parser
   accepts through every stage in a debug build.
5. **The documents**, and a sweep of the comments that still named functions
   the step removed or described re-entry.
6. **The final review's fixes**, which the whole-step measurement below
   includes:
   - **Every instantiation body inline.** The plan boxed a description of
     every body that is neither a method nor a block (`Step::Body`), and the
     walk's state per body with work after it; a chain of two levels with code
     came out 15–18% slower than `be3eb40`. Each body is now a variant of its
     own — `Step::Initializer`, `Step::InitializerThen`, `Step::ClauseMessage`,
     `Step::SuperclassClause` — each smaller than `Step::Method`, because an
     `Initializer` as large as `Method` measured 4–6% slower on every source
     that instantiates nothing, with `Result<Step, Signal>` still 56 bytes.
   - **The walk's box reused.** The walk runs in a box taken from the
     interpreter's spares and handed back emptied, so an instantiation
     allocates only its instance's fields once one has run; worth 3–13% on the
     instantiation sources.
   - **The in-progress marker.** The cyclic-definition check scanned a list of
     every creation underway, which a recursion through the accessor to the
     activation limit made quadratic — it doubled the suite's time. The class's
     own cache slot now holds a marker while its clause runs, so the check is
     one read and the list, with its object handles, is gone.
   - **`Continuation`'s drop glue** is no longer called on every return
     (`vm::finish` forgets the `Deliver` it takes), about 1%.
   - **Four tests** for unwinding through clause and initializer activations,
     one that a redeclared nested class built on its inherited one is no
     cycle, and one that a finished walk leaves its box empty; `do_step` made
     private, an empty `do:` answered without a loop state, and the review's
     comment and document corrections.

**Three golden outcomes changed**, each in the task that removed its re-entry:
`do-recursion`, `instantiation-recursion` and `accessor-recursion`
(`crates/ns-cli/tests/program_outcomes.rs`) ended in `stack overflow: too many
nested native calls` and end in `stack overflow: recursion too deep`. Each of
those recursions is made only of counted activations now, so it ends at L like
any other; the limit the old text reported no longer exists. That is why each
change is right — none was recorded because the code produced it. Every other
fixture, golden and `run-corpus` row keeps its answer under both lowerings.

**Measurement findings on the way.** Every change on the hot path was measured
against its parent over the three fixtures and throwaway sources —
`bench_do.ns` and `bench_new.ns` from the start, `bench_new2.ns` and
`bench_newarg.ns` (two levels with code, and a clause message with an
argument) from the final review on
([execution.md](design/execution.md), "What removing the re-entries moved"):

- **`do:` as a step moved every source by a uniform 2–4%**, `bench_loop.ns`
  included, which runs no `do:`. A control — the parent plus only the new
  `Continuation` variant — did not reproduce it; `vm::finish` is out of line in
  every build, and in the changed one `object_class::dispatch` was inlined
  into `dispatch_native` and `run`'s register allocation differed. Accepted as
  an LLVM inlining and layout artifact on unchanged executed code, not
  bisected further, with this whole-step measurement to re-check the net.
- **Instantiation first measured about 15.6% slower on `bench_new.ns`.**
  Counting allocations found two more boxes per instantiation, the walk's state
  and the body step. Delivering the last initializer's value, and carrying that
  initializer inline, restored the count on that shape (2 allocations, 144
  bytes, per `Cell new`) and left about +1.8%. The final review found the same
  two boxes still paid per body on a chain of two levels with code, which no
  source measured until then; carrying every body inline (−5.2% and −8.0% on
  `bench_new2.ns` and `bench_newarg.ns`) and reusing the walk's box (−3.0%,
  −8.8%, −12.8% on the three instantiation sources) took them out.
- **Two changes were measured and not kept.** An `Initializer` step with an
  optional continuation, as large as `Step::Method`, cost 4–6% on every source
  that instantiates nothing; and comparing `do:` as a well-known symbol rather
  than through the interner cost 1.7–2.1% on three sources that send to no
  collection in their loops.

Measured whole against `be3eb40` in two sittings on a release build, five
rounds of `ns-next bench <source> --iterations 30`, alternating the binaries
within each round, the "before" built in a throwaway worktree. Means of the
five medians in ms, and the change in each sitting:

| Source | Before | After | Change, sitting 1 / 2 |
|---|---|---|---|
| `bench_loop.ns` | 11.00 | 11.20 | **+1.9% / +1.4%**, after entirely above in both |
| `bench_send.ns` | 13.90 | 13.93 | +0.2% / +0.8%, overlapping |
| `bench_block.ns` | 23.04 | 23.41 | **+1.6% / +1.4%**, after entirely above in the first |
| `bench_do.ns` | 7.89 | 7.79 | −1.3% / −1.3%, overlapping |
| `bench_new.ns` | 7.28 | 7.19 | −1.2% / −2.7%, after entirely below in the second |
| `bench_new2.ns` | 8.20 | 8.13 | −0.9% / −1.2%, after entirely below in both |
| `bench_newarg.ns` | 10.51 | 10.11 | −3.8% / −3.2%, after entirely below in both |

**Instantiation is faster than it was with the re-entries, and two sources
are slightly slower.** The instantiation sources gained what
the boxes and the drop glue had cost, and running an initializer in the same
loop, once the walk allocates nothing, is cheaper than a nested run. `do:` as
a step came out −1.3%, overlapping — not clearly faster, although the design
expected it to be. `bench_loop.ns` and `bench_block.ns`, which run none of the
changed paths in their loops, are 1.4–1.9% slower: the uniform codegen shift
the step's first change showed ([OPEN-WORK.md](OPEN-WORK.md)).

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.5 s in two runs alike, as before the step. Before the marker
it took 25 s: the two tests that recurse through the nested-class accessor to
the activation limit made the old list-scanning cyclic-definition check
quadratic, 7.8 s each.

662 tests pass, with one ignored (644 before); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**.

## M4 — The safepoint, tracing, host roots and the stress mode (2026-09-13)

Plan: [ns-next-checked-roots-2b](superpowers/plans/2026-09-13-1748-ns-next-checked-roots-2b.md).

Step 2b, the second part of the root discipline. Every handle the runtime
keeps is somewhere a tracer can find and rewrite it, and a debug-build stress
mode proves it by moving every object and every reified frame at the run
loop's safepoints under the whole suite, where a handle the tracer missed
panics at its first read ([memory.md](design/memory.md), "Checked roots and a
collector"). The step's commits follow the plan (`0cfe9dd`) on
`feature/root-discipline-2b`, from `4882a2c`.

**Decisions**, settled with the user on 2026-09-13 or following from the design:

1. **The stress mode moves on a budget, not at literally every safepoint.**
   A move costs work proportional to the heap and the stack, so every
   safepoint is quadratic in the recursions to 100,000 activations — hours
   per suite run. Every safepoint moves while the traced size is at most
   `STRESS_MOVE_BUDGET` (1,000), and above it once per size-over-budget
   safepoints: the pacing a collector's allocation-driven trigger has, where
   the prior art's fixed intervals would take coverage from small tests.
2. **`ClosureData` is inline in the closure object**, and everything else that
   needs a closure names the object; the native loops' state is a
   `Box<WhileLoop>` owned by one stack entry. A closure frame record names its
   closure object: `FRAME_CLOSURE` (1) is retired for `FRAME_CLOSURE_OBJECT`
   (6), and a record naming a non-closure is `NotAClosureReference`.
3. **The host API takes and answers `Root`s**; `Signal` and `EvaluationResult`
   become crate-private; `heap_reference()` and `heap_mut()` stay until 2c;
   `image::snapshot` and `image::build` took plain values, fetched just
   before the call.
4. **The root table lives in `Heap` behind a `RefCell`**, so `stash` and
   `fetch` take `&self`, and it is swept at every move and whenever it has
   doubled, so it stays bounded in a release build too.
5. **A release build keeps the root table, `Root`, `HostResult` and the
   flag-clearing slow path**; the checking machinery — `trace.rs`, the move
   epoch, the move, the stress schedule and its test hooks — is
   `#[cfg(debug_assertions)]`.

**What changed**, in the plan's order:

1. **The closure split** (`f842f6f`). `FrameKind::Closure`,
   `WhileLoop.condition`, `DoLoop.block` and `Step::Block` name the closure
   object; the image format's closure frame names it by object id.
2. **The root table and the rooted host API** (`cfa87b0`, `03100f7`,
   `a49139a`). `Root`, `Interpreter::stash`/`fetch`, and `send`,
   `instantiate`, `load_module` and `platform` on roots, answering a
   `HostResult`; a Newspeak error reaches the host as a `HostError` with its
   receiver rooted, and `instantiate` refuses a root that holds no class. The
   CLI converted first, the tests under transitional names.
3. **The epoch, the tracing and the move** (`277e606`). `trace.rs`, with
   every impl naming every field and matching every variant; a move epoch on
   every `ObjectReference` and `FrameReference` in a debug build; `Heap::move_everything`,
   rotating both arenas with no fixed point and forwarding through a
   `Forwarder` that checks origin and epoch; `Interpreter::move_now` and
   `moves` for tests. The argument buffer is cleared when it is given back, so
   it is empty at every safepoint. A debug `Object` grew to 96 bytes and a
   frame-arena entry to 160, which puts a crafted image's debug amplification
   at about 24.9x; the release sizes, 56 and 136, did not change.
4. **The safepoint and the stress mode** (`747136a`, `36fadb6`). The poll in
   `vm::run` at every switch and backward jump, `Interpreter::safepoint` in
   `interpreter/safepoint.rs`, and `NS_NEXT_STRESS_MOVES`.
5. **The runtime's tests on roots** (`f2d20fd`, `192cd66`), every assertion
   unchanged, each file passing with and without the variable.
6. **`image::load` on roots** (`a5da85f`): `LoadedImage` carries the root class
   and the image's roots as `Root`s.
7. **Only roots leave the crate** (`b3d40d6`): the transitional entries went,
   `Signal`, `EvaluationResult`, `platform_value` and `vm::run` became
   crate-private, and `CLAUDE.md` gained the two stress runs as gates and the
   rule that a host keeps a value across a run only as a `Root`.
8. **The documents**, and the whole-step measurement below.

No golden, no `.expected` file and no `run-corpus` row changed.

**What reviews found**, each fixed before its task closed:

- **The release poll was compiled out.** With an empty release slow path, fat
  LTO proved `Interpreter::safepoint` free of side effects and deleted the
  call, the branch and the flag's load, so the first measurement of the poll
  measured a binary without one. The slow path now serves the request by
  clearing the flag, the release assembly shows the compare at both poll
  sites and the cold call, and the re-measurement found no source slower than
  its parent ([memory.md](design/memory.md), "Where a collection can happen").
- **The sweep's test did not test that the sweep keeps live roots**: `fetch`
  reads through the root's own `Rc`, so a sweep that dropped every slot would
  have passed. It now holds 100 roots across 10,000 dropped ones and checks
  the table's count; a sweep changed to drop everything fails it.
- **An exact move count across runs** in `a_world_reached_from_roots_survives_moves`
  cannot hold under the stress mode, whose runs move too; the count is taken
  across each `move_now` alone.

**Measurement findings on the way.** Each change on the hot path was measured
against its parent and against `be3eb40` with the protocol and the seven
sources of "What removing the re-entries moved"
([execution.md](design/execution.md), "What 2b moved"):

- **The closure split made `bench_block.ns` 4.4% faster** and `bench_newarg.ns`
  0.8%, both entirely below, the rest level: `MakeClosure` allocates no `Rc`
  any more, and a popped block activation drops none.
- **The poll, once it was in the binary, made no source slower**: −3.0% to
  +0.9%, every group overlapping its parent's. Cog's fallback, folding the
  request into the depth check, was not needed.
- **`bench_send.ns` read +1.0% to +1.7% against `be3eb40`** in those sittings,
  where 2a had measured it level; each task's own parent sat about as high,
  +0.9% to +2.3%, so the whole-step measurement was asked to look again.

Measured whole against `4882a2c` and `be3eb40` in two sittings on a release
build, five rounds of `ns-next bench <source> --iterations 30`, alternating
the three binaries within each round, the two before-binaries built in
throwaway worktrees. Means of the five medians in ms from the first sitting,
and the change in each sitting:

| Source | `be3eb40` | `4882a2c` | After | Against `4882a2c`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.21 | 11.38 | 11.33 | −0.4% / −0.9% | +1.1% / +1.1% |
| `bench_send.ns` | 14.03 | 14.12 | 14.16 | +0.3% / −0.5% | +0.9% / +1.4% |
| `bench_block.ns` | 23.34 | 23.75 | 22.76 | **−4.2% / −3.9%**, after entirely below in both | **−2.5% / −2.6%**, after entirely below in both |
| `bench_do.ns` | 8.02 | 8.02 | 7.91 | −1.4% / −1.1% | −1.4% / −1.3% |
| `bench_new.ns` | 7.44 | 7.34 | 7.41 | +1.0% / +0.7% | −0.4% / −1.4% |
| `bench_new2.ns` | 8.42 | 8.32 | 8.35 | +0.4% / +0.8%, after entirely above in the second, by 0.01 ms | −0.8% / −0.6% |
| `bench_newarg.ns` | 10.57 | 10.24 | 10.22 | −0.3% / −0.4% | −3.4% / −3.2% |

**No source is slower than before the step beyond the sittings' noise, and
`bench_block.ns` is about 4% faster.** `bench_new2.ns`'s after-group lay
entirely above its parent's in the second sitting, 8.37–8.44 against
8.30–8.36 ms; a third sitting, run to settle it, put it level (−0.2%,
overlapping). Against `be3eb40` nothing is slower beyond the groups' spread:
`bench_loop.ns` is +1.1%, where 2a left it +1.4–1.9% with the groups apart,
and `bench_block.ns` is 2.5% faster. `bench_send.ns`, +0.9% / +1.4%
overlapping, is level with `4882a2c`, which itself sat +0.6% / +1.9% above
`be3eb40` in the same sittings — 2a's figure read in other sittings.

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.7 s, against 12.5 s before the step. Under
`NS_NEXT_STRESS_MOVES=1` it takes 190.2 s: `crates/ns-runtime/tests/trampoline.rs`
91.5 s against 3.0 s, nearly all of it
`the_depth_limit_is_exact_and_counts_alike_under_both_lowerings`, and
`ns-cli`'s tests, which spawn `ns-next` per fixture or golden, about 96 s
against 7.6 s.

685 tests pass, with one ignored (662 before); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under the stress mode.

Two stale handles from the same old epoch compared in `same()` without a
panic, since `same` has no heap to check them against; 2c's brand closed that
([memory.md](design/memory.md), "The brand").

## M4 — The lifetime brand (2026-09-14)

Plan: [ns-next-lifetime-brand-2c](superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md).

Step 2c, the third and last part of the root discipline. Every movable
handle — `ObjectReference`, `ClassReference`, `FrameReference`, and so
`Value` and every type that holds one — carries an invariant lifetime `'gc`
obtainable only inside `Interpreter::enter`, so holding a handle across a
safepoint, keeping one outside an entry, and a native re-entering the run
loop are compile errors rather than mistakes the stress mode finds at run
time ([memory.md](design/memory.md), "The brand, checked by the compiler
rather than by convention"). The step's commits follow the plan (`7903050`)
on `feature/lifetime-brand-2c`, from `a5d1e05`.

**Decisions**, settled with the user on 2026-09-13 or following from the
design, and one revised on the way:

1. **Nothing is converted at a push, a pop or an allocation.** A safe
   conversion per payload would reallocate every boxed continuation at every
   push and pop and lose the spare instantiation boxes' reuse; the whole step
   state rests as `Mutator<'static>` instead and is handed over as it is.
   The plan made that one audited cast; Task 4's review found it
   unnecessary — `body(&mut self.mutator)` gives the same guarantee in safe
   Rust, since the closure is checked for every `'gc` — so the crate has
   `#![forbid(unsafe_code)]` and no `unsafe` at all. Both alternatives are
   recorded as considered and rejected in memory.md.
2. **The brand is phantom**: `'gc` appears in a field only through the
   `PhantomData` in a handle, which is why no conversion of the state is
   needed.
3. **No public `unbind`**: `at_rest` and `branded`, private to `heap.rs` and
   one per direction, convert a value for the host root table, and nothing
   else changes a brand.
4. **`Mutator` never reaches the `Interpreter`**: `vm::run`, the safepoint
   and the host entries take `&mut Interpreter`, and a decision, a native or
   a continuation only a `&mut Mutator`.
5. **`World` lives in `heap/world.rs`**, a child of the heap module, so
   `Heap::new` and `Heap::move_everything` are private to it.
6. **Each instruction is one entry**, and `vm::run` answers roots.
7. **The host API**: `Interpreter::enter` is public; `send`, `instantiate`,
   `load_module` and `platform` take and answer roots; `Interpreter::stash`
   takes a `Value<'static>`, an immediate; heap access, `fetch` and every
   step method are on `Mutator`.
8. **The codec runs inside an entry**: `image::snapshot` takes a
   `&Mutator<'gc>`, `image::build` a `&Heap<'gc>`, and `image::load` decodes
   in one entry and roots what it answers.
9. **`ActivationReference` stays unbranded**: nothing moves a stack entry.
10. **Two stages**: Tasks 1–3 moved the code into its final shape with no
    lifetime, and Task 4 added `'gc` and nothing else.
11. **Three `compile_fail` doctests on `Interpreter::enter`**, each beside a
    passing twin that differs from it in the one line that matters.
12. **The finding "two stale handles from the same old epoch compare in
    `same()` without a panic" closes**: within one entry nothing moves, and
    no handle survives from one entry to the next.

**What changed**, in the plan's order:

1. **The step state becomes a `Mutator` the run loop enters per
   instruction** (`d27e40b`): `World`, `Interpreter::enter`, and every step
   method moved from `Interpreter` onto `Mutator`; `vm::run` makes one entry
   per instruction and roots its answer in the last.
2. **Hosts and tests read through `enter`** (`7425f22`, `1c61f1e`), the CLI
   first, and nothing that holds a handle leaves an entry.
3. **Only `enter` reaches the step state** (`e59dc52`, `9066e58`): the
   transitional `Deref` from `Interpreter` to `Mutator` and the old heap
   accessors and `Interpreter::fetch` went, and the image tests' graph
   fixture keeps roots and inspects a decoded graph inside its entry.
4. **The brand** (`1549fb0`, `2caf3e0`): `'gc` on every movable handle and
   every type that holds one, `Heap::new` and `Heap::move_everything`
   private, the three doctests, and then the cast replaced by the plain call
   and the crate's `#![forbid(unsafe_code)]`. Two runtime tests that kept a
   value across a run on purpose went with it, since that mistake no longer
   compiles and the doctests pin it; the stress mode still checks what the
   tracer knows.
5. **The documents**, and the whole-step measurement below.

**Names are written out.** Between Tasks 2 and 3, the user decided on
2026-09-14 that no identifier in the workspace is one or two characters long
or a truncated word — `interpreter`, not `interp`; `program_counter`, not
`pc`; `ObjectReference`, not `ObjRef` — `ns-syntax` included (`a5c3a0f`,
`41b8107`, `6b186cf`, `7b41649`). Clippy's `min_ident_chars`, configured in
`clippy.toml`, refuses the short names, so the warning-free clippy gate
enforces it; the rule and its exceptions are in `CLAUDE.md`, "Conventions".
The one snapshot that changed, `ns-syntax`'s parse snapshot, is a dump of
the AST's type and field names and differs from before in names only.

No golden, no `.expected` file and no `run-corpus` row changed.

**What reviews found**, each fixed before its task closed: values and a class
handle answered out of one entry and used in the next, at about thirty test
sites, and the image tests' graph fixture keeping a class and a decoded
graph across entries — each the shape the brand would reject, found by
review before Task 4 made it a compile error; closure parameters the rename
had named `mutator` though they bound something else; and the unnecessary
cast.

Measured whole against `a5d1e05` and `be3eb40` in two sittings on a release
build, five rounds of `ns-next bench <source> --iterations 30`, alternating
the binaries within each round, the before-binaries built in throwaway
worktrees, every binary built with `+crt-static` and every function start
pinned to a 64-byte boundary (below). Means of the five medians in ms from
the first sitting, and the change in each sitting:

| Source | `be3eb40` | `a5d1e05` | After | Against `a5d1e05`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.07 | 11.20 | 11.26 | +0.5% / −0.3% | +1.7% / +1.7%, after entirely above in the second |
| `bench_send.ns` | 13.77 | 14.12 | 14.20 | +0.6% / −0.4% | +3.1% / +2.2%, after entirely above in both |
| `bench_block.ns` | 23.16 | 22.68 | 22.66 | −0.1% / −1.4% | −2.2% / −3.2%, after entirely below in both |
| `bench_do.ns` | 8.04 | 7.74 | 7.76 | +0.2% / +0.4% | −3.5% / −2.5%, after entirely below in both |
| `bench_new.ns` | 7.38 | 7.40 | 7.32 | −1.1% / +0.0% | −0.8% / −0.5% |
| `bench_new2.ns` | 8.39 | 8.36 | 8.32 | −0.5% / +0.2% | −0.8% / −0.4% |
| `bench_newarg.ns` | 10.51 | 10.26 | 10.20 | −0.5% / −0.8% | −2.9% / −2.1%, after entirely below in the second |

**Every source is level with `a5d1e05`**, every group overlapping its
parent's in both sittings, and against `be3eb40` the after-binary sits where
its parent sits: `a5d1e05` itself is +2.5% to +2.7% above `be3eb40` on the
send and +1.1% to +2.0% on the loop in these sittings.

**The pin.** A first measurement of the step, with the binaries as their
configurations built them, read 1–2.5% slower on the block and
instantiation benchmarks, `bench_new2.ns` entirely above its parent's group
in both sittings, and a bisection placed it on the brand commit, `1549fb0`.
Diffing the post-LTO assembly function by function showed its hot code
instruction for instruction identical to its parent's — the whole run loop,
`dispatch_native`, `finish`, the pushes and pops — and only cold host code
smaller, ahead of the hot functions, which moved every hot function by 224
or 240 bytes into a different 64-byte phase; rebuilt with function starts
pinned to 64 bytes (`-C llvm-args=-align-all-functions=6`), the two commits
measured level. The tables above are the step re-measured with every
binary pinned, with a fourth binary — the unpinned `a5d1e05` — in the same
rounds: it reads −0.1% to −1.8% against the pinned one, overlapping on all
but one group, so the pin buys repeatability, not speed, at about 110 KB of
padding (2,091,520 against 1,979,904 bytes, 5.6%). On 2026-09-14 the user
decided that the pin belongs to the measurement protocol only: the shipped build
stays unpinned, `.cargo/config.toml` carries `+crt-static` and nothing
else, and every binary a measurement compares — the current tree included
— gets the whole flag set through `RUSTFLAGS`.
[execution.md](design/execution.md), "What 2c moved", has every sitting's
figures, the bisection and the evidence; "What removing the re-entries
moved" there has the protocol and the flags.

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 13.1 s, where 2b left 12.7 s. Under
`NS_NEXT_STRESS_MOVES=1` it takes 190.3 s: `crates/ns-runtime/tests/trampoline.rs`
90.8 s against 3.1 s, and `ns-cli`'s tests, which spawn `ns-next` per
fixture or golden, about 97 s against 7.5 s.

690 tests pass, with one ignored (685 when 2b closed); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under the stress mode.

## M4 — Spec-conformant lookup (2026-09-15)

Plan: [ns-next-spec-conformant-lookup](superpowers/plans/2026-09-15-0108-ns-next-spec-conformant-lookup.md).

A step inserted ahead of the inline caches, because a cache records what a
lookup answers and the lookups did not answer what `newspeak-spec.pdf`
specifies. Twelve deviations were found by probe programs against `59061ec`,
each pinned by a fixture or a program outcome whose expectation comes from the
specification text, which it cites, written before the fix. The step's
commits follow the plan (`1cfee47`) on `feature/spec-conformant-lookup`, from
`59061ec`.

**Decisions**, settled with the user on 2026-09-15:

1. **Spec conformance is not optional.** Every rule follows the specification
   PDF; the older paper it cites for the rationale is background only. Where
   the specification is silent, the canonical compiler decides
   (`prior-art/newspeak/`), and the test says so.
2. **Slot accessors are methods** (§6.3.2): entries of the mixin's method
   table with the slot's accessibility, overridable like any method, answered
   without an activation. SOMns and the canonical compilers hold them the same
   way ([dispatch.md](design/dispatch.md), "Slot accessors").
3. **Only declared members are lexically visible** (§5.7, §2.3.1): an
   enclosing class's inherited member is reached only through an explicit
   `outer N m`.
4. **A declaration may not declare one member twice** — a method, a slot's
   getter, a mutable slot's setter, a nested class. The specification states
   no rule; the canonical compiler's `checkForDuplicateNames:` refuses them,
   and a method table cannot hold both.
5. **An identity hasher** for the tables keyed by identities the runtime
   assigned, written here rather than taken as a dependency.
6. **Lazy slots stay out of scope** (§6.3.3); they came with the platform
   module.
7. **The megamorphic fallback and an application id in the object header
   belong to step 3**, not to this step.

**What changed**, in the plan's order:

1. **The design documents** (`f3a3847`): [dispatch.md](design/dispatch.md)'s
   resolution rule, the three lookups with a verdict per access modifier,
   the lexical private rule, super sends and slot accessors;
   [object-model.md](design/object-model.md) and
   [image-format.md](design/image-format.md) for the method table and the
   slot record.
2. **Public lookup passes a private declaration over, and super sends follow
   §5.10** (`c920a7e`): `dispatch::walk` decides per level whether to accept,
   skip or stop; a super send finds only public and protected methods, and
   when it finds none, the `doesNotUnderstand:` defined for the superclass
   answers, where the send used to recurse without bound.
3. **A lexical send's private rule asks only N** (`bbe210d`), the lexically
   enclosing class at the resolver's depth, as Primordial Soup's
   `LexicalSend` does.
4. **The resolver sees only declared members under their full selectors**
   (`e730d13`): `foo:bar:` no longer captures `foo:`, an immutable slot
   declares no setter, and a superclass clause's bare receiver is a lexical
   send in its enclosing class (§6.1).
5. **Parenthesized self sends and private nested classes pinned**
   (`3cf7e3e`), both already right.
6. **Slot accessors are method-table entries** (`431da35`): the lexical-send
   slot pre-check and the per-application slot map are gone; an accessor
   names its slot by its position among its mixin's own slots, resolved
   through `own_slot_base` as `InitializeSlot` is. The resolver refuses a
   duplicate member, the image's slot record carries mutability and access
   so the decoder derives the same accessors and refuses a row whose members
   collide, and the golden image is regenerated for the two new bytes per
   slot and one renamed slot of its source.
7. **The identity hasher** (`7b1901c`): `hash::IdentityMap` for the method
   tables, the nested-class slot maps and the application caches.
8. **The documents**, and the whole-step measurement below.

Eleven fixtures and six program outcomes pin the twelve deviations and the
cases around them, among them §2.3's two nested-class examples verbatim. Six
existing test sources declared a slot and a getter method of one name, which
is now a resolve error; their slots are renamed and nothing they assert
changed. No other golden, `.expected` file or `run-corpus` row changed.

Measured whole against `59061ec` and `be3eb40` in two sittings on a release
build, by the protocol with every function start pinned, three of the four
throwaway sources with their slots renamed for the same reason. Means of the
five medians in ms from the first sitting, and the change in each sitting:

| Source | `be3eb40` | `59061ec` | After | Against `59061ec`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.21 | 11.41 | 11.24 | −1.5% / −0.5% | +0.3% / +0.8% |
| `bench_send.ns` | 13.92 | 14.24 | 12.47 | −12.5% / −12.4%, after entirely below in both | −10.4% / −10.1%, after entirely below in both |
| `bench_block.ns` | 23.50 | 22.86 | 21.20 | −7.3% / −7.8%, after entirely below in both | −9.8% / −9.7%, after entirely below in both |
| `bench_do.ns` | 8.07 | 7.84 | 7.54 | −3.8% / −5.1%, after entirely below in the second | −6.5% / −6.8%, after entirely below in both |
| `bench_new.ns` | 7.46 | 7.41 | 6.81 | −8.1% / −8.2%, after entirely below in both | −8.7% / −9.9%, after entirely below in both |
| `bench_new2.ns` | 8.37 | 8.39 | 7.85 | −6.4% / −6.3%, after entirely below in both | −6.2% / −6.4%, after entirely below in both |
| `bench_newarg.ns` | 10.63 | 10.36 | 9.91 | −4.3% / −4.7%, after entirely below in both | −6.8% / −7.9%, after entirely below in both |

**No source is slower, and six are faster than both commits.** A sitting of
its own separated the two code changes that decide it: the accessors alone
made every source 3.1–18.1% slower, since every lexical send then probed a
SipHash table twice where a slot send had probed once, and the hasher made
every source 11.8–19.7% faster than that. Sparing the second probe measured
level once the hasher was in; [execution.md](design/execution.md), "What
spec-conformant lookup moved", has every sitting's figures and the two
variants of that reuse.

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.7 s, and 189.9 s under `NS_NEXT_STRESS_MOVES=1`.

710 tests pass, with one ignored (690 when 2c closed); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under the stress mode.

## M4 — Primitives as method-table entries (2026-09-16)

Plan: [ns-next-primitive-method-entries-3a](superpowers/plans/2026-09-15-2345-ns-next-primitive-method-entries-3a.md).

3a of the inline caches ([dispatch.md](design/dispatch.md), "Inline
caches"), ahead of 3b and 3c. Every builtin class's natives are now entries of
its mixin's method table, found by the ordinary lookups instead of matched by
`selector.as_str()` after the lookup found a native leaf — the model Cog and
Primordial Soup hold their primitives in, and SOMns its dispatchables.

**What changed.** `enum Primitive` (`builtins/primitive.rs`) names every
native operation, one variant each, grouped by the builtin family whose
module executes it; `builtins::execute` matches on it once, in place of
every `match selector.as_str()` that `builtins/` and `platform.rs` used to
carry.
The bootstrap fills each builtin mixin's method table from a static list per
`BuiltinClass` of (selector, access, primitive), as `MethodEntry::Primitive`.
A lookup that reaches one answers `Lookup::Primitive(primitive, level)`; a
primitive that fails on a receiver of the wrong shape passes its level over,
and the lookup continues from the superclass through `dispatch::lookup_above`
— the same outcome the `None` a native answered before this step gave, now
carried by the lookup instead of by a hand-written fallback. `dispatch::walk`'s
early return at a builtin level, `Mutator::dispatch_native_inheriting_object`
and the fallbacks that used to turn a native's `None` into a public send are
gone. See [dispatch.md](design/dispatch.md), "Primitives".

**The one behaviour change.** Every primitive is `public`, except `Object`'s
`doesNotUnderstand:`, which §5.5 requires to be `protected` — a native used to
have no access at all and answered any explicit-receiver send, so an explicit
`x doesNotUnderstand: m` sent from outside the receiver now itself ends in
`doesNotUnderstand:`. No recorded outcome needed updating for it.

**Measurement.** Measured whole against the commit the step started from,
`883431b`, and against `be3eb40`, in two sittings, by the protocol of
"What removing the re-entries moved" ([execution.md](design/execution.md)):
the gate passes in both sittings, and the step is level with its start on all
seven sources (within ±2.2%, every group overlapping except `bench_newarg.ns`,
which is entirely below start in the second sitting — faster, not merely
level). Against `be3eb40`, six of seven sources are 7–10% faster and
`bench_loop.ns` is level (+1.15%/+1.56%, overlapping). See
[execution.md](design/execution.md), "What primitives as method-table entries
moved", which also records what the step's first shape cost — a 64-byte
settled enum on every send's return path measured 5.5–22.4% slower than start
in the first, blocked measurement — and the five `perf:` commits that
recovered it, the largest by having each of the five callers that can reach a
`Lookup::Primitive` match it directly instead (`8a05621`).

Gates: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets`
warning-free; `cargo test --workspace`, 722 tests passing with one ignored;
`NS_NEXT_STRESS_MOVES=1 cargo test --workspace`, the same tests passing;
`parse-corpus` at **210/0/1/0** and `run-corpus` at **6/0/205**, plain and
under the stress mode. No recorded outcome moved.

## M4 — Inline caches (2026-09-16)

Plan: [ns-next-inline-caches-3b](superpowers/plans/2026-09-16-0950-ns-next-inline-caches-3b.md).

3b of the inline caches ([dispatch.md](design/dispatch.md), "3b — The
caches"), after 3a and ahead of 3c. Every send the optimized lowering
compiles consults a cache at its own call site, then a global cache, before
it runs a lookup — the per-site caches of Strongtalk and V8, with Primordial
Soup's two-probe global cache behind them — and a cache records what the
lookup decided and never decides.

**What changed**, in the order it landed:

1. **The profile** (`e6453b4`): 3a's head profiled over the seven sources,
   so the lookup a cache can remove was measured before anything was built —
   13.5–22.4% of the timed samples ([execution.md](design/execution.md),
   "3a's head, before the caches").
2. **The outcomes a cache could change, pinned first** (`85275a2`): six
   fixtures (`tests-fixtures/cache_*.ns`) and two golden outcomes — nested
   classes sharing behaviour, polymorphic sites, a slot under two
   applications, a private lexical site, class objects and instances at one
   site, a `super` send reaching the factory check, three kinds of
   `doesNotUnderstand:` handler, and a primitive that fails — all checked
   against the uncached code under both lowerings.
3. **Dispatch ids** (`8792dea`, `431b2dd`): a dense per-heap pair per mixin
   application, minted by `apply_mixin`, for its instances and for its class
   objects, and `Heap::dispatch_id`, the one reader. A class object stores
   only the instances id and derives the class side's as the next, so a
   release `Object` stays 56 bytes and `CLAUDE.md`'s 16.9x figure stands; a
   release-build test now pins the 56.
4. **Call sites per code family** (`401817a`): a send's `CallSite` is its
   `u32` number among the sends of its root body and every block in it, each
   body records its `family_size`, and the reference lowering emits
   `CallSite::UNCACHED`. The four send tags 7–10 are retired and the sends are
   21–24; the code record's call-site list is written empty, and the decoder
   refuses a non-empty one and a family numbered other than densely
   (`CallSiteListNotEmpty`, `CallSitesNotDense`). The golden image stayed
   1235 bytes: each send's site grew by 2 bytes, and each send's entry in
   the list, 2 bytes, went.
5. **`feedback_base` on every activation** (`f8fdcfa`): a stack entry, a
   closure, `Step::Method` and `vm::run` carry where the running family's
   range starts in the heap's cache table, which a family is given the first
   time its root runs. Measured on its own, level with its parent.
6. **Resolve, then act** (`7d1f21d`, `1f4d6c6`): every send's decision split
   into its lookup (`interpreter/resolve.rs`, answering a `Resolved` and the
   `Chain` it was found on) and `act`, the one place that runs it, with every
   error text moved word for word. The split pushed `vm::run`'s step out of
   line and cost every source 6.6–13.6%; keeping `decide_super_send` out of
   the step brought it back in line and left `bench_loop.ns` 3.8–4.3% above
   its parent, a miss accepted and carried into the caches.
7. **The caches** (`ead0d85`, `a702875`): per-site entries (24 bytes) going
   monomorphic, polymorphic in blocks of 4 (96 bytes) and megamorphic; the
   global cache (1024 entries of 32 bytes) keyed by id, selector and rule;
   `inline_cache::send_public`, `send_lexical` and `send_super`, which
   `vm.rs` calls with the base, the family size and the site. The caches
   pushed the step out of line again, and `#[inline(always)]` on it is what
   kept their gain; every source then came out 7.5–13.8% below its parent,
   entirely below it, 3a's head and `be3eb40` in both sittings.
8. **Invalidation and the stress mode** (`1457efd`, `f12dd27`, `c9c3c0f`):
   `Interpreter::invalidate_caches`; `NS_NEXT_STRESS_CACHES=1`, a new gate,
   invalidating at every safepoint, under which the tests that observe cache
   states turn the mode off for their own interpreter and still run;
   `NS_NEXT_GLOBAL_CACHE_ENTRIES` (debug builds) and a run of every fixture
   with a one-entry global cache; `Interpreter::cache_statistics` and
   `ns-next run|bench --cache-statistics`.
9. **The measurement of the whole step, and of a 16-byte entry**, on no
   commit of this branch; the 16-byte entry was an experiment and was not
   merged.
10. **The documents.**

The plan sharpened five points of the design and three more were ruled on
during execution; the user went through all eight on 2026-09-17
([dispatch.md](design/dispatch.md), "Decisions"). Confirmed as built: a
private lexical site guarded on N's application, a block's `family_size`
being its own subtree's, one stored dispatch id, and the forced step
inlining. Replaced by decisions built in the next step: a vacant lexical site
skipping the global cache (the lexical global key takes N's application
instead), the factory re-checks (the primary factory becomes a class-side
method, §6.3, which also corrects a non-conforming fixture), the decoder's
density check (call-site numbers are no longer serialized), and the global
cache's one-probe insert (a two-slot least-recently-used group instead).

**Measurement.** Whole, against 3a's head (`49a2a38`) and `be3eb40`, in two
sittings by the protocol: every source is 4.3–11.3% faster than 3a's head
and 9.4–19.7% faster than `be3eb40`, every group entirely below both. An
attribution binary — 3a's head with only the forced step inlining — is level
with 3a's head on every source (within ±2.1%), so the caches account for the
whole gain and the forcing only keeps it. After the benchmarks the table
holds 12–16 sites and the global cache 9–13 entries; after each
`run-corpus` module, at most 2 of each; no site went polymorphic. The
16-byte entry measured no faster, `bench_send.ns` entirely slower in one
sitting, and carries an unbounded growth risk; the user kept the 24-byte
entry. See [execution.md](design/execution.md), "What the inline
caches moved".

**A lexical send's decision is no longer sensitive to codegen.** Spec-conformant lookup had measured a variant that changed only
how the lexical decision's result travelled at 3.4–14.7% slower. With the
caches, `bench_send.ns`'s two sittings are 0.14% apart and `bench_block.ns`'s
1.3%, and the attribution binary, a codegen-only change, moved both by less
than 1%, every group overlapping start's.

Gates: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets`
warning-free; `cargo test --workspace`, 760 tests passing with one ignored,
in about 12 s with the test binaries built; the same tests passing under
`NS_NEXT_STRESS_MOVES=1` (about 3 min 9 s) and under
`NS_NEXT_STRESS_CACHES=1` (about 1 min 5 s); `parse-corpus` at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under both stress
modes. No recorded outcome moved.

## M4 — The goldens directory (2026-09-18)

Plan: [ns-next-goldens](superpowers/plans/2026-09-18-1355-ns-next-goldens.md).

Test infrastructure rather than runtime work, but it changed a gate
`CLAUDE.md` names, so it belongs here. `tests-fixtures/` became `goldens/`:
a `.ns` program and a `.toml` beside it recording `status`, `stdout` and
`stderr`, filed under eight category directories and found by walking rather
than from any list, with anything at another depth refused by name instead of
skipped. `crates/ns-cli/tests/program_outcomes.rs` was dissolved into it --
its 33 tests existed only because the old `.expected` sidecar could not record
a non-zero exit -- and the hardcoded floor of 42 gave way to a two-way pairing
check, which catches the half-move the floor was there for and cannot catch a
whole-pair deletion, the trade for having no count.

A `skipped` key records why the runtime does not answer a golden yet, with the
other keys still holding what it should. That turned the deviations table from
a description into a work list: nine goldens were skipped then, six for gaps
the table already recorded and three for rules nothing had pinned; none is
skipped today.

Two specification-coverage audits ran over the whole document, section by
section, and found more than the gaps: §6.3's five-step instantiation order
and §5.4.4's cascade receiver were both unpinned, the second one relied on by
nearly every golden. The cascade rule is where the specification's own prose
and its grammar disagree; canonical Newspeak settles it
(`corpus/modules/NewspeakASTs.ns:170`, `CascadedSendAST>>receiver` is
`^self previousSend receiver`), and ns-next agrees. Thirteen goldens and five
deviation rows came out of the two audits.

88 goldens, 79 running. No recorded answer changed in the move, which was
checked mechanically at each step rather than by eye.

## M4 — Flat metaclasses, the factory and the cache refinements (2026-09-18)

Plan: [ns-next-flat-metaclasses-factory-and-cache-refinements](superpowers/plans/2026-09-18-0211-ns-next-flat-metaclasses-factory-and-cache-refinements.md).

The step between 3b and 3c: five changes the user decided on 2026-09-17 and
2026-09-18, three of them conformance corrections and two cache refinements
([dispatch.md](design/dispatch.md), "Decisions"). Designing the factory
change is what produced the first two — §3.2 settles, in a passage about
metaclasses rather than about instantiation, that a class side is not
inherited at all, and an inherited class side is exactly what made an
inherited factory reachable.

**What changed**, in the order it landed:

1. **Metaclasses are flat** (`1fe05e3`, `cf97de9`). A metaclass's application
   has no superclass application, so a class-side chain finds a member on the
   receiver's own metaclass or not at all; the class object's superclass link
   pointed at `Object`'s behaviour, standing in for the `Class` §3.2 makes a
   metaclass's superclass until `Behavior`, `Class` and `Metaclass` reified
   it, so a class-side `super` reached `Object` and a class-side `super new`
   no longer instantiates. Two recorded outcomes
   were re-cut: `tests-fixtures/class_side.ns`, whose subclass now declares
   its own `describe`, and `cache_super_send_reaches_the_factory_check.ns`,
   which rested on an inherited class-side `doesNotUnderstand:` and is
   replaced by `tests-fixtures/class_side_super_reaches_object.ns`.
2. **The primary factory is an entry of the metaclass mixin's method table**
   (`599cc09`, `c04e22c`), `MethodEntry::Factory(FactoryMethod { selector,
   argument_count, access })`, found by the ordinary lookups. 3b's three
   special cases went with nothing in their place: the factory check ahead of
   lookup, the `Factory` target's re-check, and the re-check ahead of a
   `super` send's `doesNotUnderstand:` target. `decide_factory` still reads
   `ClassData.builtin` off the receiver on every send, because `Array` and
   `OrderedCollection` need an `ObjectData::Array` payload, so that read stays
   on the list of reads a cache key does not cover.
3. **A class side may not declare the primary factory's selector**
   (`562d06a`, `369e72a`, `a466c5d`): the resolver refuses it in the check
   that already reports a name declared twice on one side, as Primordial
   Soup's `checkNameConflictsForFactory:` does.
4. **The global cache's lexical key carries N's application** (`341ab02`,
   `0631dfd`, `da8beed`), as a fourth component beside the guard, the
   selector and the rule, so a still-vacant lexical site may read the global
   cache — which 3b's rule forbade — by probing N's `LexicalPrivate` row and
   then the `Lexical` row keyed by `(o, N)`. The component became
   `CacheRule::Lexical(DispatchId)` at step 6 below, which gave `GlobalEntry`
   its separate field back.
5. **Call-site numbers are off the wire** (`518686a`, `c72482e`, `76d9284`,
   `c9097d1`): the decoder assigns every family's numbers in the compiler's
   emission order once the whole code table is read, so a number is a
   property of the code and never read from an artifact. The send tags 21–24
   retired for 25–28, `RETIRED_OPCODES` is `[7, 8, 9, 10, 21, 22, 23, 24]`,
   and the code record's call-site list and the `CallSitesNotDense` check went
   with them; a code record carries a flag for a family the reference
   lowering compiled uncached. The golden image shrank from 1235 to 1154
   bytes.
6. **The global cache is 512 two-slot least-recently-used groups**
   (`569fdda`, `393311f`), Strongtalk's `lookupCache` rule: one hash picks a
   group, a hit at the second slot swaps the two, and a miss moves the first
   slot's entry to the second. `tests-fixtures/bench_megamorphic.ns` was
   added for the comparison.
7. **The measurement of the step, the control binary that bisected its one
   regression, and the documents** — this section, and
   [execution.md](design/execution.md), "What the step before 3c moved" and
   "Which half of the lexical key costs the send benchmarks". The control
   binary was a throwaway, on no commit of this branch.

**Measurement.** Whole and one change at a time, against the commit the step
started from (`f99a29f`) and against `be3eb40`, in two sittings by the
protocol, with `tests-fixtures/bench_megamorphic.ns` as an eighth source. The
three instantiation sources are 2.1–4.1% faster — `X new` is an ordinary
cached send now — entirely below in both sittings for `bench_new.ns` and
`bench_newarg.ns` and in the second for `bench_new2.ns`; and the other five
are 0.2–3.6% slower, `bench_loop.ns` and `bench_send.ns` entirely above in
both.
Every source is 6.7–18.2% below `be3eb40`, entirely below in both sittings.
Attributed: the conformance changes bought instantiation 3.3–4.6% and cost
`bench_do.ns` 2.8–3.3%; the lexical key cost `bench_loop.ns` and
`bench_send.ns` 2.1–3.3%, which a control binary put on the widened key
rather than on the two probes it adds; call-site numbering and the cache
groups moved nothing beyond noise.
`vm::run` is still one function with no `vm::run::closure` out of line, 4,926
instructions against 4,910 before the step, checked with `dumpbin /disasm`
at every checkpoint.

**The control binary.** The lexical key's commit does two separable things:
it widens the global key by a fourth component, N's application, which
`vm::run`'s step inlines, and it has `send_lexical_first` probe two
global-cache rows, which the wider key makes safe. A throwaway binary with
the probes cut out and everything else kept — `vm::run` instruction for
instruction the commit's, since `send_lexical_first` is `#[inline(never)]` —
separates them across two sittings: the control alone carries the loss
(`bench_send.ns` +2.25% / +1.66% and `bench_megamorphic.ns` +1.00% / +1.54%
against the parent, entirely above in both sittings), and adding the probes
moves no source beyond the noise. **The widened key costs it; the probes cost
nothing**, which is what the site counts had only argued. The binary was not
merged ([execution.md](design/execution.md), "Which half of the lexical key
costs the send benchmarks").

**What the benchmark for the cache groups settles, and what it does not.**
`bench_megamorphic.ns` fills 27 of 1024 global slots, five of them at the one
megamorphic site it drives; 27 keys over 512 groups collide less than once on
average and eviction needs three in a group, so on the hot path every read
almost certainly answers at the first slot and the replacement rule almost
certainly never runs. It measures
the probe path and cannot measure the replacement rule, which therefore
stands on Strongtalk's precedent rather than on numbers of this runtime's own
([OPEN-WORK.md](OPEN-WORK.md)).

Gates: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets`
warning-free; `cargo test --workspace`, 777 tests passing with one ignored;
the same under `NS_NEXT_STRESS_MOVES=1` and `NS_NEXT_STRESS_CACHES=1`;
`parse-corpus` at **210/0/1/0** and `run-corpus` at **6/0/205**, plain and
under both stress modes. Two recorded outcomes moved, both deliberately:
`class_side.ns`, and `cache_super_send_reaches_the_factory_check.ns`, which
pinned a class-side inheritance §3.2 forbids and was replaced.

## M4 — The dispatch id's place (2026-09-19)

Plan: [ns-next-dispatch-id-place-3c](superpowers/plans/2026-09-18-2215-ns-next-dispatch-id-place-3c.md).

The last part of step 3. **A guard reads the dispatch id from the object
header** (`Object.dispatch_id`), one load and one compare, and a class
object's data is boxed (`ObjectData::Class(Box<ClassData>)`) to pay for the
four bytes, so an `Object` is still 56 bytes in a release build and 80, down
from 96, in a debug one ([dispatch.md](design/dispatch.md), "3c — The
dispatch id in the object header").

**What changed:** two tests pinning the ids of a reapplied class and of a
decoded graph (`27797ed`); the id in the header, stamped at allocation,
restamped for every live instance when `Heap::set_class_application` changes
a class's application, and stamped once for every decoded object after
`rebuild_applications`, with a debug-build check in `Heap::dispatch_id` that
the header agrees with the class (`681126d`); `ClassData` boxed (`5d3aeff`).

**Measurement.** Both header variants against `393311f` and `be3eb40`, two
sittings by the protocol, eight sources. Neither is faster than `393311f` by
the protocol's gate: the kept variant's means moved −0.4% to −2.8% in 14 of
16 cells, never entirely below in both sittings. The header alone grows an
`Object` to 64 bytes, the release amplification to 18.2x and
`bench_block.ns`'s peak working set by 2.6%; the boxed variant keeps 56 bytes
and 16.9x, and takes the debug amplification from 24.9x to 22.2x. Every
source is 8.3–19.4% below `be3eb40`. `vm::run` is still one function, 4,878
instructions against 4,926 at `393311f` ([execution.md](design/execution.md),
"What 3c moved").

**The decision departs from the proposed rule**, which would have kept the id
on the class object since neither variant beat `393311f`. The user kept the
header with the box, on architectural grounds: a JIT tier's guard is one
compare at a fixed offset instead of two dependent loads, a class check and a
class-side branch, as in V8, HotSpot, Cog and Primordial Soup; step 6's
reified metaclasses make the stamping uniform; and of the two header variants
the boxed one is the smaller for the same speed. Accepted: a further
indirection on every read of a class's data, measured level on the
instantiation and block sources; an allocation per class object; and a copy
to restamp wherever a class's application changes, which `become:` will have
to exchange as well ([liveness-and-reflection.md](design/liveness-and-reflection.md)).
If closure conversion shrinks `ClosureData` to 24 bytes or less, `Object`
reaches 48.

Gates: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets`
warning-free; `cargo test --workspace`, 750 tests passing with one ignored,
and the release suite with the size test; the same under
`NS_NEXT_STRESS_MOVES=1` and `NS_NEXT_STRESS_CACHES=1`; `parse-corpus` at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under both stress
modes. No recorded outcome moved.

## M4 — The specification gaps (2026-09-19)

Plan: [ns-next-specification-gaps](superpowers/plans/2026-09-19-0300-ns-next-specification-gaps.md),
committed on the branch ahead of Task 1 (`4ea0571`), over the design decided
with the user on 2026-09-19 (`56e2c1f`). Branch `feature/specification-gaps`,
started from `6873f09`, on the checked-out tree throughout — no worktree for
the work itself.

**Decisions**, each taken with the user on 2026-09-19:

- **A canonical symbol table per heap, over an immediate symbol.** Each
  `Heap` keeps one, indexed by `Symbol::index` and filled lazily by
  `Mutator::new_symbol`, so a symbol literal, `asSymbol` and a
  `doesNotUnderstand:` handler's selector all reach the same object for the
  same text (§5.1.6, §3.1.1).
- **A duplicate symbol record is refused, not merged.** Decoding an image
  that already carries two records for the same text is a decode error
  rather than a silent merge into one object.
- **The canonical compiler's reading of a setter send, over SOMns's.** Only
  a local that declares a setter answers `id:` — the canonical compiler's
  rule (`prior-art/newspeak/NewspeakCompilation.ns`), not SOMns's
  compile-time refusal of an assignment to an argument, which its own `TODO`
  doubts and which also lets an immutable local be written freely
  ([dispatch.md](design/dispatch.md), "Setter sends").
- **The local-slot declaration taken into this step** (option A): a code
  body's `| x = e. |` and `| x ::= e. |` keep their initializer and run it,
  rather than being left for a later step, although nothing under
  `ns-syntax` was expected to change for runtime work.
- **The operand-copy elision taken into this step**, riding on the
  setter-send fix: a parameter's or an immutable local's register is never
  copied, since the resolver now guarantees nothing but its own
  initialization ever writes one.

**What changed**, commit by commit:

- `df0f347` — a §5.7 member set is keyed by a real class identity
  (`EnclosingClass`) instead of a class's simple name, closing the worst
  deviation the specification audits found: two same-named classes in one
  module no longer share one member set.
- `80b3ff9` — self-review correction to the member-set change's design text.
- `717300a` — a code body's local-slot declaration is kept and its
  initializer runs in declaration order, instead of the parser discarding
  everything but the name.
- `73921e1` — a setter send is passed by a parameter or an immutable local
  (§3.6): only a local that declares a setter answers `id:`.
- `ff1657d` — a parameter's or an immutable local's register is never
  copied, since nothing lowered after its initialization can write it.
- `652e6c5` — pins that a block parameter, not only a method parameter,
  needs no copy; corrects `CLAUDE.md` and `compiler.md`'s description of
  `lower_assign`'s `debug_assert!` scope.
- `7135854` — equal symbol literals are one object per heap (§5.1.6), through
  the canonical symbol table.
- `bc52d0b` — fixes a garbled test doc comment from the symbol-identity task.
- `67ba5c0` — `Heap::record_canonical_symbol` checks the stored object's
  handle origin, matching every other write accessor.
- `abd9bb4` — a decoded image enters its symbols into the heap's table and
  refuses a repeat record, instead of decoding one object per occurrence.
- `687c7b4` — records the step as built and measured: "What the
  specification gaps moved" in [execution.md](design/execution.md), this
  section, "Where the next session starts" rewritten to say the gaps are
  done, and the `:=` deviation documented in
  [README.md](design/README.md) and [frontend.md](design/frontend.md).
- `e48c8a4` — rewords [README.md](design/README.md)'s status-cell setter-send
  clause, found reading as a cut-off sentence on review, although
  grammatical.

**The final whole-branch review** (on Fable) found nothing Critical or
Important and agreed with both rulings below; four more commits close what
its minors and recommendations found:

- `36c4d34` — [dispatch.md](design/dispatch.md)'s member-set paragraph
  reworded to drop `resolver::build_member_sets`, a function that no longer
  exists in the codebase — docs describe the current state, and the history
  belongs in git.
- `71e1eb2` — `resolve_class`'s chain built via `EnclosingClass`'s `Copy`
  impl (`.copied()`) instead of rebuilding each field by hand.
- `bfe9cea` — `lower_assign`'s `StoreOuterLocal` branch (a write from a
  nested block into an enclosing frame's local) now `debug_assert!`s the
  value-register invariant too, so every write into a named local is
  asserted, not only the two `StoreLocal` branches; `CLAUDE.md` and
  [compiler.md](design/compiler.md) updated to say so.
- `7bf39e6` — the decoder refuses a decoded symbol record whose class is not
  the builtin `Symbol` class (`DecodeError::SymbolOfWrongClass`), since the
  canonical table would otherwise hand that object to every later literal of
  its text.

**Two rulings**, both recorded in the SDD ledger as the controller's:

1. `goldens/language/assignment_through_a_setter_answers_the_value_before_the_setter_ran`
   lost its `Init` case: the program wrote a factory parameter through a
   closure built in a slot initializer, which §3.6 now forbids outright (it
   raises `doesNotUnderstand: #n:`), and an initializer activation's only
   named locals are its factory parameters, which nothing can write any
   more — the hazard that case pinned, a slot initializer's value clobbered
   by its own setter, can no longer arise at all. The program relied on the
   bug the fix closed.
2. The new local-slot golden checks nil-ness with `== nil` rather than
   `printString`, because `nil printString` answered `'an Object'` and
   `nil isNil` was a skipped gap then; both were fixed by "`UndefinedObject`
   and the nil protocol".

**Measurement.** The head (`abd9bb4`) against `22a56f7`, the merge of 3c,
with `be3eb40` carried as the reference, two sittings by the protocol, eight
sources. **Level with `22a56f7` on every source** — no source entirely above
in both sittings — and 7.6–18.2% below `be3eb40`. `vm::run` stayed one
function, 4,878 instructions in both `22a56f7` and the head, no
`vm::run::closure` split out. The copy elision measured no gain on these
eight sources: none of them declares a local slot with an initializer or
writes a parameter, so it never fires on any of them. See
[execution.md](design/execution.md), "What the specification gaps moved".

Gates: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets`
warning-free; `cargo test --workspace`, 764 tests passing with one ignored;
the same under `NS_NEXT_STRESS_MOVES=1` and `NS_NEXT_STRESS_CACHES=1`;
`parse-corpus` at **210/0/1/0** and `run-corpus` at **6/0/205**, plain and
under both stress modes. One recorded outcome moved, deliberately (the first
ruling above): the `Init` case of
`assignment_through_a_setter_answers_the_value_before_the_setter_ran`.

## M4 — Removing `:=` (2026-09-19)

Plan: [ns-next-remove-assign](superpowers/plans/2026-09-19-2125-ns-next-remove-assign.md),
committed on the branch ahead of Task 1 (`8ac121b`). Branch
`feature/remove-assign`, started from `24e414d`, on the checked-out tree.
Decided with the user on 2026-09-19, while closing the specification gaps,
as its own small step ahead of closure conversion.

- `84abbdf` — every golden and runtime test that spelled `:=` is written as a
  setter send, while `:=` was still accepted, so the whole suite under both
  lowerings proved the two spellings answered the same: no `.toml` moved.
  The image wire-format fixture was regenerated in the same commit: its
  source spelled `:=` three times, so the source spans it records shifted by
  one to three bytes, and nothing else in the image differs.
- `dcf83ae` — the lexer refuses `:=` with a diagnostic naming the setter
  send, `TokenKind::Assign` is gone, and so are the parser's `:=` arm and the
  two guards that kept a unary loop off an assignment's target.
- The record, in the commit after it: frontend.md's deviation retired, the
  README row dropped, every design-document example respelled, and
  `bench_do.ns` written out in execution.md beside the other three throwaway
  sources, since the 2c plan's copy, an execution record, keeps its `:=`.

Not measured: nothing on the run loop's path changed, and the measurement
sources changed spelling only; all four throwaway sources were run from the
text in execution.md and print their recorded answers. `parse-corpus`
stayed at **210/0/1/0** and `run-corpus` at **6/0/205**, plain and under
both stress modes.

## M4 — Closure conversion (2026-09-20)

Plan: [ns-next-closure-conversion](superpowers/plans/2026-09-19-2344-ns-next-closure-conversion.md),
committed on the branch ahead of Task 1 (`faf527c`), over the design decided
with the user on 2026-09-19 (`891de70`, refined by `ec546dd`). Branch
`feature/closure-conversion`, started from `efac7d2`, on the checked-out tree
throughout — no worktree for the work itself, only throwaway ones for the
comparison binaries. Step 4 of M4; the design is
[execution.md](design/execution.md), "Closure conversion".

**Decisions**, all taken with the user on 2026-09-19 and unchanged by the
build ([execution.md](design/execution.md), "Decisions"):

- **A closure keeps its defining activation.** Only variable access moves to
  copies and vectors, because Newspeak's `enclosingActivation` and the
  debugger's "`[] in Foo bar`" need the link. Dropping it for blocks without
  `^`, as the JVM does, was rejected, as was keeping it behind a debugging
  flag, which a closure created without it could never recover.
- **A captured mutable variable is copied unless it is written after
  capture** — Primordial Soup's rule, plus a rule of ns-next's own for a write
  inside an inlined loop that also contains the capturing block, since an
  inlined loop's next iteration writes after this iteration's block was made.
- **One indirection vector per scope, as an ordinary `Array`**, as in Cog and
  Primordial Soup, rather than a box per variable; no inline small-vector
  representation for now.
- **The capture analysis is the optimized lowering's**, not the resolver's:
  only the compiler knows which blocks `control_flow::recognize` inlines.
- **A fallback closure keeps the by-reference model**, so that inlining stays
  unobservable and the common path pays nothing.
- **The reference lowering keeps the by-reference model as an interim
  oracle**, to be removed at some point, after which the recorded golden
  outcomes are how this is tested.

**What changed**, commit by commit:

- `e7ffe32` — the `goldens/capture/` set recorded on the unconverted tree,
  before any code changed, so that every outcome the conversion must preserve
  was pinned by a hand-derived answer first.
- `2de9780` — `compiler/capture.rs`: the analysis that classifies every local
  as a register, a copied value or a vector slot, and lists each real block's
  copied values. Nothing consumed it yet.
- `ea1bc90` — `ClosureData` carries `copied` and loses `parameters` and
  `home`; `CompiledCode` gains `copied_count` and `outer_reach`; a returning
  activation copies its registers into its reified frame only when its entry
  was marked; the wire format follows (`MakeClosure` retires tag 11 for 29,
  the closure record retires body tag 4 for 6, `copied_count` on a code
  record, and the decode checks for all of it).
- `001c0e1` — `fill_copied_values` writes straight from the object arena into
  the register window, so a block activation allocates nothing for it.
- `84b9bf5` — `MakeVector`, `LoadVector` and `StoreVector` end to end, wire
  tags 30–32; nothing emitted them yet.
- `a3f36a2` — the optimized lowering converts: `Layout` places a body's
  parameters, copied values, temporaries and vector register; `resolve_variable`
  is the one place a `Binding` becomes an access; `lower_fallback_send` keeps
  a fallback closure and everything nested in it by reference.
- `5441d05` — the protective register copies are asked per register
  (`a_send_can_write`, `writes_register`) instead of per expression class,
  with a golden for the transitive write-only shape.
- `fbd7d9f` — regression coverage for the `inlined_depth` arithmetic two
  levels deep, as a golden and as two lowering tests, both checked against
  two mutations of the shipped code.

**One defect found while building, fixed at its source.** The capture
analysis recorded a *read* across a frame boundary as a need but not a
*write*, so a block that only assigns an enclosing scope's variable — and
every real block between it and the declaring scope — was handed no vector to
write through, and the shapes failed to compile
(`internal: a variable the capture analysis did not hand this block`). The
bookkeeping was extracted into `Walker::note_need` and the write path now
calls it too; `goldens/capture/a_nested_block_that_only_writes.ns` pins all
three shapes. No recorded outcome moved, because the affected programs did
not compile before the fix.

**Measurement.** Four binaries — `efac7d2`, `be3eb40`, `a3f36a2` (the
conversion alone) and `fbd7d9f` (the head) — two sittings by the protocol,
eight sources, 320 timed data lines. **The head is level with `efac7d2` on
every source**, no source entirely above in both sittings, and 6–19% below
`be3eb40`, entirely below in both. The conversion on its own is level with
`efac7d2`, and the copy-narrowing is level with the conversion,
`bench_block.ns` included. `vm::run` stayed one inlined function and grew
from 4,878 to 5,700 instructions; no `vm::run::closure` split out on either
side, and no source paid for the growth in time. **`bench_block.ns`'s peak
working set rose about 2.6%** — 454.72–454.75 MB to 466.41–467.60 MB — and a
three-way measurement puts the whole rise on the conversion, with the
copy-narrowing neither adding nor removing any of it. The explanation, an
indirection-vector `Array` per activation of a block that writes a captured
local in an arena that reclaims nothing, is an inference from what changed
and not a measured attribution: no allocation profile was taken. See
[execution.md](design/execution.md), "What closure conversion moved".

Gates: `cargo fmt --all` clean; `cargo clippy --workspace --all-targets`
warning-free; `cargo test --workspace` green, and the same under
`NS_NEXT_STRESS_MOVES=1` and `NS_NEXT_STRESS_CACHES=1`; `parse-corpus` at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under both stress
modes. **No recorded outcome moved**: no golden `.toml`, no
`corpus/runnable.toml` row and no `tests/fixtures` outcome. Two artifacts did
change by design — the wire-format fixture `golden-image.bin` (1154 to 1176
bytes, the new closure record and `copied_count`) and the `disasm` snapshot
of `find:in:`, whose block now takes the sought value as a copied register
instead of a `LoadOuterLocal`.

## M4 — The moving collector (2026-09-20)

Plan: [ns-next-moving-collector](superpowers/plans/2026-09-20-1746-ns-next-moving-collector.md),
nine tasks over the design decided with the user on 2026-09-20
([memory.md](design/memory.md), "The shape: mark, compact, forward" onward).
Branch `feature/moving-collector`, started from `dc0ffe5`, the merge of
closure conversion into `master`, on the checked-out tree throughout. Step 5,
the last numbered step of M4.

**What changed, task by task:**

- Task 1 (`26669c6`) — `Forwarder`'s mapping becomes a parameter (a
  `Rotation` for the move stress mode, a forwarding table for the
  collector), and `trace.rs` stops being debug-only, since the collector
  needs the same edge enumeration in every build.
- Task 2 (`4a66f48`, `422b5d0`) — mark bits (`heap/mark.rs`, one `Vec<u64>`
  per arena, one bit per slot) and a marker: a second `Tracer` beside
  `Forwarder` that sets the bit and pushes onto an explicit work list where
  it was clear, iterative because the graph's depth is the program's to
  choose.
- Task 3 (`a7954cb`, `94b4a53`) — compaction and forwarding: one pass over
  the mark bits builds the forwarding table as a running destination index,
  one slides the live slots forward and truncates, one rewrites every handle
  through it; both arenas and one forwarding pass per collection, since a
  closure holds a frame and a frame holds objects. `World::collect` is the
  one entry point, called by `Interpreter::collect_now`, the trigger's
  request path and the collect stress mode.
- Task 4 (`d383a96`) — the canonical symbol table becomes weak: after
  marking and before compaction, an entry whose object is unmarked becomes
  `None`, and the rest are forwarded like everything else. The application
  cache stays strong, because its key is `Rc::as_ptr`, which needs the
  strong reference to keep the address unique.
- Task 5 (`843cc5a`) — the retention gate: `heap_statistics` answers live
  and occupied slots per arena, so a program that discards `n` closures is
  pinned back at its starting size and one that keeps `n` closures is pinned
  keeping `n` frames — false retention, which no other gate can see.
- Task 6 (`ba82a60`, `4b09c7f`) — the trigger: `push_object` and `reify`
  already hold the arena's length, so each compares it against a limit and
  sets the safepoint request when it is reached, rather than counting
  allocations. After a collection each arena's limit becomes
  `max(4096, live * 2)`, so a small program — under the 4,096-slot floor —
  never collects at all.
- Task 7 (`aea8459`, `0a0413c`) — `NS_NEXT_STRESS_COLLECT`, the third stress
  mode and third gate: collects at every safepoint, paced by heap size like
  the move stress mode, so a missed root is killed at the earliest possible
  moment and the recorded outcomes catch the difference. `run-corpus`
  already ran every listed module from source and as a built binary, under
  both lowerings (`CLAUDE.md`, "Working in this repository"); that existing
  built-binary path now also runs under this gate.
- Task 8 (`014f0ff`) — `bench --collector-statistics`, printed the way
  `--cache-statistics` already is: collections, total pause and longest
  pause, then live objects and live frames.
- Task 9 (this entry) — the measurement, against `dc0ffe5` and `be3eb40`,
  and the documents.

**Measurement.** Two sittings by the protocol, eight sources, every binary
built through `RUSTFLAGS` with `+crt-static` and the function-alignment pin.
`vm::run` stayed one function, 5,732 instructions against 5,700 at closure
conversion. **Peak working set is the point of the step**: `bench
--iterations 10` on `bench_block.ns` falls from 228.8 MB at `dc0ffe5` to
5.6 MB at the head, level with `bench_loop.ns` and `bench_send.ns`, which do
not move. **Seven of the eight sources are level with `dc0ffe5`** (means spanning
−2.1% to +2.9%) **and 5.3–18.0% below `be3eb40`.** Six of the seven overlap
the baseline's group in both sittings; `bench_newarg.ns`, the narrowest at
+2.9% pooled, is entirely above `dc0ffe5` in the second sitting and
overlaps it by only 0.01 ms in the first, and collects second most often of
the eight, 107 times.
`bench_loop.ns`, `bench_send.ns` and `bench_megamorphic.ns` report zero
collections and are exactly the three whose throughput does not move — the
recorded expectation named the first two in advance, and `bench_megamorphic.ns`
fits the same pattern. **`bench_block.ns` is not
level: +12.2% against `dc0ffe5`, the one source whose pooled groups do not
overlap in either sitting and the only one beyond a couple of percent**,
still 5.3% below `be3eb40`. It is the only source that allocates
a closure per iteration, so the only one that collects enough to show: 269
collections, 15.479 ms total pause, 0.202 ms longest, over
`bench --iterations 10`. Pause accounts for roughly 1.4 ms of the 2.46 ms
the head adds per round; the rest is unexplained ([OPEN-WORK.md](OPEN-WORK.md)).
See [execution.md](design/execution.md), "What the moving
collector moved", and [memory.md](design/memory.md), "How the collector is
measured", for the full tables.

Gates: `cargo test --workspace` green (813 tests passing, one ignored);
`parse-corpus` at **210/0/1/0** and `run-corpus` at **6/0/205**. The full
stress-mode and clippy gates were run task by task as each was committed
(tasks 1–8); this entry is documentation only, over code none of it changes.
No recorded outcome moved.

## Leaf inlining (2026-09-21)

Plan:
[ns-next-leaf-inlining](superpowers/plans/2026-09-21-2107-ns-next-leaf-inlining.md),
six tasks. Branch `feature/leaf-inlining`, started from `6f5a5b1` on
`master`, on the checked-out tree throughout. The first part of step 6, the
core library in Newspeak over a VM mirror, and sequenced ahead of the rest of it
([core-library.md](design/core-library.md)): once the library is Newspeak
over a VM mirror, every arithmetic operation in it is a send, so the fast
path has to exist before the rewrite rather than after.

**What changed, commit by commit:**

- `07b8d92` — `recognize_integer_binary` (`compiler/control_flow.rs`), the
  arithmetic counterpart to `recognize`, answering an `IntegerOperation` for
  the nine binary integer selectors behind the same `is_ordinary_send` gate.
  `/` is left out because an integer has no `/` primitive at all.
- `dd15105`, `a0fa984` — `Instruction::IntegerBinary`, wire tag 33, and a
  round trip over every `IntegerOperation` through the image format.
- `b0b9d6e`, `c87645e`, `2b52b7c` — the VM arm and the lowering. The arm
  computes or declines and raises nothing; the lowering emits the
  instruction, an unconditional `Jump` past the fallback, and the ordinary
  `SendPublic` as the fallback, which carries a call site of its own.
- `e586db9`, `88d86d9` — the goldens: `goldens/arithmetic/` for the fast
  path beside the two ways the guard declines on ordinary values and floor
  division on the fast path, and `goldens/errors/` pinning that a zero
  divisor still carries the primitive's own message.
- This entry — the measurement and the documents.

**Measurement.** Two sittings by the protocol, eight sources, both binaries
built through `RUSTFLAGS` with `+crt-static` and the function-alignment pin,
the baseline `6f5a5b1` in a throwaway worktree. `vm::run` stayed one
function: 5,732 instructions at the baseline — the same figure recorded at
the moving collector — and 5,866 at the head, the `IntegerBinary` arm's 134.
**Every one of the eight sources is faster, by 11.5% to 31.0%, and every
after-group lies entirely below its before-group in both sittings**:
`bench_loop.ns` −31.0%, `bench_megamorphic.ns` −30.0%, `bench_send.ns`
−26.3%, `bench_new.ns` −20.1%, `bench_do.ns` −17.8%, `bench_new2.ns`
−16.9%, `bench_newarg.ns` −14.1%, `bench_block.ns` −11.5%. The expectation
was recorded before any binary was built, and it was right at its ends and
wrong in its middle: it named `bench_loop.ns` as the largest gain and
`bench_block.ns` as the smallest, both of which held, but it predicted
3–5x smaller magnitudes throughout and put the whole "small" group — which
landed at 11.5–20.1% — below a "moderate" group that came out at 17.8–30.0%. Its premise was the error —
it assumed an instantiation dwarfs three arithmetic sends, and the numbers
say the two are of the same order. What the ordering does track is the share
of a source's per-iteration sends the step removed, discounted by how
expensive the ones left behind are; `bench_block.ns`, which keeps a method
activation, a closure allocation and a block activation, is the extreme.
See [execution.md](design/execution.md), "What leaf inlining moved", for the
tables and for what is left unexplained.

**A known cost.** A successful operation also executes the unconditional
`Jump` past the fallback, so the hot path is two dispatches where one would
do. It does not show above the noise — `bench_loop.ns` removed 33% of its
per-iteration sends and gained 31.0%, which is the tightest the suite gets;
dropping the `Jump` is in [OPEN-WORK.md](OPEN-WORK.md).

Gates: run task by task as each code commit landed, and green at `88d86d9`,
the branch's last code commit — `cargo test --workspace`, `cargo clippy
--workspace --all-targets` clean, `parse-corpus` at **210/0/1/0** and
`run-corpus` at **6/0/205**, the stress modes included. This entry changes
Markdown only, so none of them was re-run for it. No recorded outcome
moved.

**Carried forward, reviewed and deliberately not fixed.** The whole-branch
review triaged each of these and judged none of them worth holding the merge
for; they are recorded because a reviewed decision to leave something is worth
more to a later reader than the silence that usually replaces it.

- **`lower_cascade` does not route through `lower_send`**, so a cascade such as
  `x + 1; + 2` keeps ordinary sends for its integer leaves. Behaviour is
  identical; only the optimization is missed. Widening it means reworking the
  cascade's shared receiver register, which is why it was not done here.
- **The aliasing test hardcodes register 0** for the local it watches, with a
  comment saying why that is the register. Deriving it from the emitted
  `StoreLocal` would make the test self-describing; as written it fails loudly
  rather than silently if the layout ever changes.
- **No unit test pins the two patched jump targets.** The four `insta`
  disassembly snapshots pin them exactly, and `patch_target`'s `unreachable!`
  makes a missing arm loud rather than silent, so this is coverage breadth.
- **The image round-trip fixture exercises `Add` only.** The tag mapping itself
  is covered for all nine operations by `every_integer_operation_tag_round_trips`;
  the instruction's other fields do not vary per operation.
- **The standalone opcode-tag test duplicates the line added to
  `instruction_tags_are_fixed`**, and reaches for a fully-qualified path where
  the file glob-imports the module.
- **`bench_megamorphic.ns` removed the largest send share (42%) and did not gain
  the most.** The discount model in "What leaf inlining moved" accommodates it —
  the sends it keeps are megamorphic and so dearer — but that discount has no
  independent measurement, which leaves the model as stated unfalsifiable on this
  row.
- **"The only earlier step to clear it across its whole suite was 3b"** is true
  for a step measured against its own baseline; several cumulative comparisons
  against `be3eb40` also show every source entirely below in both sittings.

## The core library over a VM mirror (2026-09-22)

Plan:
[ns-next-kernel-over-vm-mirror](superpowers/plans/2026-09-22-0107-ns-next-kernel-over-vm-mirror.md),
sixteen tasks over nine decisions taken with the user on 2026-09-21 and
2026-09-22 ([core-library.md](design/core-library.md), "The core library as
Newspeak source over a VM mirror"). Branch `feature/kernel-over-vm-mirror`,
started from `ef4a9da` on `master`, on the checked-out tree throughout. The
first of step 6's three remaining plans.

**What changed, commit by commit:**

- `fc94e84` — `VmMirror`, a builtin class with builtin class tag 14, whose
  method table carries every leaf as one row of `builtins/primitive.rs`'s
  `MIRROR`, and its one instance, created at bootstrap.
- `f5a5efc` — `crates/ns-runtime/core-library/Kernel.ns`, resolved by
  `resolver::resolve_kernel`, the only resolver that records
  `Binding::MirrorSend`; the bootstrap fills the builtin class objects it
  built as placeholders from the kernel's nested classes and instantiates
  `Kernel vmMirror:` with the mirror.
- `da695fd`, `f58b84c` — `Instruction::SendMirror`: a mirror send's literal
  `ifFail:` block lowered inline, with no fallback send and no wire tag
  ([compiler.md](design/compiler.md), "Mirror sends").
- `604a023` — primitive methods: a kernel method whose body only forwards
  runs its row's leaf in `act` without an activation, under both lowerings,
  and `NS_NEXT_NO_PRIMITIVE_METHODS=1` to measure without them
  ([dispatch.md](design/dispatch.md), "Primitive methods").
- `ed6a93f` to `ff7f543` — one family per commit: `Object`; `Boolean`, with
  `True` and `False` now under it; `Integer`; `Float`; the text classes; the
  collections; `Block`; `Platform`.
- `90632b1` — the per-class primitive lists and the pass-over a failing
  primitive took are gone, and the one observable difference is pinned.
- `52c242b` — `build` refuses an entry only the core library answers, since
  its entry check now stops at the first builtin level.
- `094db4a`, `5c76345` — the measurement and the documents; `3e0ac9f` — the
  whole-branch review's fixes, among them a test that every kernel method is
  a recognized primitive method named after its row.

**Measurement.** Two sittings by the protocol, eight sources, against
`e2bb31e`, the merge of leaf inlining, and `be3eb40`, every binary built
through `RUSTFLAGS` with `+crt-static` and the function-alignment pin.
**With primitive methods the move cost nothing measurable: −4.5% to +2.1%
against `e2bb31e`, no group entirely above in either sitting**, and 24.7% to
40.2% faster than `be3eb40`. Without them, on the same binary, the three
sources named in advance were clearly slower — `bench_megamorphic.ns`
+23.6%, `bench_block.ns` +21.3%, `bench_do.ns` +5.5% — which is what the
primitive methods recover. Start-up is 0.5 ms slower, the kernel's parse,
resolution, compilation and instantiation. `vm::run` grew from 5,866 to
6,385 instructions and stayed one function. See
[execution.md](design/execution.md), "What the core library's move cost",
for the tables and what is left unexplained.

**One observable difference, decided (Decision 9).** A builtin subclass's
own `doesNotUnderstand:` is now passed by, as §5.10 says: a declining leaf's
failure block sends `super …`, which ends in `Object`'s handler rather than
continuing the receiver's lookup. No golden had pinned the old behaviour;
`goldens/kernel/a_builtin_subclass_handler_is_passed_by_as_section_5_10_says`
pins the new one ([core-library.md](design/core-library.md), "The one
observable difference"). No other recorded outcome moved.

Gates, at `3e0ac9f`: `cargo test --workspace` green (868 tests passing, one
ignored), also under each of the three stress modes; `cargo clippy
--workspace --all-targets` clean; `parse-corpus` at **210/0/1/0** and
`run-corpus` at **6/0/205**, under each stress mode too.

## `Top`, `Value` and the value semantics (2026-09-23)

Plan:
[ns-next-value-and-top](superpowers/plans/2026-09-22-2112-ns-next-value-and-top.md),
eight tasks over the decisions taken with the user on 2026-09-22, the central
one being that a value object has no observable identity: a value may be
copied across an actor boundary, so its equality and hash are derived from
its state and its identity method is a send of `=`. Branch
`feature/kernel-over-vm-mirror`, on the checked-out tree throughout. Step 6's
first plan gave the core library the specification's *dispatch*; this one
gives it the specification's *object model*
([core-library.md](design/core-library.md), "What exists";
[object-model.md](design/object-model.md), "The builtin classes").

**What changed, commit by commit:**

- `1dc548a` — `crates/ns-runtime/kernel/` is `crates/ns-runtime/core-library/`,
  since `kernel` named only the bootstrap phase that installs it, and every
  nested class of `Kernel.ns` says `public`: §6.3.1 defaults a member to
  protected, which would leave a kernel class unreachable from any other
  module.
- `7d7026d` — `Top`, `Value` and `Number`, and with them §3.2's and §3.1.1's
  hierarchy. `BuiltinClass::ALL` is a wire-tag order and no longer a
  superclass order, so the bootstrap fills the placeholders in `fill_order`,
  sorted by depth below `Top`.
- `1932c0e` — `Top` is refused wherever an artifact names a class, as the
  mirror's tag is: §3.2 lets only `Object` name it, so no source module can
  and no image may either ([image-format.md](design/image-format.md)).
- `39fb961` — the builtin value classes hash by their value rather than by
  identity, which §3.1.1 requires of anything a copy must hash like.
- `d011a57`, `8b53296`, `7cfdee3` — the derivation: `builtins/value.rs`
  answers a value's equality and hash from its state without ever sending,
  four new `MIRROR` rows carry it, and `Value` declares `=`, `hash` and
  `==` over them with a Newspeak fallback for what the leaf declines.
- `55d3f60`, `ab692b7` — §3.1.1's two conditions, refused where an instance
  comes into being: a mutable slot below `Value` means no instance, and a
  slot of a value may hold only a value
  ([object-model.md](design/object-model.md), "A value object's two
  conditions").
- `a6edb79` — the measurement below.
- The fix below: a value's state is the slots its classes declare, so a
  nested class's hidden cache slot is no longer compared or hashed.

**Measurement.** Two sittings by the protocol, eight sources, `ab692b7`
against `b0a771d` -- the last commit of the work against this branch's head
before it started, which is documentation only, so its tree is the code as it
stood before. The value-state fix below came after the figures were taken and
touches no path any of the eight sources runs, so they stand for the head.
**Seven of the eight are
level: −0.7% to +1.9%, inside the scatter those sources show between builds
of the same tree.** `bench_new2.ns` is **3.6% slower**, all three head builds
above both baseline builds in both sittings; it was one of the three sources
the expectation named, but the two others that run the same initializers are
level, so the per-store check alone does not explain it. `vm::run` grew from
6,389 to 6,506 instructions and stayed one function, with
`instantiate::check_value_slot` inlined into its `InitializeSlot` arm.
Start-up did not move measurably. See
[execution.md](design/execution.md), "What `Top`, `Value` and `Number` cost",
for the tables and what would recover the 3.6%.

**A protocol finding.** `bench_block.ns` spreads 16% between builds of the
same tree, with the function-alignment pin in place and each build stable to
about 1% -- found because the first pairing put it 13.4% *faster* at the head,
which nothing the change touches could explain. Five builds, two of the
baseline and three of the head, land at five separate levels that straddle the
commit boundary. On that source one build per commit measures the build, not
the commit; the protocol paragraph in
[execution.md](design/execution.md), "What removing the re-entries moved",
now says so.

**What is enforced.** The slot half of §3.1.1's second bullet — no mutable
slot, and no slot holding a non-value — is refused where an instance comes
into being, which is every instance of every class, whether its class came
from source, from a nested declaration or from an image. The other half —
a value's enclosing objects are values, and its class defines no `==` —
came with mixin application and "The value conditions" below. What is
still unchecked is listed in [OPEN-WORK.md](OPEN-WORK.md).

**One defect, found while writing that section up and fixed the same day.**
A value class that declares a nested class compared wrongly
once the nested-class accessor had run: the hidden cache slot is a field
like any other, both the derivation's walk and `valueSlotCount:` counted it,
and each instance's own class object lands there — so `valueEquals:` sent
`=` to two different class objects and answered false for two values built
from the same arguments, at exit status 0. **The rule now is that a value's
state is the slots its classes declare and nothing else**, computed once per
application (`MixinApplication::declared_slots`, beside `nested_slot` and
`inherits_value`) and read by both Rust walks and both reflective leaves
through one wrapper, so the leaf and the Newspeak fallback cannot disagree
about which slots exist or in what order.
`goldens/kernel/a_values_nested_class_is_not_part_of_its_state.ns` and two
unit tests beside the walks pin it, the one-sided and the inherited case
included ([core-library.md](design/core-library.md), "A value's state is the
slots its classes *declare*").

**The final whole-branch review found five things, all fixed on
2026-09-23** (`0ccff43`, `95f2a07`, `cf1c5a0`, `0a5bb4e`, `3da730d`,
`ed70803`). `Float>>=` is class-tight, so `1.0 = 1` and `1 = 1.0` both
answer `false` and the leaf's class-first rule and `Float`'s own `=` are
one rule again — the ruling that makes the numeric tower needed
([OPEN-WORK.md](OPEN-WORK.md)). `Value`'s own application carries `inherits_value`, since §3.2's
"subclass of" is inclusive, so `Value new = Value new` and a slot holding
a `Value new` no longer disagree. Both value walks charge their node
budget when a node is *pushed*, which bounds the work vector by the budget
rather than by the heap. The documents no longer claim more of §3.1.1 than
is enforced. And both refusals name the factory the program sent. Two
goldens moved, both recorded by this branch:
`a_value_has_no_identity.ns`'s `Float/Integer` line from `true true` to
`false false`, and `value_hashes_follow_value_equality.ns` gained a
`Float/Integer` and an `Integer/Float` `cross:` line, each `false false`.

**Gates, at this branch's head** — not at `ab692b7`, since the value-state
fix above is code and tests: `cargo test --workspace` green, **892 tests
passing and one ignored**, also under each of the three stress modes;
`cargo clippy --workspace --all-targets` clean; `parse-corpus` at
**210/0/1/0**; `run-corpus` at **6/0/205** in all four lanes. Every commit
from `ab692b7` on has seen the per-commit group, and the end-of-batch group
was re-run at the head.

## Mixin application (2026-09-23)

Plan:
[ns-next-mixin-application](superpowers/plans/2026-09-23-0936-ns-next-mixin-application.md),
eight tasks. Branch `feature/mixin-application`, off `master` at `6833234`,
on the checked-out tree throughout. The user asked for §3.1.1's conditions
to be enforced strictly, and one of them is that a value's enclosing objects
are values; a module can be a value only if it can be declared one, which
SOMns writes as a mixin application (`class Kernel vmMirror: vmMirror =
Object <: Value (`). So `<:`, a recorded deviation since Milestone 1, came
first, as a language feature in its own right, and **what it unlocks is
§3.1.1's enclosing-object condition, which the value conditions enforce**
("The value conditions" below). The design is
[object-model.md](design/object-model.md), "Mixin application".

**What changed, commit by commit:**

- `ec3282c` — the parser: `<:` is one lexer token, and
  `class C = S ms <: M1 m1 <: … ( … )`, or the same ending in `.`, parses into
  `ClassDeclaration::mixin_applications`, each clause by the superclass
  clause's own parser ([frontend.md](design/frontend.md), "Grammar").
- `1adcc25`, `a547956` — the resolver resolves each clause's message in the
  declaration's factory scope, the compiler compiles one message body per
  clause, and `nested::build_applied_levels` builds one class level per
  clause, for a root class and a nested one alike: the named class's mixin
  over the level below, with the named class's enclosing object and
  declaring class (§6.2). A nested declaration's `<:` receivers are evaluated
  in its superclass clause's activation, which answers
  `{superclass. mixin…}`, so no step, frame kind or opcode was added for them.
- `fac4e2c`, `3e92ae5` — initialization: every clause message is evaluated
  with the applying class's factory arguments, its selector checked against
  the named class's primary factory, and each `mk` just before `Mk`'s
  initializer runs, following §6.2's text and matching SOMns: `ms`, `S`'s
  initializers, `m1`, `M1`'s, …, then `C`'s own. The first version evaluated
  every message on the way up; the review caught the order against §6.2's
  "Then, *m* is evaluated", and the fix defers each message to the descent
  (`Instantiation::deferred`).
- `6b281aa` — the image format: a mixin row carries its clauses, a class
  record names its mixin by a tagged reference (`MIXIN_ROW`,
  `MIXIN_BUILTIN`), so the class-record tag 5 is retired for 7, a root
  class's levels are rebuilt from the root-class section and named by
  `CLASS_ROOT_LEVEL` (class tag 3), and a clause message's frame is
  `FRAME_MIXIN_APPLICATION_MESSAGE` (6). A level whose clause named a builtin
  carries no enclosing object or declaring class, which keeps `Kernel` and
  the VM mirror out of every image ([image-format.md](design/image-format.md),
  "Sections", "Verification").
- `f1e4d0f`, `de05916`, `77d6e4d` — `goldens/mixin/`, a new category of ten
  goldens, each derived from §6.2 before it was run, and
  `goldens/spec/spec_6_2_a_mixin_application_derives_a_class` is no longer
  `skipped`.
- `1a06417`, `4e83254` and the documentation after them — the whole-branch
  review's fix pass: three more goldens for configurations the review probed
  and found right (a class nested in an applied mixin reaching two `outer`
  hops out, one mixin applied twice in one chain, and a mixin whose own
  declaration has `<:` clauses), and `verify_mixins` refuses a clause without
  arguments that carries a message body
  (`UnexpectedMixinApplicationMessage`).
- the fourteenth golden, `mixin_module_declares_itself_a_value`, pins the
  rule the user made explicit afterwards: a module declares itself a value
  with `= Object <: Value`, and a second instance of its class is `=` and
  `==` to it and hashes alike.
- `5ac5034`, `06fd024`, `6b0f36a`, `4f4ea13` — the measurement, the
  regression it found and its fix, below.

**Measurement.** The first measurement found a program with no `<:` paying
for it: instantiation 2.1–5.6% slower and nested-class creation 9.3%, every
head build above every base build, because the grown walk functions had
pushed `check_factory_arity`, `clause_message_body` and `argument_list` out of
line and the unboxed applied clause had moved `ClassData.application`. The
controller ruled it fixed before the documentation, since performance comes
first and the plan had promised that a declaration without `<:` takes the
path it took before. `06fd024` restored the ordinary paths plus one test per
level, moved the `<:` machinery into cold helpers and boxed the clause.
Measured again against `6833234`, three builds a tree and three sittings,
all ten sources are within the protocol's noise, −2.2% to +1.4%; four
instantiating sources sat 0.5–1.4% above the base in every build and
sitting, a difference whose cause is not isolated ([OPEN-WORK.md](OPEN-WORK.md)). `vm::run` is 6,508 instructions in both trees and one function. See
[execution.md](design/execution.md), "What mixin application cost".

**Gates.** Every commit that changed code or tests saw the per-commit group.
At the branch's final state the whole end-of-batch group ran at `56ecf79`, the last commit that changed code or tests:
`cargo test --workspace` had all 39 test results ok plain and under each of
`NS_NEXT_STRESS_MOVES=1`, `NS_NEXT_STRESS_CACHES=1` and
`NS_NEXT_STRESS_COLLECT=1`; `run-corpus` reported "ran 6, failed 0, skipped
205" plain and under each of the three; and `parse-corpus` reported "parsed
210, failed 0, expected-fail 1, stale 0".

## The value conditions (2026-09-24)

Plan:
[ns-next-value-conditions](superpowers/plans/2026-09-23-2252-ns-next-value-conditions.md),
eight tasks. Branch `feature/value-conditions`, off `master` at `cdc0257`,
on the checked-out tree throughout. It enforces all four conditions of
§3.1.1's second bullet strictly — a value's slots are immutable, they hold
values, its enclosing objects are values, and its class inherits `Value` and
does not define `==` — where the `Top`/`Value` plan had refused only the
slot half, and it adds the root rule, which closes the root-clause gap the
mixin-application branch recorded. The design is
[object-model.md](design/object-model.md), "A value object's four
conditions" and "Mixin application", "The root rule".

**Decisions, each taken with the user:**

- **Strict enforcement of every condition** (2026-09-23), in the order the
  design gives, each step leaving the suite green.
- **`Kernel` and the VM mirror are values, as in SOMns** (2026-09-23):
  `Kernel.ns` is `= Object <: Value` and the mirror's class sits below
  `Value`, so every builtin value class passes the enclosing-object condition
  by construction and the mirror is a value by the ordinary rule.
- **The root rule** (2026-09-23): a module's root may name only `Object` or
  `Value`, or write `Object <: Value`; every other root clause is a defect.
  The encoder refuses such a root, and the decoder a root section the rule
  forbids. Since 2026-09-30 the resolver refuses it ("The root rule is a
  resolve error" below).
- **Every golden declaring a value class inside an ordinary module becomes a
  value module and keeps its outcome** (2026-09-23), the rule decided for the
  two `class Text = String new` goldens, extended to every such module.
- **The two class conditions are decided cold and refused at instantiation**
  (2026-09-24), changed from the plan's refusal at creation because §2.3's
  own example creates a `String` subclass enclosed by a mutable object and
  never instantiates it (`goldens/spec/spec_2_3_superclass_of_a_nested_class.ns`,
  which passes as transcribed). Decided where, though, differs between the
  two: the enclosing-object condition at class-object creation, from
  `nested::after_clause`; the identity-method condition, moved by Task 6b's
  follow-up fix, once per application, from `Mutator::apply_mixin` -- which
  is what let it cover a module's root, since `apply_mixin` builds the
  root's application too, while `after_clause` never runs for one.

**What changed, commit by commit:**

- `7cc5467` — the root rule: the resolver binds no root clause receiver,
  `build_root_mixin` records a `RootClauseDefect`, `apply_mixin` folds it and
  the mutable-slot refusal into one `instantiation_refusal`, and
  `decide_instantiation` refuses it; the encoder and the root-section reader
  refuse it too. Ten goldens under `goldens/errors/root_*`.
- `727a613` — `Kernel` is `= Object <: Value`, its `Value` level built with
  `build_applied_levels` like a user root's, and its root mixin with
  `build_root_mixin`; the mirror's class moves below `Value`.
- `c8c2ab2` — every golden and inline test module declaring a value class
  declares itself a value, the transcript read where it is used or the
  module's methods moved into a nested `Script` class that is no value; no
  recorded outcome moved.
- `f3c2bd4`, `74e9e8e` — the mutable-slot check covers the whole chain,
  since a `<:` clause can put `Value`'s level below a class with slots
  (`first_mutable_slot`).
- `7c460fa` — a value class that defines `==` below `Value` is refused
  (`MixinApplication::identity_method`), a level a `<:` clause adds
  included.
- `d1032c9`, `4912499` — the enclosing-object condition, over every level of
  the class chain with an early stop at a class already checked, decided in
  `interpreter/value_class.rs` from `after_clause`, recorded on the class
  object (`ClassData::value_class_refusal`) and refused, with the identity
  method, at instantiation. `goldens/nesting/nested_override.ns`, which
  instantiated a `String` subclass enclosed by a mutable object, starts its
  overriding slot at `OrderedCollection` instead; its recorded outcome did
  not change.
- `866fddb`, `ef61c03` — the review's fix: the early stop reads the
  stopped-at class's own recorded refusal and inherits it, since a refused
  class still exists; and a decoded class object's verdict is recomputed on
  load, in a pass of its own after every application is installed, so a
  snapshot of a refused value class decodes refused.
- this entry's commit — the documents.
- **Task 6b, follow-up fix** — the no-identity-method condition moves from
  `ClassData::value_class_refusal` (decided at class-object creation, which a
  module's root never reaches) to `MixinApplication::instantiation_refusal`
  (decided once per application by `Mutator::apply_mixin`, which the root's
  application goes through like any other), closing the root gap this
  milestone entry used to record as a known gap. A golden and an image
  round-trip test pin a value module whose root defines `==`; every other
  recorded outcome held.
- **The final review's fixes.** `d704928`: a refusal inherited through the
  early stop keeps the level its own verdict names, rather than renaming it
  after the class the walk stopped at, so a fault passed on over two levels
  still names the faulty level
  (`goldens/kernel/a_value_two_levels_below_a_refused_value_class_names_the_faulty_level.ns`,
  new; no recorded outcome moved). `07bd995`: the root-section reader
  refuses a section naming no superclass (`DecodeError::RootWithoutSuperclass`),
  and the encoder refuses to write one. `521e6ec`: comment corrections.
  `7b2af64`: `decide_instantiation` tests for a class's refusal instead of
  cloning it, and both refusal errors are out of line and cold. The commit
  after it records the re-measurement.

**Measurement.** Against `cdc0257`, over three sittings and three layouts per
tree (varied with `-C metadata`, because three plain builds of one tree were
the same binary), six of nine sources are level (−0.6% to +0.6%),
`bench_newvalue.ns` is 3.3% faster, start-up did not move, and `vm::run` is
6,507 instructions against 6,508. `bench_block.ns` measured 4.5% slower and
`bench_new.ns` 2.2% slower, every head layout above every base layout in
every sitting. See
[execution.md](design/execution.md), "What the value conditions cost".

**Re-measured after the review's fix** (`7b2af64`, six sittings, the base,
the head before the fix and the head after it in three layouts each):
`decide_instantiation` is 371 instructions, below the base's 441 (503 before
the fix). `bench_new.ns` is level, +0.2% pooled, and the head before the fix
measured the same, so the earlier +2.2% did not reproduce in this session and
the fix is not what closed it. `bench_block.ns` is still slower and the fix
did not touch it: +4.4% against the base's two `-C metadata` layouts, +6.9%
pooled with the base's plain build, a fast layout outlier in this session.
A control for the seeded `==` symbol was run and refuted it: a base seeded
with `==` measures like the base, and the head stays 4.3% above it. What is
left of the 4.4% is in [OPEN-WORK.md](OPEN-WORK.md).

## The predefined classes as accessors on `Object` (2026-09-24)

Plan:
[ns-next-object-class-accessors](superpowers/plans/2026-09-24-2058-ns-next-object-class-accessors.md),
five tasks. Branch `feature/object-class-accessors`, off `master` at
`909bf40`, on the checked-out tree throughout. §3.2 has `Object` provide
"accessors for predefined classes and objects, such as the classes of the
built-in literals (5.1) e.g., `String`", so a bare `String` no enclosing
declaration declares is §5.7's `self String`; ns-next loaded it as a
constant instead (`LoadBuiltinClass`), which a class inheriting a member of
that name could not override. The design is
[core-library.md](design/core-library.md), "The predefined classes are
accessors on `Object`", and [dispatch.md](design/dispatch.md), "Binding
variants" and "Primitive methods".

**Decisions, each taken with the user on 2026-09-24:**

- **An accessor for every name a program could reach**, `Object` to
  `Block`, fifteen in all, wider than both references; `Top`, the mirror's
  class and `Platform` get none.
- **`public String = ( ^outer Kernel String )`**: the `outer Kernel` is
  required, since inside `Object` a bare `String` would find `Object`'s own
  accessor and recurse.
- **A fast path answers the accessor without an activation**:
  `MixinMethod.primitive` became `fast_path: Option<MethodFastPath>`, with
  `Mirror(row)` as before and `BuiltinClass(kind)` new, recognized only in
  `Kernel.ns` and read by `act` on every send.
- **`LoadBuiltinClass` and its wire tag 2 are retired**, never reused.
- **The mirror's capability lock moves** from "no bare name resolves to it"
  to "`Object` declares no accessor for it or for `Top`", pinned by a test.

**What changed, commit by commit:**

- `2a2721d`, `cba6881` — the design, and the correction that the accessor
  reaches the kernel through `outer Kernel`.
- `e1d3069` — the plan.
- `4f34a87` — `Kernel.ns`'s `Object` gets its fifteen accessors;
  `MixinMethod.fast_path` and `MethodFastPath` replace `primitive`;
  `ResolvedModule.kernel` confines recognition to the kernel; the recognizer
  refuses `Top` and the mirror's class; `act` answers `BuiltinClass` without
  an activation. `interpreter/kernel.rs`'s
  `objects_class_accessors_are_exactly_the_nameable_builtins` pins the set,
  and `crates/ns-runtime/tests/object_class_accessors.rs` the recognizer.
- `ed9d7f9` — a `super`-prefixed inheritance-clause receiver lowers to a
  real super send in both lowerings (§6.1), which the next commit needed:
  `super Object` had reached the builtin through the fallback it removes.
  Accepted by the controller; it closes the known gap `dispatch.md`
  recorded. One new golden.
- `46b9cf0` — the resolver binds a name no enclosing declaration declares
  `LexicalSend { depth: 0 }`, in method bodies and nested clauses alike;
  both lowerings emit `UnresolvedIdentifier` for an unbound name; the
  recognizer refuses `Platform` too. Six new goldens; no existing recorded
  outcome moved, and `run-corpus` stayed at ran 6 / failed 0 / skipped 205.
  `tests/fixtures/golden-image.bin` is regenerated: its nested class's
  `Object new` clause now sends `Object`, which adds that symbol.
- `dd1c70c` — `Instruction::LoadBuiltinClass` and
  `instantiate::builtin_kind_from_name` are deleted, and opcode 2 joins
  `RETIRED_OPCODES`.
- this entry's commit — the documents.

**Measurement.** No fixture under `goldens/bench/` names a builtin in its
loop, and all of them are level against `909bf40` within layout noise
(`bench_block.ns` and `bench_do.ns` read 2–4% faster only because the base's
two `-C metadata` layouts are slow); start-up is level. A throwaway loop that
names `Array` and `OrderedCollection` each iteration is 1.5% slower, about
1.4 ns per class-accessor send against the old constant load, and `vm::run`
grew from 6,507 to 6,626 instructions, still one function
([execution.md](design/execution.md), "What the class accessors cost").

## `True` and `False` in Newspeak (2026-09-24)

`True` and `False` implement their whole protocol as ordinary `Kernel.ns`
methods, one per branch, as SOMns's, Primordial Soup's and Newspeak on
Squeak's kernels do, and no longer reach the VM mirror: the ten Boolean
rows of `MIRROR`, `BooleanPrimitive`, `builtins/boolean.rs` and
`Continuation::Truthy` are gone ([core-library.md](design/core-library.md),
"`True` and `False` are quick methods";
[plan](superpowers/plans/2026-09-24-2320-ns-next-boolean-quick-methods.md)).
This is the Boolean half of "Planned: real bodies for control flow and
`do:`", moved ahead of the rest; `whileTrue:`, `whileFalse:`, `do:` and
`value` stay leaves.

Four decisions, taken with the user on 2026-09-24:

- **The Boolean family moves now**, the loops later, as decision 7 of the
  kernel move still says for them.
- **Quick methods keep the depth count agreeing.** The kernel compiler runs
  a body that is `^true`, `^false`, `^nil`, `^<parameter>` or
  `^<parameter> value` without an activation (`MethodFastPath::Constant`,
  `Parameter`, `ValueOfParameter`), as Squeak's and Cog's quick methods,
  so the reference lowering's `SendInlinable` of `ifTrue:` still counts
  none where the optimized lowering inlines it.
- **`and:`, `or:`, `&` and `|` do not coerce**, as in every prior-art
  kernel: `true and: [3]` answers `3`, under both lowerings.
- **An argument that is no block is sent `value`**, with no `Object>>value`
  to answer it, as in Primordial Soup: `true ifTrue: 3` ends in
  `doesNotUnderstand:`, `false ifTrue: 3` answers `nil`.

**Goldens.** Two changed, both because of the coercion decision:
`language/and_or_short_circuit` (was `and_or_short_circuit_and_coerce`),
whose `true and: [3]` answers `3` where it answered `false`, and
`language/and_or_answer_their_arguments_value` (was
`variable_and_block_coerces_its_value_to_a_boolean`), likewise, and which
now also covers `&` and `|` with a non-block argument. Four are new:
`kernel/recursion_through_and_or_counts_alike_under_both_lowerings`, which
passed before the move too and fails if any `True`/`False` method is not
recognized as quick; `language/a_short_circuited_conditional_ignores_its_argument`;
`errors/a_conditional_sends_value_to_its_argument`; and
`kernel/a_subclass_of_true_inherits_its_methods` — a user subclass of
`True` now runs its branches, where the leaves used to decline for want of
a payload.

**Measured** against `ddd9e4f` ([execution.md](design/execution.md), "What
`True` and `False` as quick methods cost"): a throwaway source sending
Boolean methods in its loop is 21.5% faster; the four fixtures are level.
`vm::run` grew from 6,626 to 6,818 instructions and is still one function.

## `UndefinedObject` and the nil protocol (2026-09-25)

`nil`'s class is `UndefinedObject`, as §5.1.3 names it; it was `Nil`
(`BuiltinClass::UndefinedObject`, `Object`'s accessor renamed with it; the
image tag stays 11). `Object` declares `isNil`, `notNil`, `ifNil:`,
`ifNotNil:`, `ifNil:ifNotNil:` and `ifNotNil:ifNil:`, and `UndefinedObject`
the same six the other way round, as SOMns's and Newspeak's JS kernel do
([core-library.md](design/core-library.md), "The nil protocol").

- **A not-nil block is culled with the receiver** — handed it when it takes
  an argument, nothing when it takes none — because the Newspeak sources
  under `corpus/` pass both shapes (386 against 126) and the prior art
  splits. `Block>>cull:` is an ordinary kernel method stating the rule.
- **Every one of the twelve is a quick method.** Two shapes joined the
  quick methods for them, `^self` (`MethodFastPath::Receiver`), which also
  made `Integer>>hash` quick, and `^<parameter> cull: self`
  (`MethodFastPath::CullParameter`). The compiler inlines none of these
  sends, so no block they activate is exempt from the depth count, and
  both lowerings count alike. A culled block with an argument is handed a
  one-element `Vec` per send, the one allocation on these paths.

`nil printString` answers `'nil'`, as Primordial Soup's does, instead of
`Object`'s generic `an Object`.

**Goldens.** `spec/spec_5_1_3_nil_answers_is_nil` is no longer skipped. New:
`kernel/nil_is_the_undefined_object`, `language/the_nil_protocol`,
`errors/if_not_nil_culls_its_argument` and
`errors/if_not_nil_with_a_two_argument_block_is_a_wrong_arity_error`. One
changed, because of the new `printString`: `cache/cache_polymorphic_sites`
prints `nil` for `nil` where it printed `an Object`.

## Double-quoted strings replace characters (2026-09-25)

`"..."` is a string literal, beside `'...'` (§5.1.5), and character literals
and the `Character` class are gone, as §5.1.4 announces (decided with the
user on 2026-09-25; [frontend.md](design/frontend.md), "Lexer"). A
double-quoted string has no escape and ends at the next `"`, so each kind of
string holds the other's quote freely; any single character between two
quotes is a one-character string, `"` included, so the corpus's `"""` still
reads as a quote and `parse-corpus` stays at 210/0/1/0. `README.md`'s
deviations table records the difference from §5.1.4's `""""`.

The `Character` class, its three mirror rows (65 remain), `Object`'s
accessor for it and `Mutator::new_character` are gone. Its literal tag 6
(`RETIRED_LITERALS`) and builtin class tag 8 are retired and never reused.
`String` gained `asString`, a `^self` quick method.

**Goldens.** Three changed, each by losing its character lines:
`kernel/a_value_has_no_identity`, `kernel/value_hashes_follow_value_equality`
and `kernel/a_subclass_of_a_builtin_falls_to_objects_methods`. New:
`language/double_quoted_string_literals`, `errors/character_is_not_a_name`
and `kernel/a_string_s_as_string_is_itself`.

## The reserved words are refused (2026-09-25)

§4.1's six reserved words — `self`, `super`, `outer`, `true`, `false`,
`nil` — can no longer name a parameter, block parameter, slot, temporary,
class or method, be assigned with a setter send, or be sent as a unary
selector; the parser reports each with what it may not be
([frontend.md](design/frontend.md), "The six reserved words"). The lexer
still reads them as identifiers, as the canonical grammar does. The
deviation row in `README.md` is gone. Before, such programs ran, and
`foo: nil = ( ^nil )` answered the literal whatever it was sent.

`parse-corpus` stays at 210/0/1/0. `goldens/capture/nested_blocks_carry_variables`
named a temporary `outer` and one image test a method `outer`; both were
renamed, with no recorded outcome moving.

## The platform's class accessors are gone (2026-09-25)

`Platform` no longer declares `Object`, `Array`, `OrderedCollection`,
`String` and `Symbol`: since the predefined classes became accessors on
`Object` (§3.2), `platform Array` answers the same class through the
inherited one. Their five mirror rows are gone (60 remain), and the
platform keeps `transcript`, `collections`, `List`, `show:` and `newLine`
([core-library.md](design/core-library.md), "The on-demand rule").
`interpreter::kernel`'s `the_platform_declares_no_class_accessor_of_its_own`
pins the absence, and `tests/platform.rs` that all five still answer.
Nothing recorded changed.

## The platform, its transcript and its namespace are values (2026-09-25)

`Platform`, `Transcript` and `Collections` are three `Kernel.ns` classes
below `Value` (decided with the user on 2026-09-25;
[core-library.md](design/core-library.md), "The platform"). The platform
holds the other two in immutable slots, `Collections>>List` is
`^outer Kernel OrderedCollection` answered without a leaf, and only
`Transcript`'s `show:` and `newLine` reach Rust (57 mirror rows remain).
Until then the three shared the class `Platform` and were told apart by how
many empty fields they had, every leaf checking that it had the right one;
that tagging is gone, and each class declares only its own protocol.

The host builds the one platform in the state `Platform new` would give it
(`Mutator::platform_value`), so asking for it runs no Newspeak and fills no
cache; `Platform` alone of the builtins may declare slots. `Transcript` and
`Collections` took builtin class tags 18 and 19.

**Goldens.** `errors/native_that_fails_on_its_receiver_falls_to_objects`
changed one line: `p transcript = p transcript` is now `true`, two
transcripts being equal values. New:
`kernel/the_platform_and_its_parts_are_values`. `tests/inline_caches.rs`'s
`a_failing_primitive_keeps_its_entry` relied on the shared class and now
shows the same thing through `Value>>=`, whose leaf declines on a receiver of
its own class when a slot's class brings its own `=`.

## `Behavior`, `Class` and `Metaclass` (2026-09-25)

§3.2's reified class hierarchy, step 6's third plan
([ns-next-class-behavior-metaclass](superpowers/plans/2026-09-25-1331-ns-next-class-behavior-metaclass.md);
[object-model.md](design/object-model.md), "Class sides and metaclasses").
`Kernel.ns` declares `Behavior` below `Object`, and `Class` and `Metaclass`
below `Behavior` (builtin class tags 20, 21 and 22). Every class object's
class is its own metaclass — its class side, with its primary factory,
applied to `Class` — built eagerly with the class object
(`Mutator::attach_metaclass`), and every metaclass's class is `Metaclass`,
whose own metaclass is again an instance of `Metaclass`. A class object's
dispatch id is its metaclass's instances id: the class-side id, the
class-side chains and the class-side resolution are gone, a factory is
honoured only on the receiver's own metaclass (`resolve::resolved_for`), and
a class side's `doesNotUnderstand:` now handles its class object's
unhandled sends, which removes the §5.5 deviation row. `Class>>name`,
`Class>>printString`, `Metaclass>>name` (`Foo class`) and
`Metaclass>>printString` answer through two new leaves
(`builtins/behavior.rs`), nothing below `Behavior` can be instantiated
(`InstantiationRefusal::ClassObject`), and a class object is a value exactly
when its enclosing object is, with `=`, `==` and `hash` derived from its
state — which removes the §3.2 deviation row and the §3.1.1 row's "a module
definition is not one". The image format carries a class record's metaclass
as a record of its own (`OBJECT_METACLASS`) and a prebuilt class's by tag
(`CLASS_METACLASS`, `HASHED_METACLASS`), holds each class record and its
metaclass record to each other (`MetaclassPairing`), and retires
`ClassOfAClassIsARecord`; the decode amplification does not move.

**The nine decisions** (the user, 2026-09-25; the plan's "Decisions this
plan implements"): a class object is a value exactly when its enclosing
object is (SOMns's `SClass.isValue`), a metaclass when its class is; the
state stays in `ClassData`, the kernel classes carry behaviour only;
metaclasses are eager, `ClassData.metaclass` is gone and `this_class` links
a metaclass back to its class; the hierarchy is Primordial Soup's, `Object`
→ `Behavior` → {`Class`, `Metaclass`}, none of them below `Value`; the
protocol is Primordial Soup's minimal one, `name` and `printString`; `Object`
declares no accessor for the three, and instantiating anything below
`Behavior` is refused; a value class object's equality is derived from its
application, enclosing object and superclass — a metaclass's from its class
— by extending `builtins::value`'s walk with class objects as a node kind;
its hash is derived from its mixin's name alone; and in the image a class
record's metaclass is a record of its own, a prebuilt class's is named by
tag, and the amplification bound does not move. Two more were settled while
building it: `Behavior>>==` and `Metaclass>>==` are the same forwarder as
`=`, to the same leaf, so `self class == other class` costs no activation;
and the hash of a value class object is taken from its mixin's name, which
survives a copy and a round trip where a dispatch id does not.

**Goldens.** Two recorded outcomes changed, each justified line by line in
its commit. `class-side/class_side_super_reaches_object` became
`class-side/class_side_super_reaches_class`: a class-side `super printString`
reaches `Class>>printString` and answers the class's name, `Simple`, instead
of `Object`'s `an Object`, since a metaclass's superclass is now `Class`.
`cache/cache_class_objects_and_instances_at_one_site` changed three lines:
the class objects `Plain`, `Maker` and `Plain` again declare no class-side
`printString` and now answer `Class`'s, their names, instead of `Object`'s
default. New: `class-side/the_class_of_a_class_is_its_metaclass`,
`class-side/a_class_object_cannot_be_made_by_instantiation`,
`class-side/a_metaclass_does_not_understand_new`,
`class-side/a_class_and_its_metaclass_print_their_names`,
`class-side/a_class_side_does_not_understand_handles_its_class_objects_sends`,
`kernel/a_value_modules_nested_class_is_a_value` and
`kernel/a_values_slot_refuses_a_class_that_is_no_value`.

**Measured** against `3defb78`, the `master` it started from
([execution.md](design/execution.md), "What `Behavior`, `Class` and
`Metaclass` cost"), three layouts of each tree, four sittings pooled, at
`1c1f624`: **the fixtures are level.** `bench_send.ns` is within 0.5% (8.59
→ 8.64 ms), `bench_loop.ns` 1.4% faster (7.35 → 7.24) and
`bench_megamorphic.ns` 2.1% faster (11.70 → 11.45), each in every sitting;
`bench_block.ns` is the known bimodal source, and fast layout against fast
layout it is within 0.4% (16.00 → 16.07). The throwaway `bench_metaclass.ns`,
100,000 × `Outer new Inner make`, is **14.5% slower** (38.68 → 44.29 ms),
about 56 ns an iteration for the eager metaclass of each nested class object.
Before `1c1f624` the fixtures were not level: `bench_send.ns` 6.0% slower,
`bench_loop.ns` and `bench_megamorphic.ns` 3.4% of the pooled medians before
rounding, and `bench_metaclass.ns` 17.3% slower (against the 14.5% above),
the step of the two send sources all on `baf6f65`. **Its cause** was code
generation: with `chain_start`'s class-side arm gone, LLVM's SROA split the
`SendKey`'s copy of the receiver into five pieces and stored them back one
by one wherever a whole `Value` was needed, so the next 16-byte load of
it -- building `Step::Method` on a hit, the key a megamorphic send hands
`miss` -- failed
store-to-load forwarding (a sampling profile put 3,000 of 75,000 samples of
`bench_send.ns` on that one load). `1c1f624` holds the key's receiver by
reference to the send's own local, which keeps it one object in memory.
`vm::run` is still one function, with no `vm::run::closure`, and shrank from
7,055 instructions at the base to 6,741. The same profile shows the same
kind of stall on every tree costing far more -- a quarter of `bench_send.ns`'s
samples on `Value`s a callee writes through its out-parameter in pieces that
do not line up with the load that reads them; the eight-byte value is its
planned answer ([OPEN-WORK.md](OPEN-WORK.md)).

## Exceptions (2026-09-25)

§8.6's exceptions as a library
([ns-next-exceptions](superpowers/plans/2026-09-25-2244-ns-next-exceptions.md);
[core-library.md](design/core-library.md), "Exceptions"; built 2026-09-25 to
2026-09-26). `Kernel.ns` declares `Exception`, `Error`,
`MessageNotUnderstood`, `Halt`, `Message`, the private `Activation` and
`KernelNamespace` (builtin class tags 23 to 29), and the platform gains a
`kernel` slot through which programs name the first five. `signal`, the
handler search, `return:`, `retry`, `resume:`, `pass` and `defaultAction`
are Newspeak over ten activation leaves (`builtins/activation.rs`);
`on:do:`, `ensure:` and `ifCurtailed:` are `Block` methods whose compiled
code carries an unwind marker (`bytecode::UnwindMarker`,
`compiler::unwind_marker`). Activations are objects of their own kind,
`ObjectData::Activation(FrameReference)`, written to an image as
`OBJECT_ACTIVATION` (body tag 9). One unwinder, `interpreter::unwind`,
carries every non-local return, `return:`, `resume:`, `retry` and
termination, running `ensure:` and `ifCurtailed:` blocks as trampoline
steps, and aims at a `FrameReference` (`Signal::Return` no longer carries an
`ActivationReference`; `Heap::activation_is_live` is gone). Every VM error
becomes a signalled `Error` pushed above the activation it arose in, and
`Object>>doesNotUnderstand:` signals a `MessageNotUnderstood` with a real
`Message` and re-sends it with the public `sentTo:` when the default
handler resumed it; the mirror row that answered it in Rust retired, which
removes the §3.4 deviation row. An uncaught exception ends the run with the
`HostError` and the line the host received before, once the pending
`ensure:` blocks have run. The depth limit keeps a reserve of 1,000
(`DEPTH_RESERVE`) for signalling an error raised near it.

**The nine decisions** (the user, 2026-09-25; the plan's "Decisions this
plan implements"): Newspeak over activation leaves, as Primordial Soup, not
native as SOMns; the scope — the five classes and `Activation`, ANSI's
protocol, and `ExceptionSet`, `Notification`, `outer`, `retryUsing:`,
`signalerContext`, `Object>>error:` and the finer `Error` subclasses left
out; `doesNotUnderstand:` is Squeak's; every VM error is a signalled `Error`
with today's text; the reserve of 1,000; one unwinder in `interpreter/`,
aimed at a `FrameReference`; `Activation` reachable only through mirror
leaves; `halt` terminates in the CLI; and `platform kernel`. **Settled while
building it:** class sides are not inherited (§3.2's flat metaclasses), so
the default `handles:` is on `Class` and `signal` and `signal:` are declared
on each kernel exception class's class side — a program's own subclass is
signalled as `MyError new signal: 'text'` unless its class side declares
them; the handler environment is ANSI's and Squeak's rather than Primordial
Soup's one flag — every `on:do:` the search passed is disabled while a
handler runs and enabled again under an `ensure:`, at the cost of a second
walk over the passed range and a collection of the handlers per handler
invocation; an `Activation` record's header is the one place an image may
name tag 23; a fresh `Activation` object per leaf call, compared only
within one call's answers; `Exception`'s `hostSelector` and `hostReceiver`
key on the recorded origin, so a VM error whose receiver is nil keeps it,
and `MessageNotUnderstood`'s fall back to them when its `message` is nil;
the `terminate…` leaf is total, rendering a description that is no String;
and the reserve is taken by any VM error and by the kernel's
`doesNotUnderstand:` near the limit, never by a program's own `signal`, is
held by the frame of the activation the error arose in rather than by a
stack index, is released when a host's run ends, and an overrun ends the
run uncatchably — with the error still being searched for, the outermost
one, when the signalling machinery overran, and with "stack overflow:
recursion too deep" when a handler did. The stack's bound is 129 × (L +
`DEPTH_RESERVE`) entries.

**Goldens.** No recorded outcome changed. Twenty-nine new goldens in
`goldens/exceptions/`. Two golden sources that read the old two-element
`doesNotUnderstand:` argument with `at:` —
`cache/does_not_understand_site_with_three_kinds_of_handler` and
`class-side/a_class_side_does_not_understand_handles_its_class_objects_sends`
— now send `selector` and `arguments`, with their `.toml`s untouched. New
Rust tests: `crates/ns-runtime/tests/exceptions.rs` (the re-send after a
resumed `MessageNotUnderstood`, a resumed VM error still ending the run,
uncaught errors and `doesNotUnderstand:`s at L − 3, L − 1, L and L + 1, and
overruns of the reserve by the search and by a `pass`), and
`interpreter::unwind`'s, `tests/image.rs`'s and `interpreter::kernel`'s
tests for the unwind target, the reserve, the activation record and the
markers.

**Measured** against `3cbfd31`, the `master` it started from
([execution.md](design/execution.md), "What exceptions cost"), three
layouts of each tree, four sittings pooled. At `3d5f69c` the branch was
slower on paths that use no exception -- `bench_block.ns` 5.0% slow layout
against slow layout, a throwaway `bench_value.ns` (three block `value`s an
iteration) 4.6%, `bench_deep.ns` (twenty sends deep and back) 6.4% -- and
bisecting located two causes, each fixed: `Signal` had grown from 48 to 56
bytes, and every `Result<Step, Signal>` a send decides from 56 to 64, once
`Terminate` held a second `NewspeakError` in place (boxed, `f518988`); and
compaction shrank an arena to twice its live set while the next collection
waits for `max(floor, 2 x live)`, so under the floor every collection cut
the object arena to 144 slots and the next cycle grew it back through five
reallocations -- older than the branch, exposed when the exception classes
moved the live set from 57 objects to 72 (`c5b1f2b`, which now shrinks
against the limit). Profiling what was left found a third,
`decide_slot_accessor` formatting its error in place and so spilling every
slot read through a stack frame (`1d7587f`, the error out of line). **At
`1d7587f` every source is faster than the base in every sitting**:
`bench_loop.ns` 24.3% (7.27 → 5.50 ms), `bench_send.ns` 18.6% (8.70 → 7.08),
`bench_block.ns` 18.7% against the three base layouts' mean (18.21 → 14.80;
no head layout draws the slow mode), `bench_megamorphic.ns` 12.0%
(11.61 → 10.22), `bench_value.ns` 13.1% (43.82 → 38.09), `bench_deep.ns`
2.5% (16.27 → 15.86). Only the `Signal` boxing repairs a cost the branch introduced; the arena shrink and the slot accessor repair defects `master` has too, so being faster than the base is not a property of the exceptions branch, and its net cost against a `master` carrying the same two fixes was not measured -- the per-commit attribution suggests about zero. With nothing signalled, measured at `3d5f69c`,
`bench_ensure.ns` (one `ensure:` and one `on:do:` an iteration) costs
about 145 ns an iteration over the same blocks sent `value`
(60.47 against 46.01 ms); `bench_signal.ns`, an `Error` signalled twenty
activations down and caught, about 6.4 µs a signal (81.38 against 17.29
ms for the same recursion answering). `vm::run` is still one function,
with no `vm::run::closure`: 6,741 instructions on the base, 6,486 at
`d7a6680`; `push` is inlined into `push_step` and `push_initializer` as
before, with `push_past_the_limit` out of line.

## Editor support: the language server and the JetBrains plugin (2026-09-26)

**Built:** `crates/ns-lsp/` (the `ns-next-lsp` binary)
([editor-support-language-server](superpowers/plans/2026-09-26-0030-editor-support-language-server.md))
and `editors/jetbrains/` (the JetBrains plugin), across seven tasks
([editor-support-jetbrains-plugin](superpowers/plans/2026-09-26-0031-editor-support-jetbrains-plugin.md));
[editor-support.md](design/editor-support.md) is the living design document,
now marked **Built**.

`ns_runtime::resolve_for_editor` answers a file's bindings and errors together,
so one resolver pass serves both diagnostics and semantic tokens even when the
module has an error; it never records `Binding::MirrorSend`, so the VM
mirror's capability invariant (`CLAUDE.md`, "The VM mirror is a capability")
is untouched. `ns-next-lsp` speaks `initialize`, `didOpen`, `didChange`,
`didClose`, `publishDiagnostics`, `semanticTokens/full`, `shutdown` and `exit`
over `lsp-server`, synchronously on one thread, and runs every analysis under
`catch_unwind` so a panic becomes one diagnostic rather than taking the
server down. Its `dump-tokens` subcommand prints a file's tokens as JSON, in
UTF-16 offsets, for the plugin's lexer comparison.

`editors/jetbrains/` is a Gradle project (Kotlin, the IntelliJ Platform
Gradle Plugin, the wrapper committed) building a plugin whose Kotlin lexer
mirrors `crates/ns-syntax/src/lexer.rs` function by function — reviewed side
by side rather than generated, so `LexerComparisonTest` can hold the two to
the same tokens, kind and span, over `corpus/modules`, `goldens/` and
`Kernel.ns` (`ns-next-lsp dump-tokens`, batched at 100 files for Windows'
command-line length limit, over the repository's close to 400 `.ns` files).
Highlighting and the editor features (brace matching, commenting, quote
handling, TODO markers via `NewspeakIndexPatternBuilder` — no dedicated
`todoIndexer` was needed) run off that lexer alone, with no server involved;
semantic tokens from `ns-next-lsp`, reached through the platform's native LSP
API (`LspIntegrationProvider`, `ProjectWideLspClientDescriptor`,
`LspCustomization.semanticTokensCustomizer`), are layered over them once the
server answers. Spell checking is an optional dependency
(`<depends optional="true" config-file="newspeak-spellchecker.xml">
com.intellij.modules.spellchecker</depends>`), so an IDE built without that
module still loads the plugin.

The plugin bundles `ns-next-lsp`, built `--release` from the same commit as
the Kotlin lexer, for Windows x64; `THIRD-PARTY-NOTICES.txt` covers the
crates linked into that binary (`cargo tree -p ns-lsp -e normal,no-proc-macro`),
excluding build-time-only proc macros. `verifyPlugin` checked the plugin
against the lowest supported and the current release of four commercial
IDEs — IntelliJ IDEA 2026.1.4 and 2026.2.3, RustRover 2026.1.4 and 2026.2.3,
PyCharm 2026.1.5, CLion 2026.2.3 — all `Compatible`, with no deprecated or
internal API usage, targeting the LSP API JetBrains reworked in 2026.1.4
alone. Every sandbox task (tests, `runIde`) disables the bundled
`com.intellij.modules.ultimate` marker plugin, whose startup activity cannot
run headless; the shipped plugin never references it.

The first `verifyPlugin` run found one real defect: `NewspeakLspIntegrationProvider`
read its own installation directory with
`PluginManagerCore.getPlugin(PluginId.getId(PLUGIN_ID))?.pluginPath`, and
every overload of that lookup is `@ApiStatus.Internal` in this platform
version, which fails `verifyPlugin` outright rather than merely warning. The
fix implements `com.intellij.openapi.extensions.PluginAware`, the platform's
own way to hand an extension its plugin descriptor: `XmlExtensionAdapter`
calls `setPluginDescriptor` right after constructing an instance that
implements it, with no internal call needed. `verifyPluginProjectConfiguration`
(not part of `check` on this platform, run on its own) reports two findings
that both stand, as `editor-support.md` records: `sinceBuild`'s major version
sits below the target platform's on purpose (2026.1.4 is the deliberate
floor), and Java bytecode stays at 21 rather than the 2026.2.x platform's
JBR 25 for the same reason — 2026.1.x still bundles JBR 21, which cannot run
Java 25 bytecode, and `verifyPlugin`'s own six `Compatible` results are the
evidence that Java 21 bytecode runs fine on the newer JBR too.

`CLAUDE.md`, `docs/design/README.md` and this file now name
`editors/jetbrains/` and its gates: `./gradlew -p editors/jetbrains check`
before every commit that touches it, `crates/ns-lsp/` or
`crates/ns-syntax/`, and `./gradlew -p editors/jetbrains verifyPlugin
buildPlugin` once at the end of a batch.

Tried by hand under `runIde` afterwards, which found one more defect: `runIde`
starts from its own sandbox (`prepareSandbox_runIde`, `plugins_runIde/`), and
only `prepareSandbox` had copied the server into a sandbox, so the plugin under
`runIde` found none. Every sandbox an IDE starts from now gets it. With that,
the server starts, the semantic colours land and a resolver error is
underlined. The settings page's server path gained a file picker.

The hand check also surfaced a runtime deviation from the specification, not
an editor defect: the resolver recognizes `outer N m` only when `N` starts
with an uppercase letter (`resolver/sends.rs`, `outer_class_name`), where
§5.9's outer send takes any identifier and makes a non-enclosing one a
compile-time error. `^outer missing foo` therefore resolves as the chain
`(outer missing) foo` and fails only at run time. The resolver was fixed
the next day ("The unreserved words are names").

## The platform module (2026-09-26 to 2026-09-27)

Plan: [ns-next-platform-module](superpowers/plans/2026-09-26-1950-ns-next-platform-module.md);
[core-library.md](design/core-library.md), "The platform".

§6.3.3's lazy slots: a lazy getter is a compiled method that opens with
`ReturnSlotIfFilled` (instruction tag 34), and a value's `=` and `hash` skip
lazy slots. The table of shipped modules (`interpreter/core_library.rs`'s
`SHIPPED_MODULES`) boots a shipped `PlatformFactory.ns` beside `Kernel.ns`.
Its root holds the mirror and the kernel and is never handed out; the host
instantiates it through the ordinary `Interpreter::instantiate`, checks its
`vmMirror`, and hands every program what the factory's one public member,
`newPlatform`, answers: an instance of the private nested `Platform`, whose
factory is `new` and whose class accessors (`public Transcript = ( ^outer
PlatformFactory Transcript )`, lexical sends that reach the root's private
classes) hand out the module's classes -- the factory pattern, plain
Newspeak rather than a runtime refusal (decided with the user, 2026-09-27).
Builtin class tags 13, 18, 19 and 29 (`Platform`, `Transcript`,
`Collections`, `KernelNamespace`) are retired.

An image names the platform and everything shipped it reaches by platform
path (`VALUE_PLATFORM_PATH`, `CLASS_PLATFORM_PATH`, `HASHED_PLATFORM_PATH`),
each step a public slot getter, a nested-class accessor or a pure class
accessor, recognized by the compiler in shipped source
(`MixinMethod::class_accessor`, read only by the image code, no fast path),
resolved before the rest of a graph decodes; a second platform is an
ordinary record of the class the path `Platform` names. The boot runs
`newPlatform` and `Platform`'s superclass clause, so a freshly bootstrapped
interpreter's caches are not empty (four sites, three global entries, pinned
by a test).

New goldens pin the lazy-slot rules (`goldens/spec/spec_6_3_3_*`,
`goldens/kernel/a_values_lazy_slot_*`) and
`goldens/kernel/a_second_platform_writes_like_the_first`;
`goldens/kernel/the_platform_and_its_parts_are_values` checks
`platform = platform class new`, and
`spec_6_3_3_a_lazy_slot_is_initialized_on_first_use` runs, unskipped.

**Measured** ([execution.md](design/execution.md), "What the platform module
cost"): start-up is unchanged, +0.2%, inside the protocol's own spread. The
lazy getter's first mechanism, a `MethodFastPath::LazySlot` arm in
`act_with_base`, inlined into every send site of `vm::run`, cost
`bench_send.ns` 4.6% and `bench_block.ns` 2.6%; moving the arm out of line,
and then taking the slot base off the activation, did not recover it, and
dropping the fast path did: a getter answers a filled slot from its own
activation, and the activation derives its slot base from its
`current_class`. All four `goldens/bench/` fixtures are level with `7c6bdbf`
(pooled over three builds a side and two sittings, -0.7% to +0.4%, every
comparison overlapping); `vm::run` is 6,691 instructions against 6,561, the
difference an arm no ordinary send runs.

## The transcript's `show:` and `print:` (2026-09-27)

Decided with the user ([core-library.md](design/core-library.md), "The
platform"): `show:` writes a string and refuses anything else as an ordinary
error, where `show: 42` wrote nothing; `print:` writes any object's
`printString`, in Newspeak, as canonical Newspeak's transcript does.
`goldens/kernel/the_transcript_prints_any_object` and
`goldens/errors/show_of_something_that_is_no_string_is_an_error` pin both.

## The test harness (2026-09-27)

Decided with the user (`CLAUDE.md`, "Working in this repository";
[README.md](design/README.md), "The goldens" and "Gates";
[compiler.md](design/compiler.md), "The harness"): the suites run under
`cargo-nextest`, with `cargo test --doc` beside it for the doctests; every
golden is a test of its own, and so is each of its build/load and
snapshot/load round trips (`crates/ns-cli/tests/run_goldens.rs`,
`golden_round_trips.rs`, on `libtest-mimic`); a test whose two lowerings are
independent is written once with `ns_runtime::for_each_lowering!` and runs
as one test per lowering; debug builds compile at `opt-level = 1`; and the
end-of-batch gate runs the three stress modes at once. The normal suite went
from about 47 s to 16 s, the three stress modes from about 48 minutes to
under 4.

## The unreserved words are names (2026-09-27)

§4.1 reserves six words, and the syntax now follows the specification
strictly for the rest ([frontend.md](design/frontend.md), "Words that are
not reserved"): `class`, `public`, `protected`, `private` and `lazy` are
names wherever the grammar takes an identifier, and where one could also
begin a declaration the grammar decides as the PEG it is -- an access
modifier standing first is the modifier, and a lazy slot is one only if
§6.3.3's whole production matches, so `private lazy = ( ... )` is a method
named `lazy`. One deviation was decided with the user: the modifiers need
no whitespace after them. A slot's or a lazy slot's initializer now needs
its mandatory dot. The resolver's `outer N` no longer requires an uppercase
`N` (the deviation the editor entry above found), so `outer missing foo` is
the compile-time error §5.9 makes it. The language server colours `lazy` as
a keyword and a send named like one of these words as a send.

`parse-corpus` stays at 210/0/1/0 and `run-corpus` at 6/0/205. Six new
goldens under `goldens/spec/` run every accepted form, and
`unreserved_word_tests` in `parser::declaration` pins the refused ones.

## The syntax is the specification's grammar (2026-09-27)

Every production of `newspeak-spec.pdf` §4.2 to §8.1 was checked against the
lexer and the parser, and the parser now accepts exactly what the grammar
derives, read as a PEG over tokens ([frontend.md](design/frontend.md),
"Grammar"). Where the specification contradicts itself its prose decides --
setter sends chain, a simultaneous slot clause is tried before an empty
sequential one -- and otherwise the canonical grammar.

What changed, each decided with the user where it was a choice:

- A `-` before a digit is a sign only where a literal stands (`3 -4`
  subtracts), a `-` continues no binary selector, a radix or fraction is
  taken only when complete, a symbol's keywords each end in their colon, and
  `<-:` is one token. `NewspeakColorizationForSqueak.ns`, blacklisted for
  `sd start -1`, parses: `parse-corpus` is at 211/0/0/0 and the blacklist is
  empty.
- `#( ... )` is gone -- neither the specification nor the canonical grammar
  has it -- with its AST, bytecode and VM paths, and image literal tag 7 is
  retired; six goldens now write tuples, their outputs unchanged.
- Refused now: a setter keyword after a receiver or in a message pattern, a
  cascade after an implicit-receiver keyword send or a primary alone, a dot
  that ends no statement or stands between members, a second message in a
  superclass clause, a class without its instance side, an access modifier on
  a module's class, a bare `super` (its two goldens became parser tests) and
  an `outer` with no class name after it, a name position holding no name.
- Accepted now: a module without a category, `class Map <[K, V]> ...`, a
  binary factory named `=`, `||` as an empty section and simultaneous
  temporaries in a code body, every binary selector in a cascade and in a
  method pattern, pattern literals by their grammar.
- `outer N` with no message answers the enclosing object, as in canonical
  Newspeak, through `Binding::EnclosingObject` and the new instruction
  `LoadEnclosingObject`, tag 35.

A pattern literal still evaluates to `nil`, now a row in the deviations
table; chains and object literals, which the specification marks
unimplemented, are recorded as possible future extensions.

## The compile-time errors the specification names (2026-09-28)

§3.6's one compile-time error the resolver did not report is reported now:
an activation -- a method, a block or a factory -- declares each name once
among its parameters and locals (`Resolver::check_activation_names`), and
`goldens/spec/spec_3_6_a_parameter_and_a_local_of_the_same_name_is_an_error`,
skipped since it was written, holds. §5.9's other one, `outer N` naming no
enclosing class, was already a resolver error.

A radix number is checked while it parses: a radix outside 2 to 36 or a digit
at or above it is a parse error, as in the canonical parser, not an error the
program raised when it ran. A radix number with a fraction or an exponent is a
float with the canonical value (`16r1.8` is 1.5, `2r1e3` is 1000); `16r1Fe2`
had read `e2` as hex digits and answered 8162 at exit status 0. A superclass
clause's receiver and `outer`'s class must be names, and §6's
`typeParameterDecl` is checked rather than skipped.

## `main:args:` receives the program's arguments (2026-09-28)

§8.4's second argument to `main:args:` is "a list of any
environment-specific (e.g., command line) arguments"; the CLI passed `nil`.
`run`, `load`, `snapshot` and `bench` now take the program's arguments after
`--` and pass them as an `Array` of `String`s, empty when there are none
(`entry::entry_arguments`), and the deviations-table row is gone. A golden's
`.toml` may list `arguments`, which the runners pass to `run` and to `load`
in both round trips
(`goldens/spec/spec_8_4_main_args_receives_the_program_arguments`,
`spec_8_4_main_args_without_arguments_is_an_empty_list`).

## The collections module (2026-09-28)

Plan: [ns-next-collections-module](superpowers/plans/2026-09-28-0102-ns-next-collections-module.md);
[core-library.md](design/core-library.md), "The collections module (plan 2)".

In the factory pattern, decided with the user: a shipped
`CollectionsFactory.ns`, whose root holds the kernel and is never handed
out, and whose private nested `Collections` is the facet `platform
collections` answers. The facet names its own class (`public Collections = (
^outer CollectionsFactory Collections )`) as `Platform` does, so its image
path is `collections Collections` without widening the compiler's
class-accessor recognition. `List` is a private class of the factory,
Newspeak over arrays it replaces when full, handed out by the facet's class
accessor. The mirror's `moduleNamed:` leaf builds a shipped module's root
class on every request and keeps nothing in Rust; `PlatformFactory`'s lazy
`collectionsModule` keeps the one instance, so every platform shares one
`List` class and the module is compiled on the first `platform collections`.
`Array new:` answers a sized array of nils (a class-side mirror forwarder),
and the exception handler search collects its handlers into an `Array` in
two walks. Every program uses `List`; `OrderedCollection`, its `add:` leaf
and `Heap::array_push` are removed, builtin class tag 10 retired, so no
payload grows in place any more. Three semantics were confirmed with the
user: `moduleNamed:` keeps nothing, `List>>at:put:` answers the value, and
`List>>do:` reads the size once. Image tests cover a `List` record, a second
facet and the refusal of the factory. The one golden whose output changed is
`goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods`, seven
lines to six: there is no builtin `OrderedCollection` left to subclass.

**Measured** ([execution.md](design/execution.md), "What the collections
module cost"), three builds a side against `6d9974b`: all four
`goldens/bench/` fixtures are level (-0.2% to +0.5%, every comparison
overlapping); a start that never sends `platform collections` is level too
(+0.03 ms, +0.2%), since each shipped module is resolved on its first
request, not at boot, and the first `platform collections` costs about
0.28 ms (+2.1%): resolving, compiling and instantiating the module.

## Simultaneous slots (2026-09-28)

Plan: [ns-next-simultaneous-slots](superpowers/plans/2026-09-28-1152-ns-next-simultaneous-slots.md);
[object-model.md](design/object-model.md), "Simultaneous slots".

Through pull futures, decided with the user: a `|| ... ||` clause, in a
class header or as a code body's temporaries, initializes its slots
simultaneously, so well-founded mutual recursion between them resolves. Four
rules: once the clause is done no slot of it holds a future (its end forces
each and writes the value back); every future it made is resolved then,
escaped ones included; a resolved future is observable by no program --
messages are forwarded, identity, equality and every leaf see through it,
and the collector replaces it with its value in object fields; and a value
never holds one. The mechanism: the parser keeps the `simultaneous` flag;
one desugaring before both lowerings (`compiler/simultaneous.rs`) emits
`MakeFuture`, `InitializeSlotProvisionally` or a store, then `LoadOwnSlot`
or a read, `ResolveFuture` and a checked write at the clause's end
(instruction tags 36 to 39); `Kernel.ns`'s private `Future` (builtin class
tag 30), after Primordial Soup's `Proxy`, forwards through
`doesNotUnderstand:`; forcing is a trampoline step under
`Continuation::Future`, and an instruction or a leaf that meets an
unresolved future forces it and runs again (`vm::reexecute`), which rests on
a leaf that declines or raises having changed nothing, and a leaf's callers
look for a future only on its failure path; ill-founded recursion is an
ordinary error; an abandoned clause leaves its futures unresolved, to be
computed on the next touch; an image writes a resolved future as its value
and an unresolved one as a record. Fourteen new goldens under
`goldens/spec/` pin it, and
`spec_6_3_2_simultaneous_slots_resolve_mutual_recursion` answers `3`,
unskipped.

**Measured** ([execution.md](design/execution.md), "What simultaneous slots
cost"), three builds a side against `0b1f474`, two sittings: `bench_loop.ns`,
`bench_send.ns` and `bench_block.ns` level (+0.86%, -0.27%, +0.23%,
medians); `bench_megamorphic.ns` 4.2% slower, separated in both sittings. A
first version, which wrapped every leaf and inlined the four arms, cost all
three sending fixtures 1–2%; the leaf search moved to the callers' failure
paths and the arms out of line recovered two. The rest bisected to the one
calling arm the four instructions add to `vm::run`'s dispatch: a build whose
arm calls nothing is level on all four, and one whose arm calls and discards
the answer is not; aligning every jump target does not help; it reorders
every block of `vm::run` and the `Flow` test after a send with it. The loop
over locals (below) removed the `Flow` and recovered it.

## The loop over locals (2026-09-28)

Plan: [ns-next-loop-over-locals](superpowers/plans/2026-09-28-1808-ns-next-loop-over-locals.md);
[execution.md](design/execution.md), "The loop over locals".

Designed with the user after the 4% `bench_megamorphic.ns` lost to
simultaneous slots was traced to where LLVM placed the test of a send's
`Flow`. `vm::run` enters once per stretch between two safepoints;
`run_stretch`, forced inline, keeps the running frame's activation, code,
program counter, feedback base and a `RegisterBase` (`heap.rs`: a register
access is one index and one bounds check, a debug build checks its
activation's generation) in locals, each arm advances the program counter
itself, and a send, a return, an unwind or a forcing answers a one-byte
`Transfer` with the next frame in a `Handoff`; `Flow` and `vm::poll` are
gone, and the frame is written back to `FrameRegisters` only when a
safepoint is requested. The safepoints are where they were, and no golden
changed.

**Measured** ([execution.md](design/execution.md), "What the loop over
locals moved"), three builds a side against `5fbb2cb`, two sittings: every
fixture faster -- `bench_loop.ns` -29.7%, `bench_send.ns` -23.3%,
`bench_block.ns` -13.6%, `bench_megamorphic.ns` -19.9% (medians), separated
in every sitting but one of `bench_loop.ns`'s, which two outlying rounds
overlap; `vm::run` is one function of 7,143 instructions against 7,083. A
scratch build with one more calling instruction arm, not committed, left
`bench_megamorphic.ns` and `bench_block.ns` level but cost `bench_loop.ns`
and `bench_send.ns` about 3% (+3.0%, +3.3%, means): an arm is not free. A
profile places the loss at a store-forwarding stall: every send's
`Result<Step, Signal>` passes through one stack slot where the inline
cache's hit paths and its `miss` join, the 16-byte `Value` read back from
narrower stores holds 28% of `vm::run`'s samples, and `Step`'s drop glue
runs for every send (4.7%) ([execution.md](design/execution.md), "Where the
3% goes").

A sink that delivers a send's value where it is decided was planned and
tried as an experiment ([execution.md](design/execution.md), "A send's value
delivered where it is decided: tried and halted"): inlined all the way to the
register write it made `bench_loop.ns` 17% faster but doubled `vm::run`, and
it showed that any 16-byte `Value` passing through memory stalls. The user
halted it in favour of the eight-byte value ([OPEN-WORK.md](OPEN-WORK.md)).

## The memory layer (2026-09-29)

Plan: [ns-next-memory-layer](superpowers/plans/2026-09-29-0327-ns-next-memory-layer.md).

Road step 1 ([OPEN-WORK.md](OPEN-WORK.md), "The road to a high-performance
VM"): `crates/ns-memory` reserves one contiguous address range, commits,
decommits and protects page ranges, and lends committed read-write ranges as
word slices, on Windows through `windows-sys` and on POSIX through `libc`
([memory.md](design/memory.md), "The memory layer"). It is the only crate with
`unsafe`, and every `unsafe` block states why it holds; `ns-runtime` keeps
`#![forbid(unsafe_code)]`. A failure the operating system reports, and a
malformed range, is a value, never a panic. Nothing uses the crate yet: road
step 3, the object space, is its first user. The POSIX half is checked with
clippy for Linux and macOS targets, since nothing here runs either.

## Large integers (2026-09-29)

Plan: [ns-next-large-integers](superpowers/plans/2026-09-29-0911-ns-next-large-integers.md);
[core-library.md](design/core-library.md), "Large integers".

Road step 2's first plan ([OPEN-WORK.md](OPEN-WORK.md), "The road to a
high-performance VM"): §5.1.1's integers "are not limited in size". An
integer that fits `i64` is a `SmallInteger`, any other a `LargeInteger`, a
kernel subclass of `Integer`, and `Mutator::new_integer` decides which, so
each integer has one representation; `3 class` stays `Integer`. The
arithmetic in `crates/ns-runtime/src/arbitrary_integer/` is adapted under MIT
from `num-bigint` and `num-integer`, now submodules under `prior-art/` beside
`num-rational`, with the licence texts in `THIRD-PARTY-NOTICES.md` and a
header in every adapted file. The recorded deviation "Integers are `i64`" is
gone, and with it the golden that recorded an overflow error; ten new
goldens pin the new behaviour. Literal tag 8 (`MalformedInteger`) is
retired; value tag 9, literal tag 10 and builtin class tag 31 are new.

- `b02323d` — `//` rounds toward negative infinity, as Primordial Soup's
  `Integer>>//` does, where it had rounded as Euclidean division; `7 // -2`
  answers `-4`.
- `5c25deb` — `ArbitraryInteger`: addition, subtraction, long,
  half-Karatsuba, Karatsuba and Toom-3 multiplication, algorithm D, floor
  division and the radix and `f64` conversions, with
  `THIRD-PARTY-NOTICES.md` and the three submodules.
- `9e7eb94` — multiplication by the number-theoretic transform above 512
  digits and Burnikel-Ziegler division above 64, decided with the user the
  same day.
- `0837ea1` — the `LargeInteger` class and the promotion of an `Integer`
  result that does not fit `i64`, which retired the overflow error.
- `d56c9de` — `LargeInteger`'s own arithmetic and ordering, and every
  integer and float leaf accepting an argument of either integer class.
- `8f7fd91` — every integer literal is representable, as
  `Literal::LargeInteger`, and an image carries a `LargeInteger` inline as a
  value, only in its canonical encoding.
- `fa5f0d3` — the decoder's comment states that decoded `LargeInteger`
  values add to a failed load's peak ([memory.md](design/memory.md), "The
  interim cost, and the one question still open").

**Measured** against `master` (`db380dc`) in one sitting on a release build
with the pinned `RUSTFLAGS`: three builds of each tree from clean target
directories, five alternating rounds of `bench --iterations 30`, so fifteen
medians per tree and benchmark, pooled. No change beyond the builds' own
spread:

| Benchmark | `master` | branch | Change | Spread between builds (`master` / branch) |
|---|---|---|---|---|
| `bench_send.ns` | 4.945 ms | 4.933 ms | -0.23% | 1.88% / 0.93% |
| `bench_loop.ns` | 3.540 ms | 3.538 ms | -0.06% | 1.46% / 1.02% |
| `bench_megamorphic.ns` | 7.571 ms | 7.540 ms | -0.41% | 2.36% / 0.27% |
| `bench_block.ns` | 11.988 ms | 12.145 ms | +1.31% | 1.53% / 3.53% |

`bench_block.ns`'s +1.31% lies inside both trees' spread, and most of it is one
branch build. `dumpbin /disasm` of a release build with debug information shows
`checked_floor_divide` inlined into `vm::run`, and no `vm::run::closure` or
`run_stretch` became a function of its own.

## One artifact, packaged before `main:args:` (2026-09-30)

Plan: [ns-next-packaged-artifact](superpowers/plans/2026-09-29-2341-ns-next-packaged-artifact.md);
[image-format.md](design/image-format.md), "The artifact" and "Packaging".
Commits: `78bb531..cec79bf`, `78bb531` being the plan's, and the commit of this entry.

A program is one `.ns` file, whose root is instantiated with `new`, or a
JSON project file of several modules, whose root is instantiated with
`packageUsing:` a manifest -- `core-library/Manifest.ns`, an unprivileged
class compiled into the program, whose `doesNotUnderstand:` answers each
module by the name its source declares. Either way the platform reaches the
program only as `main:args:`'s first argument (§8.4). `build` assembles the
application exactly as `run` does and writes it just before `main:args:`, as
Newspeak on Squeak and Primordial Soup package theirs; `load` starts it. The
stateless binary, the `snapshot` command and `--send` and `--entry` are
gone, and `bench` and `run-corpus` assemble the way `run` does, the latter
through a generated wrapper application per corpus row. An image carries
several modules: the mixin table is a forest, a module section records each
module's origin, source and root section, and a module's root class and
levels are named by index. New wire numbers: class tags 6 and 7, hash tags 5
and 6, origin tags 0 and 1, header flag 4 (`FLAG_START`); retired: class
tags 1 and 3, hash tag 1, header flag 2 (`FLAG_STATELESS`). Every golden
reaches the platform only through `main:args:` with its `status`, `stdout`
and `stderr` unchanged, and `goldens/manifest/` pins the manifest.

- `b933032`, `3ce757d` — every golden's root is `new`-instantiated and
  reaches the platform only through `main:args:`, with its recorded outcome
  unchanged; a loaded world's initializer re-run shows in a printed
  identity hash.
- `9cf7abd` — an image names an embedded module's origin (`ORIGIN_EMBEDDED`).
- `63096bf`, `facec11` — the code and mixin tables describe a forest of
  modules, and `CodeIds::id_of` answers an error instead of panicking.
- `ab5a68f` — the module section, and a module's root class and levels
  named by index (`CLASS_MODULE`, `CLASS_MODULE_LEVEL`,
  `HASHED_MODULE_CLASS`, `HASHED_MODULE_LEVEL`), which gave a module's
  `<:` levels the identity-hash entry a root level lacked.
- `7b95b5b`, `27f07fc` — `image::package` writes an application to be
  started, under `FLAG_START`, and a started image must carry exactly one
  root.
- `9d02911`, `5c9b8ad` — `ns-cli` reads a JSON project file, with either
  separator in every path and UNC and verbatim prefixes kept.
- `23bd056`, `898d045` — `run` assembles an application with `new` or a
  project's manifest, and `--send` is gone from `run`.
- `59407ec` — project goldens pin the manifest (`goldens/manifest/`).
- `bf776f2` — `build` packages the application just before `main:args:`,
  `load` starts it, and the `snapshot` command is gone.
- `7c9e082` — `bench` times `main:args:` of an application assembled like
  `run`'s.
- `97561cc` — `run-corpus` starts each corpus module through a generated
  application, in all four lanes.
- `763fbe2` — the stateless binary, its header flag and its entry field
  retire.
- `088cd6c` — the documentation describes the built state.
- `cec79bf` — comments that cited retired headings of `image-format.md`, the
  host-send callers of `check_method_arity` and the commands sharing the source
  pipeline match the tree; `run-corpus` names its lanes "image" and
  "reference image".
- This commit — the whole-branch review's text findings in the documentation.

All thirteen tasks were reviewed, with fix rounds until each review was
clean. The end-of-batch gates passed: the three stress modes, 2625 of 2625
each; `ns-lsp`, 40 of 40; `run-corpus` "ran 6, failed 0, skipped 205" plain and
under each stress mode; the release suite, 2364 passed. The review of the whole
branch found no code defect; its text findings were fixed in the two commits
after `088cd6c`. What the batch leaves open is in [OPEN-WORK.md](OPEN-WORK.md).

## The root rule is a resolve error (2026-09-30)

A module root whose clauses break the root rule is refused by the resolver,
for every module root, at the offending clause, before anything runs
([object-model.md](design/object-model.md), "The root rule"). Decided with
the user as a deviation from §6.1 ([README.md](design/README.md),
"Deviations from the language specification"): refused only at
instantiation, a project module nothing instantiated ran under `run` while
`build` refused it. The rule allows exactly the seven combinations of
`Object` and `Value` that work -- none, `Object`, `Object new`, `Value`,
`Value new`, `Object <: Value` and `Object new <: Value` -- and now also
refuses a superclass clause that sends anything but `new`
(`goldens/errors/root_superclass_sending_other_than_new_is_refused.ns`) and a
`<:` clause that sends anything; `Value <: Value` stays refused. The instantiation
refusal and the image encoder's and decoder's refusals stay for what does not
come through the resolver: a `ResolvedModule` a host assembles by hand, and
an image. The ten `goldens/errors/root_*` goldens now end with the resolve
error; the editor underlines the clause.

## Are We Fast Yet, stage 1 (2026-09-30)

Plan: [ns-next-are-we-fast-yet-stage-1](superpowers/plans/2026-09-30-1353-ns-next-are-we-fast-yet-stage-1.md);
[execution.md](design/execution.md), "The Are We Fast Yet benchmarks";
[core-library.md](design/core-library.md), "What exists", "The platform"
and "Large integers".
Commits: `072a102` (the plan's) to the commit of this entry.

Seven of the Are We Fast Yet benchmarks -- Sieve, Permute, Queens, Towers,
Bounce, Storage and List -- are runnable programs in
`benchmarks/are-we-fast-yet/`, ported from SOMns into Primordial Soup's
module shape and protocol (decided with the user on 2026-09-30, for stage
1), under a harness adapted from Primordial Soup's `BenchmarkRunner.ns`:
the root of the project `are-we-fast-yet.json`, which reaches each
benchmark as `manifest Sieve`. Without `--timed` the harness runs each
benchmark once and prints `<Name>: ok`, which `goldens/awfy/` pins under
both lowerings and `ns-next bench` times; with `--timed` it prints each
benchmark's runs per second, measured with `platform time Stopwatch`.
The kernel gained, in Primordial Soup's protocol, `Number`'s `to:do:` and
`to:by:do:` as Newspeak, and ten `MIRROR` rows: `Integer`'s and
`LargeInteger`'s `\\`, `bitAnd:` and `asFloat`, `Float`'s `<=`, `>=` and
`asStringFixed:`, and the clock; the platform gained `time`, whose
`Stopwatch` is the one class of `PlatformFactory.ns` that is an object
rather than a value. No wire tag changed, and no existing golden's outcome.
`THIRD-PARTY-NOTICES.md` carries SOMns's MIT notice and Primordial Soup's
Apache 2.0 licence.

- `0516126` — `Number` iterates with `to:do:` and `to:by:do:`, and `Float`
  orders with `<=` and `>=`.
- `bb9c59b` — `Integer` and `LargeInteger` answer `\\`, the remainder of
  floor division.
- `fbfd946` — `Integer` and `LargeInteger` answer `bitAnd:` over two's
  complement.
- `8c74685` — integers answer `asFloat`, and `Float` answers
  `asStringFixed:` rounding as Primordial Soup does.
- `41c7a6c` — `platform time` answers Primordial Soup's `Stopwatch` over a
  monotonic clock.
- `7232111` — a loaded stopwatch's test pins its start, and the clock's
  comments say what holds: the origin is per process.
- `61ee9e0` — the harness and Sieve, verified by goldens, with
  `THIRD-PARTY-NOTICES.md`'s SOMns and Primordial Soup entries.
- `eb77d7e` — the notices introduce all four upstreams, and the harness's
  usage line names the command.
- `38c692c` — Permute, Queens and Towers in Primordial Soup's protocol.
- `5c66ddc` — Bounce, Storage and List complete stage 1.
- `dc25ee4` — a timed run's scores, and `bench` over the project, are
  checked by shape.
- This commit — the documentation describes the built state.

What the batch left open is in [OPEN-WORK.md](OPEN-WORK.md): stage 3 (stage
2 is built, below), a program's exit status, and whether to inline `to:do:`
and `to:by:do:`.

## Are We Fast Yet, stage 2 (2026-09-30)

Plan: [ns-next-are-we-fast-yet-stage-2](superpowers/plans/2026-09-30-2156-ns-next-are-we-fast-yet-stage-2.md);
[execution.md](design/execution.md), "The Are We Fast Yet benchmarks";
[core-library.md](design/core-library.md), "What exists", "Large integers"
and "Gap: an integer has no `/`".
Commits: `5699ab0` (the plan's) to the commit of this entry.

Mandelbrot, NBody and Richards join the seven benchmarks of stage 1 in
`benchmarks/are-we-fast-yet/`, ported from SOMns into Primordial Soup's
module shape, and Primordial Soup's own Richards, the Ton80 version, comes
in beside them as `RichardsTon80`: eleven programs under the one harness.
Mandelbrot and NBody run at the sizes Are We Fast Yet pins. The kernel
gained, in Primordial Soup's protocol: an integer's `+ - * < > <= >=`
meeting a `Float` argument, as a `Float`'s meet an integer one; `bitOr:`,
`bitXor:` and `<<` for `Integer` and `LargeInteger`, over two's complement,
and `Integer>>bitInvert` in Newspeak; and `Float>>sqrt`. The platform did
not change. No wire tag changed. One golden's outcome changed on purpose,
`goldens/errors/a_large_integer_meets_a_float` becoming
`a_large_integer_meets_nil`, because a `LargeInteger` receiver now answers a
`Float` argument.

The decisions, each the user's (2026-09-30):

1. Stage 2 follows Primordial Soup's protocol, as stage 1 does; the ports
   translate SOMns's division selectors (`//` is float division there, `/`
   in Primordial Soup) and leave out Richards's tracing, so no `platform
   system` protocol is needed.
2. Both Richards versions come in, as `Richards` and `RichardsTon80`.
3. Mandelbrot (750, answering 50) and NBody (250000 steps) run at Are We Fast
   Yet's pinned sizes, knowing a run takes seconds in a release build.
4. `Mandelbrot.ns` keeps the Computer Language Benchmarks Game's BSD-3 header
   as SOMns carries it; `THIRD-PARTY-NOTICES.md` gets no entry for it.

Mixed arithmetic, a proposal the user had not confirmed when the plan was
written, is built as proposed: `=` stays class-tight and `//` and `\\` take
integers only. Cross-class `=` (with the numeric tower) and `//` with a
`Float` operand are recorded as open in [OPEN-WORK.md](OPEN-WORK.md); `\\`
with a `Float` is a refusal Primordial Soup makes too.

- `4f90ca5` — an integer's arithmetic and ordering meet a `Float` argument.
- `5a451f7` — comments name the modulo selector and describe an integer
  meeting a `Float`.
- `472d4d5` — a test's doc comment names the mismatches it really drives.
- `eccbfdc` — `Integer` and `LargeInteger` answer `bitOr:`, `bitXor:` and
  `bitInvert` over two's complement.
- `4ba1e1f` — `Integer` and `LargeInteger` shift left with `<<`, refusing a
  result that cannot be allocated.
- `58e8765` — `Float` answers `sqrt`.
- `2f4af6e` — Mandelbrot and NBody in Primordial Soup's protocol, at Are We
  Fast Yet's sizes.
- `4634a60` — the Are We Fast Yet Richards and Primordial Soup's Ton80
  Richards.
- This commit — the documentation describes the built state.

What the batch leaves open is in [OPEN-WORK.md](OPEN-WORK.md): stage 3,
`//` with a `Float` operand, and a program-chosen size that a
system which overcommits accepts and then kills the process for.
