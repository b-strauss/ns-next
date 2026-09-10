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
| M4 — Representation: arena, checked roots, stack activations | In progress | — | [heap-seam](superpowers/plans/2026-09-08-2034-ns-next-heap-seam.md), [object-and-frame-arena](superpowers/plans/2026-09-09-1952-ns-next-object-and-frame-arena.md) |
| M5–M6 — JIT | Not started | — | — |

## Where the next session starts

M4 is under way. Its pieces are one change: the object representation, the
activation representation and the root discipline each constrain the other
two, which is why the object and activation representations were converted
together rather than one at a time. Each piece is designed in the document for
its own aspect; those documents are the authority, and this is only the map to
them.

**Two pieces are done.** The heap seam routes every object and activation
access through `Heap` (`crates/ns-runtime/src/heap.rs`), and the arena behind
it replaced `Rc<RefCell<_>>`: an `ObjRef` or a `FrameRef` is now a 32-bit
index into a `Vec` the `Heap` owns, with a private field so the compiler
enforces the seam. That is worth ~15% of the benchmark and is described in
[memory.md](design/memory.md); see "M4 — The heap seam" and "M4 — The object
and frame arena" below. **Nothing reclaims a slot** — the arena grows for the
life of the process. That is a deliberate interim cost. Step 1 below bounds it
for activations, and step 5, the collector, is what ends it.

The order below is the order the remaining work goes in, and it is not the order
the milestone was originally sketched in. Two things settled it, neither of them
a preference.

The first is a measurement. On a release build a user-method send cost ~165 ns,
of which ~68 ns — about 41% — was spent building the activation, and none of
that is touched by an inline cache. The decomposition is in
[execution.md](design/execution.md)'s measurements section. The arena took part
of that activation cost; step 1 takes the rest.

The second is that the root discipline is defined *over* the activation stack,
so it wants the stack to exist first. With the arena in place and roots checked,
the collector's root set is the bootstrap plus the activation stack — "a
contiguous, precisely typed array rather than a pointer-chased chain of
individually allocated frames", as [memory.md](design/memory.md) puts it.
Establishing the discipline against today's frame arena would mean establishing
it against a representation step 1 then replaces, and threading lifetimes
through `Interpreter::send`, `run_code` and `Signal::Return` twice. The
discipline also costs nothing at run time and pays off only with the collector,
so nothing is lost by it going second.

1. **Activations on a stack, reified on demand** —
   [execution.md](design/execution.md). Cog's model, as Primordial Soup
   implements it for Newspeak. What the arena left behind: `Frame.args` and
   `Frame.temps` are still a `Vec` allocation apiece per activation, and every
   activation is reified whether or not anything looks at it. Designed in full
   and split in two, each landing green and measured on its own:
   - **1a — the stack.** Activations become entries on one stack the `Heap`
     owns, addressed by a new `ActivationRef`; a `Frame` is reified only when a
     block is created, and divorced — its registers copied out — when its
     activation returns. Both engines share it. Bounds what the arena currently
     leaves unbounded, since an entry that was never reified leaves nothing.
   - **1b — the trampoline.** `send` split into a decision and an activation,
     arguments copied straight from the caller's registers, `Block value`
     absorbed, recursion depth an ordinary Newspeak error.
2. **A compiler-checked root discipline** — [memory.md](design/memory.md). Nova's
   `GcScope`/`NoGcScope`, where holding an unrooted handle across a collection
   safepoint is a type error rather than corruption found later under collection
   pressure. Free at run time; the ergonomic cost is accepted deliberately.
3. **Inline caches** — [dispatch.md](design/dispatch.md). The call-site slots are
   already emitted and inert, so no bytecode format break. A cache must guard on
   the **mixin application**, not on the class object; the obvious key is the
   wrong one and would never hit.
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
on the activation stack, and so do the debugger and `become:`-based class
mutation ([liveness-and-reflection.md](design/liveness-and-reflection.md)).

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
cause. Both, with the conditions the figures were taken under and the control
that would settle the attribution, are in [memory.md](design/memory.md), "What
the collector owns".

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

`cli.rs`, `differential.rs` and `run.rs` keep their own handling: they remove
what they create, and rewriting working tests for symmetry is tidying rather
than fixing.

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
