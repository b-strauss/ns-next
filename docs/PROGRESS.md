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

M4 is complete. **Step 2, the compiler-checked root discipline, is done**,
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
the measurement protocol — and only there (the third finding below).

**Name resolution and lookup follow the specification**, a step inserted
ahead of the inline caches and done: a slot's accessors are method-table
entries that a subclass overrides, §5.7 lets only lexically *declared*
members decide a lexical send, and every send kind applies its access rule
(§5.5–§5.10, §6.3.2). The method and slot tables hash their identity keys
with a multiply-rotate hasher instead of SipHash. Measured whole, no source is
slower than `59061ec` or `be3eb40`, and six are 4–12.5% faster than both; see
"M4 — Spec-conformant lookup" below.

**Step 3, the inline caches, is done** (designed
2026-09-15; [dispatch.md](design/dispatch.md), "Inline caches"; item 3 of the
order of work below), in three parts, each landing green and measured on its
own. **3a, primitives are method-table entries, is done**: every native is
an entry of its builtin class's method table carrying an enumerated
primitive, found by the ordinary lookups; see "M4 — Primitives as
method-table entries" below. **3b, the caches, is done** (2026-09-16): every
send the optimized lowering compiles probes a cache at its call site, per
heap, indexed from the activation's `feedback_base`; a site goes
monomorphic, then a polymorphic cache of 4, then megamorphic, with a global
cache of 1024 entries — 512 two-slot groups since the step below — behind
every site; the guard is a dense per-application
dispatch id; a send's decision is
`resolve` then `act`, so a cache records what the lookup decided and never
decides; call sites are numbered per code family as `u32`s behind new send
tags; the reference lowering runs uncached as the oracle; and
`NS_NEXT_STRESS_CACHES=1` vacates every cache at every safepoint, a gate of
its own. Measured whole, every source is 4.3–11.3% faster than 3a's head and
9.4–19.7% faster than `be3eb40`; see "M4 — Inline caches" below.

**The step before 3c is done** (2026-09-18): five changes the user decided on
2026-09-17 and 2026-09-18 ([dispatch.md](design/dispatch.md), "Decisions",
"Decided by the user … built before 3c"), planned and measured together;
see "M4 — Flat metaclasses, the factory and the cache refinements" below.
Designing the factory change is what added the first two: §3.2 settles, from
a section the earlier reading had not reached, that a class side is not
inherited at all.
- **Metaclasses are flat** (§3.2, §6): a class side is applied to `Class` —
  to `Object`'s behaviour here, since ns-next reifies no `Class` — instead of
  to the superclass's metaclass, so a class side is not inherited and a
  class-side `super` send looks in `Object`'s behaviour.
- **The primary factory is a class-side method** (§6.3), an entry of the
  metaclass mixin's method table found by the ordinary lookups. With flat
  metaclasses the factory's special cases went with nothing in their place:
  a factory entry sits only on the receiver's own metaclass, which the
  class-side guard determines, so neither re-check survives. A class side may
  not declare the factory's selector; the resolver refuses it. The factory
  selector is a member of the class side for §5.7's depth search too, so an
  implicit `new` inside a class-side method reaches the class that declares
  it rather than an enclosing class's member of the same name.
- **The global cache's lexical key includes N's application**, so a vacant
  lexical site may use it.
- **Call-site numbers are no longer serialized**; the decoder numbers each
  family, with a flag for a family compiled uncached, and the send tags
  retired again, to 25–28.
- **The global cache is 512 two-slot least-recently-used groups**, measured
  against 3b's insert with `goldens/bench/bench_megamorphic.ns` — which
  measures the probe path only, so the replacement rule itself is still
  unmeasured (the finding below).

The rest of §3.2 — the reified instance-of relation and the `Class`,
`Behavior` and `Metaclass` kernel classes — is **scheduled for step 6, the
core library in Newspeak**, where both references declare them and which sits
immediately before the reflection work that cannot be done without them
([object-model.md](design/object-model.md), "Class sides and metaclasses").

**3c, the dispatch id's place, is done** (2026-09-19): a guard reads the id
from the object header, and a class object's data is boxed so that an
`Object` stays 56 bytes in a release build. Neither header variant measured
faster than the id on the class object, and the rule proposed with the
measurement would have kept it there; the user chose the header for a JIT
tier's guard and step 6's uniform stamping; see "M4 — The dispatch id's
place" below.

**The specification gaps are done** (2026-09-19): the worst deviation the two
specification audits found — a §5.7 member set keyed by a class's simple
name, so two same-named classes in one module shared one set, silently
wrong for eighteen corpus modules and invisible to the reference lowering
because the mistake was in the resolver, above both — and the two cheap
fixes the second audit found alongside it — equal symbol literals not one
object (§5.1.6) and a parameter slot accepting a setter send (§3.6) — plus
two changes the design work surfaced along the way — a local slot's
declaration kept and its initializer run (§3.6), and a parameter's or an
immutable local's register never copied. All five decided with the user on
2026-09-19 and built in one plan
([specification-gaps](superpowers/plans/2026-09-19-0300-ns-next-specification-gaps.md)).
See "M4 — The specification gaps" below for what changed, commit by commit,
and the decisions behind each.

**Removing `:=` is done** (2026-09-19): the lexer refuses Smalltalk's
assignment by name, and every golden, test and document example is spelled
as §5.4.3's setter send, `name:: expression`, with no recorded outcome
moving; see "M4 — Removing `:=`" below.

