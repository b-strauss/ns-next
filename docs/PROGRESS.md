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
| M4 — Representation: arena, checked roots, stack activations | In progress | — | [heap-seam](superpowers/plans/2026-09-08-2034-ns-next-heap-seam.md), [object-and-frame-arena](superpowers/plans/2026-09-09-1952-ns-next-object-and-frame-arena.md), [activation-stack](superpowers/plans/2026-09-10-2145-ns-next-activation-stack.md), [retire-tree-walker](superpowers/plans/2026-09-11-1848-ns-next-retire-tree-walker.md), [clean-up-with-one-engine](superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md), [reference-lowering](superpowers/plans/2026-09-12-1157-ns-next-reference-lowering.md), [tag-named-identity-hashes](superpowers/plans/2026-09-12-1604-ns-next-tag-named-identity-hashes.md), [trampoline](superpowers/plans/2026-09-12-1619-ns-next-trampoline.md), [no-reentry](superpowers/plans/2026-09-13-0149-ns-next-no-reentry.md), [checked-roots-2b](superpowers/plans/2026-09-13-1748-ns-next-checked-roots-2b.md), [lifetime-brand-2c](superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md) |
| M5–M6 — JIT | Not started | — | — |

## Where the next session starts

M4 is under way. **Step 2, the compiler-checked root discipline, is done**,
in the three parts it was designed in on 2026-09-13 ([memory.md](design/memory.md),
"Checked roots and a collector"). **2a, no re-entry from Rust** — `do:`,
instantiation and the nested-class accessor are trampoline steps, so Rust
enters the run loop only as the host; see "M4 — No re-entry from Rust" below.
**2b, the safepoint, the tracing, host roots and the stress mode** — the run
loop polls a safepoint, `trace.rs` enumerates every handle a collector would
trace, a host keeps values across runs only as `Root`s, and
`NS_NEXT_STRESS_MOVES=1` moves everything at the safepoints under the whole
suite, which `CLAUDE.md` makes a gate; see "M4 — The safepoint, tracing, host
roots and the stress mode" below. **2c, the lifetime brand** — every movable
handle carries a lifetime obtainable only inside `Interpreter::enter`, in safe
Rust, so holding one across a safepoint, keeping one outside an entry, and a
native re-entering the loop fail to compile; see "M4 — The lifetime brand"
below. Measured whole, it is level with `a5d1e05` on every source; its first
measurement was not, and finding out why put a function-alignment pin into
the measurement protocol — and only there (the first finding below).

**The next piece is step 3, the inline caches** ([dispatch.md](design/dispatch.md),
"Planned: inline caches"; item 3 of the order of work below). A candidate
small step of its own comes before them: a cached getter symbol for
setter-shaped lexical sends and a hasher for `u32`-shaped keys in the method
and slot tables, which would take most of `Symbol::intern`'s 11.9% of
`bench_send.ns` out before a lexical-send cache entry removes the lookup
altogether ("The profile of `bench_block.ns` is taken" below says why). Each
gets its own plan under `docs/superpowers/plans/`, written against the code
as 2c left it. Read for it: [dispatch.md](design/dispatch.md), "Planned:
inline caches" (a cache guards on the mixin application, and a native entry
carries an enumerated primitive); [compiler.md](design/compiler.md),
"Safepoints and call-site identity", for the inert call-site slots every
`CompiledCode` already carries;
[execution.md](design/execution.md), "The VM", for the send path the caches
sit on, where each instruction is one entry of the step state, `Mutator`; and
[memory.md](design/memory.md), "The brand" and "What the collector traces" —
code is shared behind an `Rc`, where no handle may live, so a cache entry in
it must hold none, which a key on the application satisfies. Performance is
measured, not assumed: the protocol and the seven sources of "What removing
the re-entries moved" ([execution.md](design/execution.md); the four
throwaway sources are written out in the
[2c plan](superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md),
"Measurement protocol"), **every compared binary, the current tree
included, built with the protocol's explicit flags through `RUSTFLAGS`** —
`+crt-static` and the function-alignment pin, which no configuration
carries — two sittings for a whole step, and a comparison against `be3eb40`
as well as against the commit the step starts from (the second finding
below).

**Open findings**, for whoever plans next to weigh:

