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
| M4 — Representation: arena, checked roots, stack activations | In progress | — | [heap-seam](superpowers/plans/2026-09-08-2034-ns-next-heap-seam.md), [object-and-frame-arena](superpowers/plans/2026-09-09-1952-ns-next-object-and-frame-arena.md), [activation-stack](superpowers/plans/2026-09-10-2145-ns-next-activation-stack.md), [retire-tree-walker](superpowers/plans/2026-09-11-1848-ns-next-retire-tree-walker.md), [clean-up-with-one-engine](superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md) |
| M5–M6 — JIT | Not started | — | — |

## Where the next session starts

M4 is under way, and **the next piece is C, the reference lowering. Its design
is written first** ([compiler.md](design/compiler.md), "Planned: the reference
lowering"), before any plan or code, and it has to settle how a recursion-depth
limit counts: the reference lowering activates blocks the optimized one
inlines, so a limit that counts activations would trip at different points
under the two, and 1b needs the answer. The bytecode VM is the only engine
([execution.md](design/execution.md), "One engine"): step A retired the
tree-walker on 2026-09-11, because designing 1b, the trampoline, showed that a
recursive AST evaluator cannot take part in what the trampoline exists for —
suspending a running computation, the debugger, edit-and-continue on a live
activation. Three steps come before 1b, each landing on its own:

- **A — Retire the tree-walker. Done.** Its verdicts were harvested into golden
  outcomes first, then everything that existed only because there were two
  engines was removed; see "M4 — Retiring the tree-walker" below.
- **B — Clean up with one engine. Done.** Behaviour-preserving refactoring,
  measured wherever it touched a hot path, with the golden outcomes and the
  image bytes as the guard; see "M4 — Cleaning up with one engine" below.
- **C — The reference lowering. Next; its design is written first.** The
  differential oracle the retired tree-walker used to be: a second,
  deliberately naive lowering, under which every program must behave byte for
  byte as it does under the optimized one ([compiler.md](design/compiler.md),
  "Planned: the reference lowering"). How the prior art bounds recursion depth
  is recorded in [execution.md](design/execution.md), "How other systems bound
  recursion depth".

M4's pieces are one change: the object representation, the activation
representation and the root discipline each constrain the other two, which is
why the object and activation representations were converted together rather
than one at a time. Each piece is designed in the document for its own aspect;
those documents are the authority, and this is only the map to them.

**Five pieces are done.** The heap seam routes every object and activation
access through `Heap` (`crates/ns-runtime/src/heap.rs`), and the arena behind
it replaced `Rc<RefCell<_>>`: an `ObjRef` or a `FrameRef` is a 32-bit index
into a `Vec` the `Heap` owns, with a private field so the compiler enforces the
seam. That is worth ~15% of the benchmark and is described in
[memory.md](design/memory.md); see "M4 — The heap seam" and "M4 — The object
and frame arena" below. Then activations moved onto a stack the `Heap` owns
(step 1a below), reified into the frame arena only when a block captures one;
see "M4 — The activation stack" below. **Nothing reclaims an arena slot** —
every object and every reified frame is kept for the life of the process. That
is a deliberate interim cost. The activation stack bounded it for activations
nothing captures, and step 5, the collector, is what ends it. Then step A
retired the tree-walker and step B cleaned up after it; see "M4 — Retiring the
tree-walker" and "M4 — Cleaning up with one engine" below.

**Before 1b fixes its baseline, profile `bench_block.ns`.** Its time did not
move under the activation stack, although the stack took the block's own
activation out of the frame arena; what offsets that saving was not known.
Retiring the tree-walker took about 4% off it, attributed to a smaller closure.
Step B then followed the lead: storing a closure behind an `Rc`, so that
`value:` bumps a reference count instead of copying the closure out of the
heap, took about 17% off it, measured against its parent commit
([execution.md](design/execution.md), "What step B moved"). Whether that
copy is what offset the stack's saving is not established, because no profile
was taken, so the profile is still owed. What it has to explain is smaller:
measured against `master` in one sitting, `bench_block.ns` cost about 360 ns
per iteration more than `bench_send.ns` before step B and about 218 ns after
it — roughly the creation, reification, invocation and divorce of one block,
since both methods otherwise do one `+` apiece.

The list below is M4's order of work — 1a done, A to C inserted ahead of 1b
with A and B done, everything from 1b on still ahead — and it is not the order
the milestone was originally sketched in. Two things settled the order of the
numbered steps, neither of them a preference.

The first is a measurement. On a release build, before the arena, a
user-method send cost ~165 ns, of which ~68 ns — about 41% — was spent building
the activation, and none of that is touched by an inline cache. The
decomposition is in [execution.md](design/execution.md)'s measurements section.
The arena took part of that activation cost and the activation stack more of
it: measured against the arena in one sitting, a user-method send went from
~146 ns to ~91 ns, and what it costs beyond a native send from ~62 ns to ~37 ns.

The second is that the root discipline is defined *over* the activation stack,
so it wanted the stack to exist first. With the arena in place and roots
checked, the collector's root set is the bootstrap plus the activation stack —
"a contiguous, precisely typed pair of arrays rather than a pointer-chased chain
of individually allocated frames", as [memory.md](design/memory.md) puts it.
Establishing the discipline against the frame arena would have meant
establishing it against a representation step 1 then replaced, and threading
lifetimes through `Interpreter::send`, `run_code` and `Signal::Return` twice.
The discipline costs nothing at run time and pays off only with the collector,
so nothing is lost by it going second.

1. **Activations on a stack, reified on demand** —
   [execution.md](design/execution.md). Cog's model, as Primordial Soup
   implements it for Newspeak. Split in two, each landing green and measured on
   its own:
   - **1a — the stack. Done.** Activations are entries on one stack the `Heap`
     owns, addressed by an `ActivationRef`; a `Frame` is reified only when a
     block is created, and divorced — its registers copied out — when its
     activation returns. A user-method send fell from ~146 ns to ~91 ns, and
     an activation nothing captures leaves nothing behind; see "M4 — The
     activation stack" below.
   - **1b — the trampoline. After A to C.** `send` split into a decision and an
     activation, arguments copied straight from the caller's registers,
     `Block value` absorbed, recursion depth an ordinary Newspeak error.
     Designed in [execution.md](design/execution.md), "Planned: the
     trampoline", which also lists what 1a left for it; no plan is written yet.
2. **A compiler-checked root discipline** — [memory.md](design/memory.md). Nova's
   `GcScope`/`NoGcScope`, where holding an unrooted handle across a collection
   safepoint is a type error rather than corruption found later under collection
   pressure. Free at run time; the ergonomic cost is accepted deliberately.
3. **Inline caches** — [dispatch.md](design/dispatch.md). The call-site slots are
   already emitted and inert, so no bytecode format break. A cache must guard on
   the **mixin application**, not on the class object; the obvious key is the
   wrong one and would never hit. A native entry carries an enumerated
   primitive, not a selector or a function pointer, so a hit costs the guard and
   an indexed jump and the JIT can emit the primitive inline
   ([dispatch.md](design/dispatch.md)).
4. **Closure conversion** — [execution.md](design/execution.md). Blocks copy the
   variables they only read and share a box for the ones they write, instead of
   reaching them through their method's frame. That ends 1a's divorce copy and
   makes a surviving block retain only what it captured, which is why it comes
   directly before the collector. Every Smalltalk and Newspeak VM that moved to
   stack frames made this change to do it; ns-next makes it second, because the
   reification mechanism is needed with or without it.
5. **A moving collector** — [memory.md](design/memory.md). `become:` and instance
   migration rule out a conservative non-moving design, and pinning whatever a
   conservative stack scan finds was considered and rejected for the same reason.
   This is what ends the arena's never-reclaim interim.

Deferred past the milestone on purpose: moving `do:` out of Rust and into
Newspeak. It is required eventually, because a Rust native that calls back into
Newspeak is a permanent barrier to optimization
([core-library.md](design/core-library.md)), but doing it before the steps above
makes the numbers worse and after them costs nothing. M4 owes it only that the
rewrite not deepen the investment in Rust callbacks.

What M4 unlocks beyond speed is recorded where it applies: suspending a running
computation and serializing it ([image-format.md](design/image-format.md)) waits
on the trampoline, and so does the debugger; `become:`-based class mutation
waits on the collector
([liveness-and-reflection.md](design/liveness-and-reflection.md)).

## M1 — Frontend (2026-09-01)

`ns-syntax`: hand-written lexer, recursive-descent and Pratt parser, and AST for
the official Newspeak3 textual syntax, plus `ns-next parse` and `parse-corpus`.
Validated against the vendored corpus at 210 of 211 files, with one deferred
lexer ambiguity in `corpus/blacklist.toml`.

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
Carries the statement→PC safepoint table and a reserved inert inline-cache slot
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
reach `Rc<RefCell<_>>` directly. `ObjRef` and `FrameRef` became opaque
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
`frames: Vec<Frame>`; an `ObjRef`, a `FrameRef` and the `ClassRef` alias are
32-bit indices into them, declared in `crates/ns-runtime/src/heap.rs` with a
**private** field — so the compiler, not a grep gate, refuses every read of a
handle from anywhere else. The grep gate `CLAUDE.md` used to carry is removed.
`ObjRef`, `FrameRef` and `Value` are now `Copy`. Converted in two steps —
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
  construction, the crate's only `unsafe`. An arena knows a slot's index before
  it fills the slot, so an object can name itself in safe Rust.
  `grep -rn "unsafe" crates/ns-runtime/src/heap.rs` is empty.

What it added: a `debug_assertions`-only origin guard. Each `Heap` carries an id,
each handle records the id of the heap that minted it, and all 46 accessor sites
assert the two match. This exists because reading a handle through the wrong heap
is the one new failure mode the arena introduces and it is *silent* — an index
from another heap is in range and names a different, perfectly valid object. In a
release build neither the field nor the check exists, so a handle is exactly its
32-bit index. The same reasoning retired a proof `interpreter/nested.rs` had been
relying on: `cache_slot` used `0` as the sentinel for a non-object receiver,
justified by "0 is an address no live `Rc` has", and under the arena index 0 is
the first object allocated. It now returns `Option<(ObjRef, usize)>`, so the case
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
activation cost — `args` and `temps` are still a `Vec` allocation apiece per
activation, and that is the next step's, not this one's. The tree-walker is
unchanged within noise, as expected.

The interim cost is real and is the number the collector will be judged against.
Nothing reclaims a slot, so Windows peak working set over `bench_loop.ns` went
from 4.6 MB to 25.4 MB for one tree-walk run, and from 4.6 MB to 340.7 MB for
`bench`'s twenty-two entry sends. Retention is proportional to every activation
ever created, not to what is live. The tree-walk maximum in the same ten timed
runs also went from ~38 ms to ~117 ms, which the arena's `Vec` doubling would
explain — an inference from the shape of the growth curve rather than a measured
cause, and one the next step contradicted for this benchmark (below). The
conditions the peak figures were taken under, and the control that would settle
the doubling attribution, are in [memory.md](design/memory.md), "Open: what the
collector owns"; the tree-walk figures, retired with the engine, are recorded
only here.

## M4 — The activation stack (2026-09-10)

Plan: [ns-next-activation-stack](superpowers/plans/2026-09-10-2145-ns-next-activation-stack.md).

Step 1a of M4. Every Newspeak activation runs on one activation stack the `Heap`
owns — `ActivationStack` in `crates/ns-runtime/src/heap.rs`, one contiguous
`Vec<Value>` of registers and one `Vec<StackEntry>` — addressed by a new handle,
`ActivationRef`, which is 4 bytes in a release build. A `Frame` exists only when
a block is created: `Heap::reify` builds one *married* to its stack entry, so a
block reads its method's variables through the live stack slot, and
`Heap::pop_activation` *divorces* it when the activation returns, copying the
registers out, because ns-next's blocks read those variables by reference. The
frame arena holds `FrameSlot`s — a `Frame` plus the entry it is married to,
private to `heap.rs`. The four sites that build an activation (`invoke_method`,
`invoke_closure`, `run_initializer_level`, `run_clause_body`) push and pop under
both engines, and `Interpreter::send` and `instantiate` debug-assert that they
leave the stack balanced. Every function that means *the running activation*
takes an `ActivationRef`; closures, frames and image records take only a
`FrameRef`, so storing a stack handle is a compile error. Accessors are
`activation_*` for a live activation and `frame_*` for a reified frame. The
design is in [execution.md](design/execution.md), "Activations".

What it deleted rather than moved:

- The `sender` parameter of `Interpreter::send`, `invoke_method`,
  `invoke_closure`, `builtins::dispatch_native`, `builtins::call_block` and the
  `builtins/` modules. The design had it retyped to an `ActivationRef`; it was
  removed instead, because its only use was to fill `Frame.sender`, and once a
  live activation's sender is the entry below it, nothing read it.
- `FrameRef`'s `receiver_and_current_class`, `local`, `set_local`,
  `frame_slot_base` and `frame_is_method`, replaced by their `activation_*`
  counterparts, and `mark_uncontinuable`, whose job is now part of
  `pop_activation`'s divorce; `receiver` and `grow_temps` became
  `frame_receiver` and `frame_grow_temps`. Outside tests, `Heap::alloc_frame`
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
saving has not been attributed, since no profile was taken; and what the design
left for later — the argument vector `collect` builds for every send, the
`HashMap` lookup per invocation that finds a compiled body, and recursion on the
Rust stack with no depth limit. The first and the last are 1b's, the lookup the
inline caches'. And the tree-walker's ~112 ms maximum on `bench_loop.ns`, which
the arena's `Vec` doubling was thought to explain, survived under a binary that
retains nothing for that benchmark — 4.6 MB across all twenty-two entry sends —
so that explanation does not hold for it; its cause was never found. What the
bytecode maxima do and do not show is in [memory.md](design/memory.md), "Open:
what the collector owns".

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
- From the image format: the frame record's pc-kind byte, so the minimum frame
  record is 18 bytes rather than 19. `PC_KIND_*` retired its two numbers,
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
  selector, args)`, `instantiate(class, args)`, `invoke_closure(interp,
  closure, args)`, `run_code(interp, act, code)`. `ClosureData`,
  `CompiledModule` and `LoadedImage` no longer carry one.

**What replaced the differential for now** is recorded answers: every fixture
against its `.expected` stdout and an empty stderr, the golden outcomes, and
`run-corpus` — `CLAUDE.md`'s "Program outcomes are pinned, not re-derived".
Step C's reference lowering is the planned differential oracle.

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
`run-corpus` at **6/0/205**.

What the retirement left for step B — its inventory of leftovers, the items its
plan assigned B, and what its final review found — step B cleared, except the
four items listed under "M4 — Cleaning up with one engine" as "Left,
deliberately".

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
   code. Recorded in [dispatch.md](design/dispatch.md), "Planned: inline
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
7. **Handles by value.** Every function taking an `ObjRef`, `ClassRef`,
   `FrameRef` or `ActivationRef` takes it by value — 143 sites — and
   `clippy::trivially_copy_pass_by_ref`, sized by a new `clippy.toml` to see
   the debug-build `ActivationRef` and `pub` functions too, keeps it so. Four
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

**Left, deliberately** — each is outside a behaviour-preserving cleanup:

- **Two error messages that name a milestone.** `builtins/numbers.rs`'s
  `type_mismatch` ("... (Milestone 1 has no Integer/Float coercion) ...") and
  `interpreter/literals.rs`'s `parse_int_literal` ("fractional radix literals
  are not supported in Milestone 1"). A program observes them on stderr, so
  rewording them is a behaviour change that needs its own decision.
- **The discarded `Object` allocation in `build_builtin_classes`.** Its loop
  allocates a class object for `BuiltinClass::Object` and then discards it in
  favour of `class_of_class`. Removing it would shift every later identity hash
  by one, which a program and an image both observe.
- **`ClosureData.params` duplicating `code.num_args`.** At run time only the
  count is read, but the image's closure record carries the names, so dropping
  them is a wire change. Step B shares the vector instead of copying it.
- **The lexical-send slot lookup.** `dispatch_lexical_send` interns the getter
  name for every setter-shaped selector and hashes the name into the
  application's `slot_index` for every lexical send. A lexical-send inline-cache
  entry that records the slot's index removes both
  ([dispatch.md](design/dispatch.md), "Planned: inline caches").

## Notes

**Milestone renumbering (2026-09-06).** Language completeness was pulled ahead of
the performance work, so M3 became nested classes and source-free images, inline
caches and the GC became M4, and the JIT tiers became M5–M6. Plans written before
that date use the old numbering.

**Design documentation (2026-09-07).** The dated design specs under
`docs/superpowers/specs/` were folded into the living documents in `docs/design/`
and removed. Git holds the originals.

**Deferred follow-ups from the artifact-inspection branch — cleared 2026-09-08.**
The final whole-branch review raised four items as real but not merge-blocking.
All four are done: the CLI tests share one self-removing scratch directory
(`crates/ns-cli/tests/common/mod.rs`), the stale comments in
`crates/ns-cli/src/main.rs` are corrected, and `image-format.md`'s code list
names `opcodes.rs`.

One of the four was recorded inaccurately and is worth stating properly, since
the same shape will recur. The note said only `disasm_snapshots.rs` failed to
remove its temporary directories while the other CLI tests cleaned up after
themselves. They did — but by hand, at roughly thirty separate call sites, which
is exactly why four were missed. A full suite run left 807 directories in the
system temp folder; it now leaves none. Cleanup that every call site has to
remember is cleanup that some call site will forget.

`cli.rs` and `run.rs` keep their own handling: they remove what they create,
and rewriting working tests for symmetry is tidying rather than fixing.

**Deferred follow-ups from the object-and-frame-arena branch — cleared
2026-09-10.** Three items were left out of the arena commit deliberately;
all three are done.

*`identity_key` moved onto `Heap`.* It was the one handle operation with no
origin check, because it was handed a handle and no `Heap` to check against:
two handles from different arenas produced the same key and collided silently
in a caller's map. Folding the debug-only `origin` field into the key was
tried on the arena branch and reverted — it would have made the collision
impossible in exactly the build that reports it and left it as possible in
the build that ships. `Heap::object_identity_key` and
`Heap::frame_identity_key` check the origin like every other accessor, and
the compiler enforces the rule in the most literal way available: no `Heap`,
no key. All seven call sites already had one in reach;
`GraphTable::object_id`, `GraphTable::frame_id` and `encode_frame_ref` gained
a `&Heap` parameter to pass it on.

*Source comments citing plan bookkeeping.* The sweep was wider than the
`Task[s]? [0-9]+` grep that motivated it: the same unresolvable references
also appear as "Gap 1 (Task 12 brief)", "Final-review finding I6", "the
brief's original example" and "a later task's job". 99 lines across 32 files,
comments only, no code changed. Three outcomes per site — drop the
reference where the sentence stands without it, state what the comment means
where it carried its point through the reference, and keep it where the
number is a genuine algorithm or specification step
(`newspeak-spec.pdf`'s Class Instantiation steps, `interpreter/nested.rs`'s
numbered accessor steps, `image/mixins.rs`'s decode steps). Two comments
turned out to be stale rather than merely opaque and were corrected:
`build_mixin`'s claim that a class side has no accessor to dispatch to
(`metaclass_of` applies it) and the lexer's claim that `skip_trivia` only
skips ASCII whitespace (it handles nested block comments too).

*`.cloned()` over a `Copy` element.* 30 sites, 18 under `src` and 12 under
`tests`, applied with `cargo clippy --fix`. The lint that finds them,
`clippy::cloned_instead_of_copied`, is in `clippy::pedantic` and off by
default; it is now enabled in `[workspace.lints.clippy]` with `[lints]
workspace = true` in each crate manifest, so the existing "clippy must be
warning-free" gate covers it and the question stops needing a note here.