**Closure conversion, step 4 of the order of work below, is done**
(2026-09-20; designed 2026-09-19,
[execution.md](design/execution.md), "Closure conversion"): under the
optimized lowering a block no longer reaches its method's variables through
the method's frame. A captured variable nothing writes after capture is
copied into the closure at `MakeClosure`; one that is written lives in its
scope's indirection vector, an ordinary `Array` built by `MakeVector` and
read and written by `LoadVector`/`StoreVector`. A block's registers are its
parameters, its copied values, its temporaries and then the hidden vector
register. `ClosureData` carries `copied` and derives its home from its
captured frame, at the same 32 bytes as before. Whether a returning
activation copies its registers into its reified frame is decided per
activation, from `CompiledCode.outer_reach`, a depth. The fallback closures
an inlined conditional sends to a non-Boolean receiver keep the by-reference
model, and so does the reference lowering, which stays the oracle. The
protective register copies are now asked per register rather than assumed for
every send. See "M4 — Closure conversion" below.

**Measured whole, it costs nothing and gives nothing back on speed**: the
head is level with `efac7d2` on all eight sources in both sittings, and stays
6–19% below `be3eb40`. The one concrete cost is memory, and it is the
conversion's own: `bench_block.ns`'s peak working set rose about 2.6%
(454.7 MB to 466.4 MB), which a three-way measurement attributes to the
conversion rather than to the copy-narrowing. `vm::run` grew from 4,878 to
5,700 instructions and stayed one inlined function. See
[execution.md](design/execution.md), "What closure conversion moved".

**Step 5, the moving collector, is done** (2026-09-20;
[ns-next-moving-collector](superpowers/plans/2026-09-20-1746-ns-next-moving-collector.md),
nine tasks over the shape decided with the user the same day,
[memory.md](design/memory.md), "The shape: mark, compact, forward" onward):
**mark-compact over the index arena, one space, stop-the-world**, with
generations as a later, separately measured step. Marking is a second
`Tracer` beside `Forwarder`, driven from the roots through the one edge
enumeration in `trace.rs`; compaction slides each arena's live slots to the
front, truncates and shrinks with hysteresis (`max(4096, live * 2)` after
each collection), and rewrites every handle through a flat forwarding table.
The canonical symbol table is now the system's only weak structure; the
application cache stays strong because its key is an address, which
`Rc::as_ptr` needs a strong reference to keep unique. A collection runs only
at a safepoint of the run loop, requested by a comparison `push_object` and
`reify` already make against an arena's limit, never inside an allocation.
`NS_NEXT_STRESS_COLLECT` is a third stress mode and gate, collecting at
every safepoint to catch premature reclamation, which the move stress
mode's everything-is-live sweep cannot see. `bench --collector-statistics`
reports the counts and pauses. This ends the arena's never-reclaim interim:
before this step nothing gave an object or a reified frame back, which is
why `bench_block.ns` grew with the work it did whichever way its blocks
reached their variables; the indirection vector closure conversion added is
itself an object the collector now reclaims.

**Measured against `dc0ffe5` and `be3eb40`: peak working set is the point of
the step, and it moved as designed.** `bench --iterations 10` on
`bench_block.ns` — the one source that allocates a closure and reifies a
frame every iteration, the interim's worst case — falls from 228.8 MB to
5.6 MB, level with `bench_loop.ns` and `bench_send.ns`. Seven of the eight
sources are level with `dc0ffe5` on throughput and 5.3–18.0% below
`be3eb40`; six of them overlap the baseline's group in both sittings, and
`bench_newarg.ns`, pooled at +2.9%, is the narrowest of the seven — entirely
above `dc0ffe5` in the second sitting and overlapping it by only 0.01 ms in
the first — and it is the source with the second-most collections, 107.
`bench_loop.ns`, `bench_send.ns` and `bench_megamorphic.ns`
report zero collections and are exactly the three whose throughput does not
move — the recorded expectation named the first two by exactly this
reasoning in advance, and `bench_megamorphic.ns` turns out to fit it too.
`bench_block.ns` is the one source that does not stay level — **+12.2%
against `dc0ffe5`**, the one source whose pooled groups do not overlap in
either sitting and the only one beyond a couple of percent — because
it is the only one that collects enough to show: 269 collections, 15.479 ms
total pause, 0.202 ms longest. Pause accounts for roughly 1.4 ms of the
2.46 ms the head adds per round; the rest is recorded as unexplained rather
than guessed at. See "M4 — The moving collector" below,
[execution.md](design/execution.md)'s "What the moving collector moved" and
[memory.md](design/memory.md)'s "How the collector is measured" for the
full tables.

**The unexplained remainder on `bench_block.ns` is an open finding, not a
closed one.** Pause accounts for roughly 1.4 ms of the 2.46 ms the head adds
per round there; nothing in this measurement explains the rest. What the
numbers do support: on the four other collecting sources the pause is
1.7–5.4% of the median (`bench_do.ns` 0.12 ms of 7.06 ms; `bench_newarg.ns`
0.49 ms of 9.13 ms) while the measured difference against `dc0ffe5` runs
−2.1% to +2.9%. Set against the change actually measured, the pause on those
four — far from being additive — *exceeds* it by 0.13 to 0.38 ms per send:
collecting gives more back there than it costs, plausibly because an arena
that never passes its limit never doubles and copies. On `bench_block.ns`
the pause falls 1.05 ms per send short instead, and that sign change across
four sources is the part most in need of explaining
([execution.md](design/execution.md), "What the moving collector moved", has
the table).