- **A measurement builds every compared binary with the same explicit
  flags, function alignment pinned; the shipped build is unpinned.** 2c's
  first whole-step measurement read 1–2.5% slower than `a5d1e05` on the
  block and instantiation benchmarks, `bench_new2.ns` beyond noise in both
  sittings, and a bisection placed it on the brand commit, `1549fb0`. The
  cause was placement, not code: the brand commit's hot machine code is
  instruction for instruction identical to its parent's, but smaller cold
  host code ahead of it moved every hot function by 224 or 240 bytes into a
  different 64-byte phase, and with function starts pinned to 64 bytes
  (`-C llvm-args=-align-all-functions=6`) the two commits measured level.
  Under the pin the whole step is level with `a5d1e05` on every source in
  both sittings, −1.4% to +0.6%, every group overlapping
  ([execution.md](design/execution.md), "What 2c moved"). The unpinned
  parent reads −0.1% to −1.8% against the pinned one, overlapping on all but
  one group, and the pin costs about 110 KB of padding, 5.6% of the binary:
  it buys repeatability, not speed. So the user decided on 2026-09-14 that the
  pin belongs to the measurement protocol only — `.cargo/config.toml` carries
  `+crt-static` and nothing else, and every binary a measurement compares,
  the current tree included, is built with
  `RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`
  set for that build, since `RUSTFLAGS` replaces the configuration's flags
  ([execution.md](design/execution.md), "What removing the re-entries
  moved", the protocol). The two earlier "uniform shift on unchanged code"
  findings of 2a likely share the mechanism, verified only for 2c.
- **The comparison with `be3eb40` continues.** 2a left `bench_loop.ns` 1.4–1.9%
  and `bench_block.ns` 1.4–1.6% slower than `be3eb40`, the commit it started
  from — a uniform codegen shift in `vm::run` and `dispatch_native`, traced as
  far as inlining and register allocation and not bisected; code placement,
  the mechanism the protocol now pins, is the likely cause, verified only
  for 2c. 2b
  took `bench_block.ns` to 2.5% faster than `be3eb40`, and 2c, measured
  under the pin, keeps it: `bench_block.ns` −2.2% and −3.2%,
  `bench_newarg.ns` −2.9% and −2.1% (2b: −3.2% to −3.4%), `bench_do.ns`
  −3.5% and −2.5%, `bench_new.ns` and `bench_new2.ns` level. `bench_send.ns`
  is +3.1% and +2.2%, entirely above `be3eb40` in both sittings (2b: +0.9%
  to +1.4%), and `bench_loop.ns` +1.7% and +1.7% — the parent's own shift,
  `a5d1e05` sitting +2.5% to +2.7% above `be3eb40` on the send and +1.1% to
  +2.0% on the loop in the same sittings, with 2c level with it
  ([execution.md](design/execution.md), "What 2c moved"). The rule stays:
  **every performance measurement up to and including step 3 compares
  against `be3eb40` as well as against the commit it starts from**, reports
  both, and builds every binary with the same codegen flags (the finding
  above). If a source is still slower than `be3eb40` once step 3 has landed,
  the shift is chased then, on the rebuilt send path.
- **A decoded image's last initializer is not held to answering `self`**, so
  `new` answers whatever it answers — wrong but safe, no panic.
  `image::decode::verify_mixins` could refuse such an initializer, which would
  make "`new` answers the instance" hold for images too.
- **No test reaches a `^` that targets a superclass clause's own activation.**
  A clause's receiver is a single name, so only a crafted image can put a
  block in that body; the clause *message*'s case, a `^` caught by the
  message's activation, is pinned
  (`a_return_in_a_clause_messages_block_hands_its_value_to_the_walk`).
- **The stress suite takes three minutes** (190 s against 13.1 s), almost half
  of it one test, `the_depth_limit_is_exact_and_counts_alike_under_both_lowerings`.
  It is a gate before every commit, so step 3's plan should budget for it.

Sends run on the activation stack rather than the Rust stack, in one loop that
pushes, pops and resumes; `value`, the control-flow natives, `do:`,
instantiation and the nested-class accessor are trampoline steps, so nothing
re-enters the loop from Rust; and recursion depth has one counted bound,
100,000 activations, counted by the rule in [compiler.md](design/compiler.md),
"How the limit counts" — an ordinary Newspeak error that agrees under both
lowerings ([execution.md](design/execution.md), "The depth limit"). The bytecode
VM is the only engine ([execution.md](design/execution.md), "One engine"):
step A retired the tree-walker on 2026-09-11, because designing 1b showed that
a recursive AST evaluator cannot take part in what the trampoline exists for —
suspending a running computation, the debugger, edit-and-continue on a live
activation. Three steps came before 1b, each landing on its own, and all three
are done:

- **A — Retire the tree-walker. Done.** Its verdicts were harvested into golden
  outcomes first, then everything that existed only because there were two
  engines was removed; see "M4 — Retiring the tree-walker" below.
- **B — Clean up with one engine. Done.** Behaviour-preserving refactoring,
  measured wherever it touched a hot path, with the golden outcomes and the
  image bytes as the guard; see "M4 — Cleaning up with one engine" below.
- **C — The reference lowering. Done.** The differential oracle the retired
  tree-walker used to be: a second, deliberately naive lowering, under which
  every program must behave byte for byte as it does under the optimized one
  ([compiler.md](design/compiler.md), "The reference lowering"). Every
  recorded program answers alike under both. Probing with it found and fixed
  one defect in the optimized lowering, and lazy identity hashes left one
  known regression — a builtin class, the root class and `platform` did not
  keep their hash across a snapshot — closed by the small fix below. See
  "M4 — The reference lowering" below.

M4's pieces are one change: the object representation, the activation
representation and the root discipline each constrain the other two, which is
why the object and activation representations were converted together rather
than one at a time. Each piece is designed in the document for its own aspect;
those documents are the authority, and this is only the map to them.

**Ten pieces are done.** The heap seam routes every object and activation
access through `Heap` (`crates/ns-runtime/src/heap.rs`), and the arena behind
it replaced `Rc<RefCell<_>>`: an `ObjectReference` or a `FrameReference` is a 32-bit index
into a `Vec` the `Heap` owns, with a private field so the compiler enforces the
seam. That is worth ~15% of the benchmark and is described in
[memory.md](design/memory.md); see "M4 — The heap seam" and "M4 — The object
and frame arena" below. Then activations moved onto a stack the `Heap` owns
(step 1a below), reified into the frame arena only when a block captures one;
see "M4 — The activation stack" below. **Nothing reclaims an arena slot** —
every object and every reified frame is kept for the life of the process. That
is a deliberate interim cost. The activation stack bounded it for activations
nothing captures, and step 5, the collector, is what ends it. Then step A
retired the tree-walker, step B cleaned up after it, and step C built the
reference lowering as the differential oracle; see "M4 — Retiring the
tree-walker", "M4 — Cleaning up with one engine" and "M4 — The reference
lowering" below. Then 1b took sends off the Rust stack; see "M4 — The
trampoline" below. Then 2a took the last re-entries of the run loop from Rust
away; see "M4 — No re-entry from Rust" below. Then 2b put a safepoint into the
run loop, enumerated every handle a collector would trace, rooted what a host
keeps, and checked all of it by moving everything under the whole suite; see
"M4 — The safepoint, tracing, host roots and the stress mode" below. Then 2c
branded every movable handle with the lifetime of the entry that minted it,
so the compiler checks what 2b's stress mode could only find at run time; see
"M4 — The lifetime brand" below.

**The profile of `bench_block.ns` is taken**
([execution.md](design/execution.md), "What the profiles show"). The question
was why `bench_block.ns`'s time did not move under the activation stack,
although the stack took the block's own activation out of the frame arena. The
answer is allocation: before the trampoline, a send's argument vector was 16.7%
of the timed samples, `value` as a native callback 12.1%, reifying the frame
into an arena that grows by doubling 9.6% and the divorce on return 8.9%; the
trampoline took the first two. Profiling the trampoline's head as well found
one cost no measurement had isolated: `Symbol::intern` is 11.9% of
`bench_send.ns`, because a lexical send with a setter-shaped selector —
`self tick: sum` — interns its getter name on every send to look for a slot,
and the method and slot tables hash `Symbol` keys with SipHash. That is the
lexical-send slot lookup listed as left for the inline caches; a cached getter
symbol and a hasher for `u32`-shaped keys would take most of it out sooner,
and are a candidate small step of their own before the caches.

The list below is M4's order of work — 1a done, A to C inserted ahead of 1b
and all three done, 1b done, 2 done in all three parts, everything from 3 on still ahead — and it is
not the order the milestone was originally sketched in. Two things settled the order of the
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
lifetimes through `Interpreter::send`, the run loop and `Signal::Return` twice.
The discipline costs nothing at run time and pays off only with the collector,
so nothing is lost by it going second.

1. **Activations on a stack, reified on demand** —
   [execution.md](design/execution.md). Cog's model, as Primordial Soup
   implements it for Newspeak. Split in two, each landing green and measured on
   its own:
   - **1a — the stack. Done.** Activations are entries on one stack the `Heap`
     owns, addressed by an `ActivationReference`; a `Frame` is reified only when a
     block is created, and divorced — its registers copied out — when its
     activation returns. A user-method send fell from ~146 ns to ~91 ns, and
     an activation nothing captures leaves nothing behind; see "M4 — The
     activation stack" below.
   - **1b — the trampoline. Done.** `send` split into a decision and an
     activation, run by one loop on the activation stack; arguments gathered
     into one reusable buffer instead of a vector per send; `Block value` and
     the control-flow natives trampoline steps; recursion depth an ordinary
     Newspeak error under two bounds (2a removed the second, on re-entries from
     Rust), the first counted by the rule in
     [compiler.md](design/compiler.md), "How the limit counts", with
     `SendInlinable` and the counter added together. `bench_loop.ns` fell by
     about 32%, `bench_send.ns` 31% and `bench_block.ns` 22%; see "M4 — The
     trampoline" below.
2. **A compiler-checked root discipline. Done.** [memory.md](design/memory.md),
   "Checked roots and a collector". Designed on 2026-09-13, from how
   Cog, Nova, CPython, piccolo, V8, HotSpot, Primordial Soup and SOMns place
   their collections: a collection only ever at a safepoint in the run loop,
   never in an allocation; no Rust frame live below one; and a lifetime brand in
   the style of gc-arena, which makes holding a handle across a safepoint a
   compile error. That replaced the earlier plan of Nova's
   `GcScope`/`NoGcScope`, whose scopes would root the re-entry paths rather
   than remove them. The ergonomic cost is accepted deliberately. Split in
   three, each landing green and measured on its own:
   - **2a — no re-entry from Rust. Done.** `do:`, instantiation and the
     nested-class accessor are trampoline steps, and the bound on re-entries
     went with them ([execution.md](design/execution.md), "No re-entry from
     Rust"). Measured whole, instantiation came out 0.9–3.8% faster, `do:` and
     `bench_send.ns` level, and `bench_loop.ns` and `bench_block.ns` 1.4–1.9%
     slower — an open finding, above; see "M4 — No re-entry from Rust" below.
   - **2b — the safepoint, the tracing, host roots and the stress mode. Done.**
     The run loop polls a safepoint at every switch and every backward jump,
     `trace.rs` enumerates every place a handle is stored, a closure's payload
     sits inline in its object so that no handle is behind a shared `Rc`, a
     host keeps values across runs only as `Root`s, and a debug-build stress
     mode moves everything at the safepoints, on a budget paced by heap size,
     under the whole suite. Measured whole, no source is slower than before
     the step beyond noise and `bench_block.ns` is about 4% faster; the suite
     takes 12.7 s, and 190 s under the stress mode; see "M4 — The safepoint,
     tracing, host roots and the stress mode" below.
   - **2c — the brand. Done.** Every movable handle carries a lifetime
     obtainable only inside `Interpreter::enter`, which lends the step state,
     `Mutator`, to a closure generic over it in safe Rust — the crate forbids
     `unsafe` code — once per instruction the run loop executes, so a host
     holds values only as `Root`s and reads through `enter`. Measured whole,
     every source is level with `a5d1e05`; a first measurement read 1–2.5%
     slower on the block and instantiation benchmarks, which turned out to be
     code placement, and the measurement protocol now pins every function
     start to a 64-byte boundary in every binary it compares (the first
     finding above); see "M4 — The lifetime brand" below.
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
Newspeak. It is required eventually, so that `do:` becomes a real method that
lookup, mirrors, the debugger and a JIT see — a `do:` in Rust is opaque to a
JIT, which cannot inline its block or compile the loop as a unit
([core-library.md](design/core-library.md)) — but
doing it before the steps above makes the numbers worse and after them costs
nothing. M4 owed it only that the rewrite not deepen the investment in Rust
callbacks, and step 2a took the last callback away, as a trampoline step
rather than as Newspeak.

What M4 unlocks beyond speed is recorded where it applies: suspending a running
computation and serializing it needed the trampoline and the removal of the
re-entries from Rust, both built, and still needs a yield path out of the run
loop and a re-entry point for a decoded frame
([image-format.md](design/image-format.md), "Planned: suspend and resume"),
and so does the debugger; `become:`-based class mutation waits on the
collector ([liveness-and-reflection.md](design/liveness-and-reflection.md)).

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
activation cost — `arguments` and `temporaries` are still a `Vec` allocation apiece per
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

**What replaced the differential for now** is recorded answers: every fixture
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

**Left, deliberately** — each is outside a behaviour-preserving cleanup:

- **Two error messages that name a milestone.** `builtins/numbers.rs`'s
  `type_mismatch` ("... (Milestone 1 has no Integer/Float coercion) ...") and
  `interpreter/literals.rs`'s `parse_integer_literal` ("fractional radix literals
  are not supported in Milestone 1"). A program observes them on stderr, so
  rewording them is a behaviour change that needs its own decision.
- **The discarded `Object` allocation in `build_builtin_classes`.** Step C
  removed it once identity hashes became lazy, so dropping an allocation no
  longer shifted any hash.
- **`ClosureData.parameters` duplicating `code.argument_count`.** At run time only the
  count is read, but the image's closure record carries the names, so dropping
  them is a wire change. Step B shares the vector instead of copying it.
- **The lexical-send slot lookup.** `dispatch_lexical_send` interns the getter
  name for every setter-shaped selector and hashes the name into the
  application's `slot_index` for every lexical send. A lexical-send inline-cache
  entry that records the slot's index removes both
  ([dispatch.md](design/dispatch.md), "Planned: inline caches").

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
   the rule is ns-next's own. Neither `SendInlinable` nor the counter is built:
   the counter is its only consumer, so both arrive with 1b
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

**Left, deliberately:**

- **`SendInlinable`, the depth counter, the limit's value and its error text**
  — 1b's, together with whether the control-flow natives become trampoline
  steps. Until then they call back into Newspeak on the Rust stack, so under
  the reference lowering a recursion through an `ifTrue:` costs more native
  stack per level and aborts sooner. The deepest recursion a suite runs under
  it is eleven levels, `fact: 10` in
  `crates/ns-runtime/tests/reference_lowering.rs`; the fixtures reach six
  ([compiler.md](design/compiler.md), "How the limit counts"). Closed by 1b;
  see "M4 — The trampoline" below.
- **Programs without a recorded answer** — generated programs, fuzzing — and
  the direct run-against-run comparison only they would need. While every
  program the suites run has a recorded answer, "both produce the recorded
  answer" is the comparison.
- **`ns-runtime`'s integration tests under the reference lowering.** They test
  dispatch, the natives, instantiation and the image, which the two lowerings
  share.
- **The two error messages that name a milestone, `ClosureData.parameters` and the
  lexical-send slot lookup**, still as listed under "M4 — Cleaning up with one
  engine".

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
stack is the only place a running computation lives, apart from the three paths
that still re-enter the loop from Rust — `do:`, instantiation and the
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
`04e5fc0` (above, and [execution.md](design/execution.md), "What the profiles
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

**Left, deliberately:**

- **Instantiation, the nested-class accessor and `do:` stay re-entries of the
  loop**, counted against N. `do:` moves to Newspeak after the inline caches;
  the other two could become trampoline steps once something needs it. (All
  three became trampoline steps in 2a; see "M4 — No re-entry from Rust" below.)
- **The unchecked `arguments[0]`** in the control-flow natives (`builtins/boolean.rs`,
  `builtins/block.rs`). The decoder does not check a send's argument count
  against its selector, so a crafted artifact that sends `ifTrue:` with no
  argument still reaches it and panics
  ([image-format.md](design/image-format.md)).
- **A `^` into a running initializer** still escapes as "internal: non-local
  return escaped the entry send", the known gap, now pinned by a test.
- **Catchable exceptions.** Both depth errors unwind to the entry send like
  every other error. `Heap::activation_is_live`'s release-build answer rests on
  nothing being pushed while a signal propagates, so an `ensure:` or `on:do:`
  needs the activation generation in every build first
  ([execution.md](design/execution.md), "Non-local return").
- **Suspending a running computation**: a yield path out of the loop, and a way
  past its re-entries from Rust, which went in 2a
  ([image-format.md](design/image-format.md), "Planned: suspend and resume").
- **Small follow-ups the task reviews deferred.** A nested run's argument
  buffer is dropped when the outer one is given back, so a re-entry with
  arguments allocates once, and a returned buffer keeps its stale values until
  the next send (a collector concern). `catches_return` clones a `FrameKind`
  to test its variant; `doesNotUnderstand:` copies the send's arguments; a
  native loop's body step looks its closure up again every iteration;
  `run_initializer_levels` allocates a list per instantiation — none measured.
  The argument buffer's stale values and every value a continuation holds
  (`WhileLoop`) are roots step 2, the root discipline, has to account for.
  `vm::send` carries `#[allow(clippy::too_many_arguments)]`. `inspect`'s walk
  bounds the tag-named hash count at one byte per entry where the decoder uses
  five; it never pre-allocates from the count, so this is only an
  inconsistency.
- **The memory bound under a crafted artifact is real but large.** A decoded
  code table may nest blocks 128 deep, the parser's own limit, so up to 128
  exempt activations can sit above each counted one: at most 129 × L stack
  entries, about 1.3 GB of entries at the size measured when this landed
  ([execution.md](design/execution.md), "The depth limit"). The encoder
  does not refuse deeper nesting itself — no parsed source can produce it, so
  only a hand-built `CompiledCode` could write an image the decoder then
  refuses.
- **The two error messages that name a milestone, `ClosureData.parameters` and the
  lexical-send slot lookup** (now in `sends::decide_lexical_send`), still as
  listed under "M4 — Cleaning up with one engine".

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
are slightly slower, an open finding.** The instantiation sources gained what
the boxes and the drop glue had cost, and running an initializer in the same
loop, once the walk allocates nothing, is cheaper than a nested run. `do:` as
a step came out −1.3%, overlapping — not clearly faster, although the design
expected it to be. `bench_loop.ns` and `bench_block.ns`, which run none of the
changed paths in their loops, are 1.4–1.9% slower: the uniform codegen shift
the step's first change showed, not isolated.

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.5 s in two runs alike, as before the step. Before the marker
it took 25 s: the two tests that recurse through the nested-class accessor to
the activation limit made the old list-scanning cyclic-definition check
quadratic, 7.8 s each.

662 tests pass, with one ignored (644 before); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**.

**Left, deliberately:**

- **The two slightly slower benchmarks** and **`verify_mixins` not holding a
  decoded last initializer to `self`**, for whoever plans next to weigh
  ("Where the next session starts").
- **`do:`'s per-element argument vector.** Each element's block activation
  takes a one-element `StepArguments::Owned` vector; a one-value `StepArguments` variant
  would avoid it, and would change `Step`'s layout, which the trampoline
  measured as a 4–8% risk.
- **Suspend and resume must encode each stack entry's `Continuation`**, with
  the Rust state it carries — `WhileLoop`, `DoLoop`, `NestedClassCreation`,
  `Instantiation` — which no image records today
  ([image-format.md](design/image-format.md), "Planned: suspend and resume").
- **Step 2b must split `DoLoop.block`** along with every other
  `Rc<ClosureData>` that holds a handle behind a shared `Rc`
  ([memory.md](design/memory.md), "What the collector traces").
- **A `^` into a running initializer** still escapes as "internal: non-local
  return escaped the entry send", the known gap, unchanged.
- **Moving `do:` into Newspeak**, after the inline caches, as before.

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
   `image::snapshot` and `image::build` keep taking plain values, fetched just
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
`be3eb40` in the same sittings — 2a's figure read in other sittings, carried
as an open finding ("Where the next session starts").

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.7 s, against 12.5 s before the step. Under
`NS_NEXT_STRESS_MOVES=1` it takes 190.2 s: `crates/ns-runtime/tests/trampoline.rs`
91.5 s against 3.0 s, nearly all of it
`the_depth_limit_is_exact_and_counts_alike_under_both_lowerings`, and
`ns-cli`'s tests, which spawn `ns-next` per fixture or golden, about 96 s
against 7.6 s.

685 tests pass, with one ignored (662 before); `parse-corpus` stays at
**210/0/1/0** and `run-corpus` at **6/0/205**, plain and under the stress mode.

**Left, deliberately**, the one finding of the reviews that is not closed:
two stale handles from the same old epoch compare in `same()` without a
panic, since `same` has no heap to check them against. 2c's brand closed it
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
3. **No public `unbind`**: `rebrand`, private to `heap.rs`, converts a value
   for the host root table, and nothing else changes a brand.
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

`cli.rs` and `run.rs` were left with their own helpers at the time, on the
reasoning that they removed what they created. They did not: `cli.rs`'s
`tempdir()` never removed its directory, so every suite run left one behind
(670 by 2026-09-13), and the other helpers removed theirs only when a test
passed. Since 2026-09-13 every CLI test uses `ScratchDirectory`, whose `Drop` runs
on the way out of a failing test too, and `ScratchDirectory` writes under the
repository's git-ignored `.temp/` instead of the system temp folder, so that
anything a killed run still leaves behind stays in the project.

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
`GraphTable::object_id`, `GraphTable::frame_id` and `encode_frame_reference` gained
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