One candidate is ruled out: freeing the dead payloads is **not** a mutator
cost, since `compact_arena`'s `truncate` drops them inside the span
`World::collect` times. What distinguishes `bench_block.ns` from the other
four is that it collects 269 times against their 26 to 107 at a similar live
object count, and that it is the only source with a populated frame arena
(1,529 live frames against 0–1 elsewhere), so the only one whose closures
have their `captured_frame` rewritten at every collection. A cost that scales
with how *often* a program collects rather than with the total pause would be
the cache after-effect of walking the live graph, which falls outside the
measurement. **These are hypotheses, not findings.**
Three cheap experiments, none run. A fixture whose blocks capture nothing
keeps the object churn and removes the frame reification, separating the
frame arena from everything else. Varying `COLLECTION_FLOOR` so the same
program collects less often but does more per collection separates a cost
that tracks the collection count — the cache after-effect — from one that
tracks the pause total. And to separate the trigger's cost from the
collection's: a head build with `COLLECTION_FLOOR` raised above
`bench_block.ns`'s whole allocation, which then never collects but still
pays the trigger's comparison in `push_object` and `reify` — that isolates
whether the remainder is the reallocation cost above or something the
trigger's comparison itself costs. Not yet run.

**M4's numbered steps are now complete.** What comes next, after M4 and
before the JIT tiers, is **step 6, the core library in Newspeak, and then
step 7, the payload space** (the order of work below). That ordering was
decided with the user on 2026-09-21: step 6 first because it removes the
one constraint that complicates the space -- a payload that grows in place,
which exists only because `OrderedCollection` is a Rust builtin -- and
because it raises what the space is worth, a Newspeak `List>>add:`
allocating a fresh `Array` at every capacity overflow where a `Vec::push`
amortizes to nothing.

**Step 6's first part, the inlined fast path for the integer leaves, is
done** (2026-09-21;
[ns-next-leaf-inlining](superpowers/plans/2026-09-21-2107-ns-next-leaf-inlining.md);
[compiler.md](design/compiler.md), "Arithmetic inlining"). The compiler
recognizes the nine binary integer primitives (`+ - * // < > <= >= =`) at an
ordinary explicit-receiver send and emits `Instruction::IntegerBinary`,
which computes when both operands are `Value::SmallInteger` and the
`checked_*` operation yields a result and otherwise jumps to the ordinary
send standing beside it, so every error the operation can raise — a
`doesNotUnderstand:`, an overflow, a zero divisor — still comes from
`builtins/numbers.rs` and from nowhere else. **Measured by the protocol
below against `6f5a5b1`, the commit on `master` the branch was cut from,
every one of the eight sources is 11.5% to 31.0% faster, and every
after-group lies entirely below its before-group in both sittings**. The
only earlier step to clear that gate across its whole suite was 3b, the
inline caches, at 4.3–11.3% over seven sources. `vm::run` grew from 5,732 to
5,866 instructions and stayed one inlined function. See "Leaf inlining"
below and [execution.md](design/execution.md), "What leaf inlining moved".

**This is the cushion step 6's move to the VM mirror was sequenced to have.**
Rewriting `do:`, `collect:` and the kernel classes as Newspeak turns every
arithmetic operation in those bodies into a send the interpreter dispatches;
the numbers above are what that rewrite now has to spend before it shows as
a regression against `6f5a5b1`, and the fast path is what supplies it.

**What step 6 still needs, and it has a design but no plan yet.** The design
is [core-library.md](design/core-library.md), "Planned: the core library as
Newspeak source over a VM mirror", decided with the user on 2026-09-21 and
written up in four parts: why the mirror is the whole Rust boundary (§8.3.1
requires it — "A primitive call is a message send to the VM mirror" — which
rules out the primitive pragma an earlier version of that document named as
the target), how the bootstrap fills empty class placeholders from the
compiled `Kernel.ns` before instantiating it, where the library source lives
and how modules are found, and what the step must not break.

It breaks into three plans, deliberately not one, for the reason the leaf
inlining was split off first: each produces working, measured software on its
own, and each one's measurement informs the next.

1. **The vmMirror, `Kernel.ns` and the bootstrap.** The largest of the three.
   The mirror is an object of a builtin class whose method table carries every
   leaf as an enumerated primitive, so step 3a's machinery is kept whole and
   only the class holding the entries changes. Expect the arithmetic-heavy
   benchmarks to regress against `e2bb31e` by whatever the leaf inlining does
   not absorb; that is the number this step is measured on.
2. **`Class`, `Behavior` and `Metaclass`** — the rest of §3.2, which
   [object-model.md](design/object-model.md) describes and which the
   reflection work after step 6 cannot start without.
3. **The payload space**, step 7 of the order of work, whose own measurements
   are named under [memory.md](design/memory.md), "Payloads belong to the Rust
   allocator, and what that costs". Run one of them before building anything
   there.

Two things are recorded as decided but unbuilt, and neither should be
rediscovered: `Integer` has no `/` and step 6 has to settle what an exact
quotient answers without a `Fraction` class
([core-library.md](design/core-library.md), "Gap: an integer has no `/`"); and
the arithmetic fast path can drop its `Jump` by inverting the branch sense,
which requires retiring wire tag 33 and taking 34
([compiler.md](design/compiler.md), "Arithmetic inlining").

**The baseline was `efac7d2`**, the merge of removing `:=` into `master`;
`22a56f7`, the merge of 3c, stayed superseded, and `be3eb40` was carried
beside it, as every measurement since
step 2. Performance is measured, not assumed: the protocol and the seven
sources of "What removing the re-entries moved"
([execution.md](design/execution.md)), plus
`goldens/bench/bench_megamorphic.ns`, the eighth source and the only one
that drives a megamorphic site.

**Four of the eight sources are goldens, under `goldens/bench/`. The other
four are not in the tree at all and must be written out from the text the
documents give, never reconstructed from their prose descriptions.**
A reconstruction runs and prints the right answer while measuring something
else, and older comparison binaries may not run it at all. All four —
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns`, whose slots had to be
renamed when accessors became method-table entries, and `bench_do.ns` — are
written out in [execution.md](design/execution.md), "What spec-conformant
lookup moved", spelled with the setter send. The
[2c plan](superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md)'s
older copies spell `:=`, which the tree no longer parses; take them from
execution.md.

**Every compared binary, the current tree included, was built with the
protocol's explicit flags through `RUSTFLAGS`** — `+crt-static` and the
function-alignment pin, which no configuration carries — two sittings,
compared against the baseline and against `be3eb40`. The head is level with
`efac7d2` on every source, no source entirely above in both sittings, and
6–19% below `be3eb40`. Closure conversion was measured in two steps against
the one baseline — the conversion alone, then the narrowed protective copies
on top of it — because they are separate changes with separate risks: the
conversion changes what every block creation and activation does, and the
copy-narrowing changes the code every method compiles to. `dumpbin /disasm` checked
before any timing that `vm::run`'s step was still inline: one function of
4,878 instructions at the baseline and 5,700 at the head, no split closure.
The measurement, and the template for the next, is
[execution.md](design/execution.md), "What closure conversion moved".

**Known gaps the step left**, each recorded where it applies:

- **Lazy slots are eager** (§6.3.3): the parser drops `lazy`, and the slot is
  initialized with the others ([README.md](design/README.md), deviations).
- **The class side's own scope**: both sides of a class declaration
  contribute to one member set for lexical resolution; the specification is
  silent and the canonical compiler scopes the class side apart
  ([dispatch.md](design/dispatch.md), "Resolution").
- **A builtin class name is loaded, not sent** (§3.2): `Object` and the other
  builtin names resolve statically rather than as sends to the accessors
  §3.2 describes ([dispatch.md](design/dispatch.md), "Deviation: a builtin
  class name is loaded, not sent").
- **`nil printString` answers `'an Object'`**: `Object>>printString` is a
  stub (`crates/ns-runtime/src/builtins/object_class.rs`, `an Object` for
  every receiver); no golden pins the correct answer yet.

**The two specification audits' other findings are closed**, in "M4 — The
specification gaps" below: the member-set fix, the symbol-identity fix and
the parameter-setter fix. Mixin application (§6.2) is the one finding left
open, and it fails loudly — a parse error — rather than answering wrongly,
so it is not the same class of defect. **Five goldens are still `skipped`**
(`grep -l '^skipped' goldens/*/*.toml`): the parameter-and-a-local-of-the-
same-name compile check (§3.6), `nil isNil` (§5.1.3), mixin application
(§6.2), the sequential simultaneous slot clauses (§6.3.2) and the eager lazy
slot (§6.3.3) — the last two also rows in [README.md](design/README.md)'s
deviations table, answering a **correct** program **wrongly, at exit status
0**, and worth reading before trusting a program's output.

**Open findings**, for whoever plans next to weigh:

- **The global cache's replacement rule is unmeasured.** The cache is 512
  two-slot groups with the least recently *used* evicted, Strongtalk's
  `lookupCache` rule, and it replaced 3b's insert on that precedent, not on
  numbers of this runtime's own. `goldens/bench/bench_megamorphic.ns`, added
  for the comparison, cannot supply them: it fills 27 of 1024 slots, five of
  them at the one megamorphic site it drives, so across 512 groups its hot keys
  almost certainly share no group — almost every hit is at the first slot, the
  second is almost never read and almost certainly nothing is evicted. What it
  measured is the probe path, one hash and one cache line against 3b's two
  hashes anywhere in the table, and there the change moved no source beyond
  noise. Deciding the rule on numbers
  needs cache pressure: many more hot keys than any fixture has, or a debug
  build's small `NS_NEXT_GLOBAL_CACHE_ENTRIES`
  ([dispatch.md](design/dispatch.md), "The global cache";
  [execution.md](design/execution.md), "What the step before 3c moved").
- **The run loop's step sits near LLVM's inlining limit.** A send's
  decision is sensitive to the shape it travels through, not only to what it
  looks up: 3a's first shape returned a 64-byte settled enum on every send
  and cost 5.5–22.4% ([execution.md](design/execution.md), "What primitives
  as method-table entries moved"), and in 3b splitting the decision into
  `resolve` and `act`, then the caches' hit path, each pushed `vm::run`'s
  per-instruction step out of line, where every instruction pays a call and
  returns its `Flow` through memory — 6.6–13.6% on every source. The step is
  now forced inline (`#[inline(always)]`), which on 3a's code measured no
  faster by itself, and `decide_super_send` is kept out of it
  ([execution.md](design/execution.md), "What the inline caches moved";
  [dispatch.md](design/dispatch.md), "A standing hazard"). Anything added to
  a send's hit path lands in the step: check it with `dumpbin /disasm` (from
  the Visual Studio Build Tools, not on `PATH`) on a release build with debug
  information — no `vm::run::closure` may be a function of its own — before
  timing it. The structural answer is planned for later (the user's decision,
  2026-09-17): one `Interpreter::enter` per stretch between two safepoints
  rather than one per instruction, so that no per-instruction boundary
  exists; to be looked at before a send's hit path grows further, and at the
  latest with the JIT tiers.
- **A later option for private lexical sites: guard on N's mixin.** 3b
  guards a private site on N's application, so a site goes polymorphic when
  its mixin is applied under several superclasses. A per-heap mixin id on the
  application, with a mixin-relative slot index, would keep it monomorphic
  at a second load per private hit ([dispatch.md](design/dispatch.md),
  "Decisions"). None of the measured sources has such a site; measure it
  against the built guard once a program does.
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
  Under the pin 2c is level with `a5d1e05` on every source in
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
  for 2c. 2c, measured under the pin, left `bench_send.ns` +3.1% and +2.2%
  and `bench_loop.ns` +1.7% above `be3eb40` ([execution.md](design/execution.md),
  "What 2c moved"). Spec-conformant lookup closed both: `bench_send.ns` is
  10.4% and 10.1% below `be3eb40`, `bench_loop.ns` level with it (+0.3% and
  +0.8%, overlapping), and every other source 6.2–9.9% below it
  ([execution.md](design/execution.md), "What spec-conformant lookup
  moved"). The rule stays:
  **every performance measurement up to and including step 3 compares
  against `be3eb40` as well as against the commit it starts from**, reports
  both, and builds every binary with the same codegen flags (the finding
  above). 3a closed the rest: `bench_loop.ns` stays level with `be3eb40`
  (+1.15%/+1.56%, overlapping) and the other six sources are 7–10% below it
  ([execution.md](design/execution.md), "What primitives as method-table
  entries moved"). 3b left every source 9.4–19.7% below `be3eb40`,
  `bench_loop.ns` included, entirely below in both sittings
  ([execution.md](design/execution.md), "What the inline caches moved"). The
  step before 3c gave some of that back — `bench_loop.ns` and
  `bench_send.ns` 2.2–3.6% slower than the commit it started from, entirely
  above in both sittings — and every source is still 6.7–18.2% below
  `be3eb40`, entirely below in both
  ([execution.md](design/execution.md), "What the step before 3c moved").
  3c left every source 8.3–19.4% below `be3eb40`, entirely below in both
  sittings ([execution.md](design/execution.md), "What 3c moved"). None is
  slower. The specification gaps' plan carried `be3eb40` too, one binary
  more, and left every source 7.6–18.2% below it, entirely below in both
  sittings ([execution.md](design/execution.md), "What the specification
  gaps moved"). Whether a measurement past step 3 still carries `be3eb40` is
  for its own plan to say.
- **A send's hit path pays about 2% for every component its key gains.** The
  step before 3c cost `bench_loop.ns` and `bench_send.ns` 2.1–3.3% and
  `bench_megamorphic.ns` about 1%, all of it on the lexical global-cache
  key's commit. A control binary — that commit with
  `send_lexical_first`'s two probes cut out and everything else kept, whose
  `vm::run` is instruction for instruction the commit's — settled which half:
  the control alone reproduces the loss, and adding the probes on top of it
  moves no source beyond the noise. So it is not work done per send; it is
  the eighteen instructions the key's fourth component adds to the step,
  which every send executes. Entry size is not it either — `GlobalEntry`
  stayed 32 bytes and `CacheEntry` 24 throughout. It is the going rate for
  widening a send's hit path: a change that adds work per send should be
  expected to show, not assumed to be free. 3c changed what every guard
  *reads* rather than what the key carries, and measured level
  ([execution.md](design/execution.md), "Which half of the lexical key costs
  the send benchmarks").
- **`bench_do.ns` moves without a cause, twice in one step.** 2.8% / 3.3%
  slower on the flat-metaclass and factory change, entirely above in both
  sittings, and most of it back on the least-recently-used groups — and it
  neither instantiates, nor sends to a class object, nor reaches a
  megamorphic site in its loop. This is the fourth recorded instance of a
  uniform shift on code a change does not execute, and the
  function-alignment pin, which removes a 64-byte phase shift, does not
  remove it. A fifth instance is worth one focused investigation rather than
  another footnote ([execution.md](design/execution.md), "What the step
  before 3c moved").
- **A decoded image's last initializer is not held to answering `self`**, so
  `new` answers whatever it answers — wrong but safe, no panic.
  `image::decode::verify_mixins` could refuse such an initializer, which would
  make "`new` answers the instance" hold for images too.
- **No test reaches a `^` that targets a superclass clause's own activation.**
  A clause's receiver is a single name, so only a crafted image can put a
  block in that body; the clause *message*'s case, a `^` caught by the
  message's activation, is pinned
  (`a_return_in_a_clause_messages_block_hands_its_value_to_the_walk`).
- **The stress suites take over four minutes together.** Under
  `NS_NEXT_STRESS_MOVES=1` the suite takes about 3 min 9 s, almost half of it
  one test, `the_depth_limit_is_exact_and_counts_alike_under_both_lowerings`;
  under `NS_NEXT_STRESS_CACHES=1`, about 1 min 5 s. Both are gates before
  every commit, so a plan should budget for them.
- **Small follow-ups 3b's reviews deferred**, none affecting an outcome:
  `InlineCaches::fresh_range` adds `start + size as usize` unchecked, which
  only a 32-bit `usize` could overflow; no unit test covers a decoded
  closure's base being written back
  or `fresh_range` refusing a range past the table's limit (an image test
  covers the first through a program); `types.rs` names
  `interpreter::inline_cache::FeedbackBase` rather than its re-export
  `interpreter::FeedbackBase`; `inline_cache::send_super` reads
  `current_class` and its superclass before it knows whether the send is
  cached, and `send_lexical` computes N twice on a private site's miss.
  Also open: whether the family map per push shows on the instantiation
  benchmarks, the condition under which a `Factory` entry would carry the
  bases of the bodies its instantiation runs — no task isolated the map's
  cost ([dispatch.md](design/dispatch.md), "Code families").
- **`ns-cli build`'s `root_class_answers` accepts a slot setter as an entry
  selector, although its own doc comment says it must not.** It checks
  `mixin.methods.get(&symbol)` for any entry that is not a
  `MethodEntry::Primitive` — which now also matches a
  `MethodEntry::SlotAccessor`, getter and setter alike, since 3a left slot
  accessors as they already were (method-table entries) but this check
  predates that and was never updated to tell the two accessor kinds apart.
  A build whose only entry selector is a setter passes the check and still
  starts and runs: `image::build` interns the entry selector itself
  (`crates/ns-runtime/src/image/mod.rs`, "The symbol table is not covered by
  that guarantee"), and a decoded mixin carries its slot accessors again. The
  consequence is the one `build.rs`'s own doc comment names — accepting a
  selector `mixins::encode` never interned makes `build` grow the symbol
  table to record it, breaking the byte-identity guarantee with `snapshot`
  that `root_class_answers` exists to preserve
  (`a_build_and_a_snapshot_agree_on_the_describing_sections`,
  `crates/ns-cli/tests/build_load.rs`). Pre-existing, untouched by this step;
  worth a small task (`crates/ns-cli/src/commands/build.rs`).
- **Golden proposals the coverage audits raised and nobody built.** The two
  audits of 2026-09-18 classified all 64 numbered sections and proposed more
  goldens than were worth adding; nine were built and about a dozen judged
  marginal, so that analysis survives only as a decision. Worth knowing
  before anyone audits the specification a third time: what was left out was
  §5.1.8's closure value rules, §5.1.5's doubled quote, §5.1.7's empty and
  immutable tuples, §5.1.4's character literals (the section's
  `Character codePoint:` equivalence is unimplementable -- no such selector),
  §6.3.4's binary message pattern, §6.3.1's unmodified *method* being
  protected, §4.3's inert metadata comments, §8.1's optional category string
  and §8.4's `args:` list. Each is either pure syntax the corpus already
  gates or a near-duplicate of a golden that exists. The sections judged
  untestable -- actors, mirrors, types, object literals, pattern literals --
  are untestable for reasons that will not change until those subsystems
  exist.
- **Three findings the review of the goldens branch carried.**
  `spec_6_3_factory_is_a_class_side_member` is the one golden under
  `goldens/spec/` with neither a rationale comment nor a citation.
  `spec_6_3_2_simultaneous_slots_resolve_mutual_recursion` raises at exit 1
  today, while the deviation it belongs to is described as answering wrongly
  at exit status 0 -- the general claim holds, that one program just does not
  demonstrate it, because its expression errors on the nil instead of
  computing with it. And
  `spec_6_3_3_a_lazy_slot_is_initialized_on_first_use.toml` records a
  trailing space, which is correct and which an editor that strips trailing
  whitespace would silently break.

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

**Thirteen pieces are done.** The heap seam routes every object and activation
access through `Heap` (`crates/ns-runtime/src/heap.rs`), and the arena behind
it replaced `Rc<RefCell<_>>`: an `ObjectReference` or a `FrameReference` is a 32-bit index
into a `Vec` the `Heap` owns, with a private field so the compiler enforces the
seam. That is worth ~15% of the benchmark and is described in
[memory.md](design/memory.md); see "M4 — The heap seam" and "M4 — The object
and frame arena" below. Then activations moved onto a stack the `Heap` owns
(step 1a below), reified into the frame arena only when a block captures one;
see "M4 — The activation stack" below. **Nothing reclaimed an arena slot
until the collector** — every object and every reified frame was kept for
the life of the process, a deliberate interim cost the activation stack
bounded for activations nothing captures. Step 5, the moving collector, ends
it: a slot is reclaimed once nothing reaches it (see "M4 — The moving
collector" below). Then step A
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
"M4 — The lifetime brand" below. Then name resolution and lookup were made to
follow the specification, slot accessors became method-table entries, and the
method and slot tables got an identity hasher; see "M4 — Spec-conformant
lookup" below. Then 3a made every builtin's native an entry of its class's
method table too, carrying an enumerated primitive instead of a selector
matched as a string; see "M4 — Primitives as method-table entries" below.
Then 3b put an inline cache in front of every send's lookup; see "M4 —
Inline caches" below.

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
`self tick: sum` — interned its getter name on every send to look for a slot,
and the method and slot tables hashed `Symbol` keys with SipHash.
Spec-conformant lookup took both out: a slot's accessors are method-table
entries, so no send interns a name, and the tables hash identity keys with
one multiply; `bench_send.ns` is 12.5% faster for it.

The list below is M4's order of work — 1a done, A to C inserted ahead of 1b
and all three done, 1b done, 2 done in all three parts, the spec-conformant
lookup inserted ahead of 3 and done, 3 done in all three parts, the
specification gaps done, removing `:=` done as its own small step ahead of
4, 4 done, 5 done — and it is
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
3. **Inline caches** — [dispatch.md](design/dispatch.md). **Ahead of them, done:
   spec-conformant lookup**, which made the lookups the caches record follow
   §5.5–§5.10 and §6.3.2, put slot accessors into the method table, where a
   cache entry records them like any member, and gave the tables an identity
   hasher; see "M4 — Spec-conformant lookup" below. Designed on 2026-09-15 in
   three parts, 3a to 3c ("Where the next session starts" above).
   **3a, primitives are method-table entries, is done**: a native entry
   carries an enumerated primitive, not a selector or a function pointer, so a
   hit will cost the guard and an indexed jump and a later JIT tier can emit
   the primitive inline ([dispatch.md](design/dispatch.md), "Primitives");
   see "M4 — Primitives as method-table entries" below. **3b, the caches, is
   done**: a cache guards on the **mixin application**, not on the class
   object, whose obvious key would never hit on a nested class; see "M4 —
   Inline caches" below. **3c, the dispatch id's place, is done**: a guard
   reads the id from the object header; see "M4 — The dispatch id's place"
   below. **The specification gaps are done**, one plan and one measurement;
   see "M4 — The specification gaps" below. **Removing `:=`** is done, its
   own small step before step 4; see "M4 — Removing `:=`" below.
4. **Closure conversion. Done.** [execution.md](design/execution.md),
   "Closure conversion". Blocks copy the
   variables they only read and share their scope's indirection vector for the
   ones they write, instead of reaching them through their method's frame
   (designed 2026-09-19, built 2026-09-20). That ended 1a's divorce copy for
   optimized code — whether a returning activation copies its registers out is
   decided per activation, from the block's `outer_reach` — and makes a
   surviving block retain what it captured plus its creator's frame
   record without registers, which is why it came
   directly before the collector. Every Smalltalk and Newspeak VM that moved to
   stack frames made this change to do it; ns-next made it second, because the
   reification mechanism is needed with or without it. Level with `efac7d2` on
   all eight sources; `bench_block.ns`'s peak working set 2.6% higher; see
   "M4 — Closure conversion" below.
5. **A moving collector. Done** (2026-09-20) — [memory.md](design/memory.md).
   Mark-compact over the index arena, one space, stop-the-world, with
   generations as a later step. `become:` and instance migration rule out a
   conservative non-moving design, and pinning whatever a conservative stack
   scan finds was considered and rejected for the same reason. This ends the
   arena's never-reclaim interim — the one the closure conversion measurement
   ran into, where an indirection vector per block activation showed as peak
   working set because nothing gave it back. `bench_block.ns`'s peak working
   set under `bench --iterations 10` fell from 228.8 MB to 5.6 MB; every
   source but `bench_block.ns` itself stayed level with the baseline, and
   `bench_block.ns` is 12.2% slower, the honest cost of the collections it
   alone runs; see "M4 — The moving collector" below.

Then, after M4 and before the JIT tiers:

6. **The core library in Newspeak, over a VM mirror** —
   [core-library.md](design/core-library.md). Every builtin method — `do:`,
   `whileTrue:`, `collect:` and the bodies Rust reimplements today, and the
   kernel classes with them — becomes Newspeak source in a `Kernel.ns`, the
   way SOMns's kernel is; the leaves (`+`, `<`, `at:put:`, `size`, `show:`)
   stay Rust primitives, but as entries of **one VM mirror object** that
   `Kernel.ns` is handed, which §8.3.1 requires ("A primitive call is a
   message send to the VM mirror") and which makes the Rust boundary a
   capability rather than something any method can claim. The inlined fast
   path for leaves is sequenced first, because without it every leaf send
   costs an activation. Decided with the user on 2026-09-21; a leaf is what a
   Rust primitive is good
   for and the artifact-size and trust arguments still hold for it.
   **That first part is done** (2026-09-21): the nine binary integer
   primitives compile to `Instruction::IntegerBinary` under a run-time type
   guard whose fallback is the ordinary send, and all eight measurement
   sources came out 11.5–31.0% faster for it, each entirely below its
   baseline group in both sittings — the cushion the rest of this step
   spends. See "Leaf inlining" below. It is
   required so that every builtin method is a real method lookup, mirrors, the
   debugger and a JIT see: a `do:` in Rust is opaque to a JIT, which can
   neither inline its block nor compile the loop as a unit. Where it sits is
   fixed from both ends. Before the steps above it makes the numbers worse; a
   `do:` written in Newspeak pays for its block until the caches and a JIT can
   inline it. After them it costs nothing, and it must come before the tiers,
   which need the library as real Newspeak code to optimize at all. M4 owed it
   only that the rewrite not deepen the investment in Rust callbacks, and step
   2a took the last callback away, as a trampoline step rather than as
   Newspeak. What it still needs: the kernel classes loadable as Newspeak
   source without the bootstrap depending on it, and the leaves reachable as
   primitives from that source — which 3a prepared, a primitive being a method
   table entry carrying a number. **It also carries the rest of §3.2**
   (decided 2026-09-18): `Class`, `Behavior` and `Metaclass` become declared
   kernel classes, as they are in both references, and a class object's own
   class becomes its metaclass — the reified instance-of relation that
   mirrors need and that nothing before this step does
   ([object-model.md](design/object-model.md), "Class sides and
   metaclasses").

7. **The payload space** — [memory.md](design/memory.md), "Payloads belong to
   the Rust allocator, and what that costs". An object's variable-length part
   is a separate Rust allocation today: `Instance`'s and `Array`'s
   `Vec<Value>`, `ClosureData.copied`, and a reified frame's `arguments` and
   `temporaries`. Moving those four into one `Value` space the collector
   compacts beside the arenas makes an allocation a bump and reclamation free,
   and lays a payload next to its object, which nothing does today. Decided
   with the user on 2026-09-21 to sit here: **after step 6, before the tiers.**

   Step 6 removes the one constraint that would complicate it. `Heap::array_push`
   grows a payload in place only because `OrderedCollection` is a Rust builtin
   (`builtins/collections.rs`'s own design note calls that a deliberate
   simplification); both reference runtimes implement a growable collection in
   Newspeak over a **fixed** `Array` — Primordial Soup's `List`
   (`prior-art/primordialsoup/newspeak/Collections.ns`) and SOMns's `Vector`
   (`prior-art/somns/core-lib/Kernel.ns`) each allocate a larger `Array` and
   copy on overflow, as `java.util.ArrayList` does, and neither VM has a
   primitive that resizes a payload. Step 6 also makes the space worth more: a
   `List>>add:` written in Newspeak allocates a fresh `Array` and payload at
   every capacity overflow, where a `Vec::push` amortizes to nothing.

   What the space must still support after step 6 is reflective, not hot:
   edit-and-continue adding locals to a live activation, and a MirrorBuilder
   adding or removing slots of live instances (requirements 2 and 3 of
   [liveness-and-reflection.md](design/liveness-and-reflection.md)). Each takes
   a fresh region and orphans the old one, so the space needs a fill-level
   trigger of its own beside the two arena limits. `become:` exchanges the
   payload reference along with the rest of the object, and the space belongs
   to a `Heap` — never to a thread or the process — because an actor is meant
   to be a per-heap scope.

   **Measure before building.** The three experiments named under "Where the
   next session starts" above still apply, and a fourth is cheaper than any of
   them: a `#[global_allocator]` swap in `ns-cli` says whether payload
   allocation costs measurable time at all. It is needed because the
   allocator-churn reading of `bench_block.ns`'s unexplained remainder is
   weaker than it looks — against `dc0ffe5` the head allocates exactly as many
   payloads as the baseline did, since the baseline reclaimed none; what the
   collector added is the frees, and `compact_arena`'s `truncate` drops those
   inside the span `World::collect` times.

What M4 unlocks beyond speed is recorded where it applies: suspending a running
computation and serializing it needed the trampoline and the removal of the
re-entries from Rust, both built, and still needs a yield path out of the run
loop and a re-entry point for a decoded frame
([image-format.md](design/image-format.md), "Planned: suspend and resume"),
and so does the debugger; `become:`-based class mutation can now build on the
collector's object forwarding, which it needed and did not have before step 5
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
  application's `slot_index` for every lexical send. Spec-conformant lookup
  removed both: a slot's accessors are method-table entries, and the tables
  hash identity keys with one multiply ("M4 — Spec-conformant lookup" below).

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
6. **Lazy slots stay out of scope** (§6.3.3); the parser drops `lazy`.
7. **The megamorphic fallback and an application id in the object header
   are open questions for step 3**, not part of this step.

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
`Lookup::Primitive` match it directly instead (`8a05621`). 3b rebuilds this
same decision path and should not repeat that shape; carried forward as an
open finding above.

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
inlining. Replaced by decisions still to be built: a vacant lexical site
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

**The open finding that a lexical send's decision is sensitive to codegen is
closed.** Spec-conformant lookup had measured a variant that changed only
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
a description into a work list: nine goldens are skipped, six for gaps the
table already recorded and three for rules nothing had pinned.

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
   still points at `Object`'s behaviour, which stands in for the `Class` §3.2
   makes a metaclass's superclass, so a class-side `super` reaches `Object`
   and a class-side `super new` no longer instantiates. Two recorded outcomes
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
(the open finding above).

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
   `printString`, because `nil printString` answers `'an Object'`
   (`Object>>printString` is a pre-existing stub, out of scope here) and
   `nil isNil` is itself a skipped gap.

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

**Left, none affecting an outcome:** the golden comment "order collected"
and dispatch.md's "what a namesake declares" name the member-set fix's
second axis differently (Task 1); README.md's forward-reference paragraph
also had its count corrected, beyond the brief's literal wording, signed off
by the controller (Task 3); the image round-trip test's `storedSymbol` name
collides with its own held accessors, explained in its own comment (Task 6);
`Heap::traced_size` counts the canonical symbol table's empty slots too —
its length follows the process-wide interner, not this heap's own symbols —
irrelevant to the stress mode's pacing, which only needs an upper bound, and
worth revisiting only if many heaps live in one process (actors). `nil
printString` answering `'an Object'` and `:=` not being Newspeak syntax are
open findings, not follow-ups of this step's own code; both are recorded in
"Where the next session starts" above.

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
comparison binaries. Step 4 of the order of work above; the design is
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

**Left, none affecting an outcome:** `capture.rs`'s `Captures::keys` exists
for tests only (`#[cfg(test)]`); `closure_activation` answers a
parameter count that is now just its body's own `argument_count`, which its
caller already holds; `closure_home` does not check the frame's
handle origin as its neighbours do; `inspect.rs`'s `read_closure_record`
bounds its count with `min(1024)` rather than `reserve_for`; a fallback
closure reloads its scope's vector on every access; one slot-initializer test
traded its exact-instruction assertion for three targeted ones when the
program it lowers grew a second statement; the committed `insta` snapshot
carries an `assertion_line` key; and no fixture exercises the one silent form
of a wrong inlined-depth correction, where the frame it lands on declares a
local at the same index.

**Left by the branch's own reviews, worth knowing before the next change
here:** `outer_reach_of`'s `_ => 0` arm means the rule that every
by-reference instruction must be named there is carried by prose and by the
three places it is written down, not by the compiler — an instruction added
without it silently loses its frame's registers. `DecodeError::CodeArityMismatch`
(a different variant from the corrected `ClosureArityMismatch`) still
justifies itself by `read_local` indexing past the end, which
`Heap::frame_local` no longer does. And three citations of "Converted
representation" are split across a line break, so grepping the workspace for
that section's name misses them.

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
the head adds per round; the rest is recorded as unexplained rather than
guessed at. See [execution.md](design/execution.md), "What the moving
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
`master`, on the checked-out tree throughout. The first part of step 6 of
the order of work above, and sequenced ahead of the rest of it
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
  division on the fast path, and two under `goldens/errors/` pinning that an
  `i64` overflow and a zero divisor still carry the primitive's own message.
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

**A known cost, carried forward, not paid.** A successful operation also
executes the unconditional `Jump` past the fallback, so the hot path is two
dispatches where an out-of-line fallback would leave one. It does not show
above the noise — `bench_loop.ns` removed 33% of its per-iteration sends and
gained 31.0%, which is the tightest the suite gets — so moving the fallback
out of line is a follow-up worth trying rather than one these numbers
demand.

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
