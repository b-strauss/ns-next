# Inline Caches (Step 3b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every send the optimized lowering compiles probes a cache at its own
call site, then a global cache, and runs the full lookup only when both miss,
so that a hit costs a guard compare instead of a method-table walk, and each
site keeps the receiver-type profile the later optimizing tiers will read.

**Architecture:** The cache state lives per heap, in a table the `Mutator`
owns (`InlineCaches`), never in the shared `CompiledCode`. Each running
activation carries `feedback_base`, the start of its code family's range in
that table, and a site's entry is `sites[feedback_base + call_site]`. A site
goes vacant → monomorphic → polymorphic (a pooled block of 4) → megamorphic,
with a 1024-entry global cache behind every site. The guard is a dense
per-heap `DispatchId`, two per mixin application (instances, class side). The
decision path is split into `resolve` (today's lookups, unchanged in order and
rules, answering a `Resolved`) and `act` (turning a `Resolved` into the
`Step`), so the uncached path is literally `act(resolve(…))` and a hit is
`act(materialize(target))`: a cache records what the lookup decided and never
decides. The reference lowering emits every send uncached and is the oracle.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency. `samply` 0.13.1 (already installed) for the opening profile.

**Spec:** `docs/design/dispatch.md`, "Planned: inline caches" → "3b — The
caches" (the authority for this plan), with `docs/design/compiler.md`
("Safepoints and call-site identity"), `docs/design/image-format.md` ("The
instruction encoding", "Verification"), `docs/design/execution.md` (the
measurement protocol in "What removing the re-entries moved", and "What
primitives as method-table entries moved") and `docs/design/memory.md`
("What the collector traces", "The stress mode").

## Global Constraints

- Work on a branch `feature/inline-caches` created from `master`, on the
  checked-out tree — no `git worktree` for the work itself (throwaway
  worktrees for the measurement's before-binaries are fine and are removed
  afterwards). This plan is the branch's first commit.
- Before Task 1, record `git rev-parse HEAD` of `master` as **START**
  (`49a2a38` when the plan was written; its code is 3a's head). Every
  measurement compares against START and against `be3eb40`.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or
  two characters, no truncated word, in code, tests and commit messages alike.
  Clippy's `min_ident_chars` refuses the short ones.
- No dispatch logic in `vm.rs` or `compiler/`. `vm.rs` passes the running
  activation's `feedback_base`, the running code's `family_size` and the
  instruction's `CallSite` to `interpreter/inline_cache.rs` and does nothing
  else with them.
- **No new error text, and no changed outcome.** Every error still comes from
  today's paths. Every `tests-fixtures/*.ns`, every golden in
  `program_outcomes.rs` and `run-corpus` keep their recorded answers; a hit
  must produce exactly the step, value or signal the uncached path would.
- **Never `{:?}` a `Value`**, never `.unwrap()`/`.expect()` an
  `EvaluationResult` or `HostResult`; tests use a local `expect_ok` as the
  neighbouring test files do.
- An image is untrusted input: no byte sequence may panic, in a debug build
  either. A crafted artifact may get wrong-but-safe behaviour from a cache,
  never a panic and never an out-of-range index.
- A wire tag is assigned once. The four send tags 7–10 are retired, never
  reused; the sends take 21–24; the next free number becomes 25.
- Code that implements a spec rule cites it (`newspeak-spec.pdf` §N.N in the
  doc comment), the house style of `dispatch.rs` and `interpreter/`.
- Comments explain the decision, not the mechanics.
- Every commit passes `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, and `cargo test --workspace`. Before the last
  commit of every task also run, in Git Bash:
  `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` (about three minutes),
  `cargo run -p ns-cli -- parse-corpus corpus/modules` (must report
  `parsed 210, failed 0, expected-fail 1, stale 0`),
  `cargo run -p ns-cli -- run-corpus corpus/modules` and
  `NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules`
  (both must report `ran 6, failed 0, skipped 205`). From Task 8 on, also
  `NS_NEXT_STRESS_CACHES=1 cargo test --workspace` and
  `NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules`
  (same report).
- Conventional commits (`test:`, `refactor:`, `feat:`, `fix:`, `perf:`,
  `docs:`). **Every commit message ends with two trailer lines**, the first
  naming the model that actually wrote the commit's code (a subagent names its
  own model, not the controller's):

  ```
  Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_012MiKCA7qUwWdJ9QFfmzf8T
  ```

- Never override the git author or committer identity.
- Not in scope: 3c (the dispatch id in the object header); invalidation scoped
  to one application (the dependency keys of `liveness-and-reflection.md`);
  caching host sends (`Interpreter::send`) or sends a continuation decides
  (`mustBeBoolean`), which stay uncached; closure conversion.

## Where this plan refines the design

`dispatch.md`'s 3b section was written before the code was read line by line.
Five points need a sharper answer than it gives; Task 10 writes each one back
into the design document. **They are proposals of this plan, for the user to
confirm when approving it.**

1. **A private lexical site is guarded on N's application, not unguarded.**
   The design fixes a private target per site "with no guard". But a mixin
   applied under two superclasses gives N two applications with different
   inherited slots, so a private *slot* accessor's absolute field differs
   between them (`cache_slots_under_two_applications.ns`, Task 2, pins it).
   A private entry is therefore guarded on N's instance id
   (`Rule::LexicalPrivate`, key `(id(N), selector)`), and such a site can go
   polymorphic when N's mixin has several applications. Whether a site is
   private is still decided once, when it is first filled, and only chooses
   which id the hit path computes; the guard replaces the design's debug-build
   check that N is still the mixin the decision was made for, and unlike that
   check it cannot panic on a crafted image — a site whose N changed simply
   misses.
2. **A vacant lexical site skips the global cache.** Whether §5.9's private
   rule applies is not known until the site is first resolved, and a global
   entry under `(id(o), selector, Lexical)` may have been filled by a site
   whose N does not declare the selector private. So the first fill of a
   lexical site always runs `resolve`.
3. **A `super` send's `doesNotUnderstand:` target re-checks the factory, like
   its `Factory` target.** §5.10's failure path starts with the factory check
   on `self`, which the key (`current_class`'s application) does not cover:
   two class objects inheriting one class-side method can have different
   factory selectors (`cache_super_send_reaches_the_factory_check.ns`, Task 2,
   pins both orders).
4. **A block's `family_size` is its own subtree's, not the whole family's.**
   A block is compiled, and its `CompiledCode` finished, before the root that
   contains it; it records one past the highest call-site number among its
   own sends and its nested blocks'. That bounds every site it can run, which
   is all the check needs, and the decoder derives the same number the
   compiler does. The root's is the family's size.
5. **The decoder requires dense call-site numbers per family**
   (`DecodeError::CallSitesNotDense`): every number in a family distinct and
   below the root's `family_size`, and exactly `family_size` of them. That
   bounds the table a crafted family can allocate by its bytes, and keeps two
   sends from sharing one entry. With (1), `verify_mixins` needs no new check:
   no relation between code and mixin is assumed anywhere.

Two sizes will differ from the design's estimates and are reported, not
assumed: an entry holding an `Rc` is 24 bytes (a polymorphic block is 96, not
one 64-byte cache line), and a global entry is 32 bytes (about 32 KB, not
24). Task 9 measures a 16-byte entry that holds an index instead of an `Rc`.

## Shared definitions

Later tasks use these names exactly. Each is introduced by the task named
beside it.

```rust
// crates/ns-runtime/src/mixin.rs (Task 3)

/// The guard an inline cache compares: a dense number, minted per heap, that
/// names one mixin application's behaviour for its instances or for its
/// class objects (`docs/design/dispatch.md`, "The dispatch id").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchId(u32);

impl DispatchId {
    /// What `Heap::dispatch_id` answers for a value whose class has no
    /// application -- only a crafted image makes one -- and what an
    /// application built outside `apply_mixin` carries. Never cached.
    pub const NONE: DispatchId = DispatchId(u32::MAX);
    /// The guard of a cache entry that guards nothing: a vacant, polymorphic
    /// or megamorphic site. `Heap::dispatch_id` never answers it, so no
    /// receiver matches one.
    pub(crate) const VACANT: DispatchId = DispatchId(u32::MAX - 1);

    pub(crate) fn from_counter(value: u32) -> DispatchId {
        debug_assert!(value < DispatchId::VACANT.0);
        DispatchId(value)
    }
}

/// An application's two ids: one for its instances, one for its class
/// objects' class side, since every class object shares one placeholder
/// class and its behaviour depends on its application alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchIds {
    pub instances: DispatchId,
    pub class_side: DispatchId,
}

impl DispatchIds {
    pub const NONE: DispatchIds = DispatchIds {
        instances: DispatchId::NONE,
        class_side: DispatchId::NONE,
    };
}
```

```rust
// crates/ns-runtime/src/bytecode.rs (Task 4)

/// One send's identity within its code family (`docs/design/dispatch.md`,
/// "Code families"): its number among every send of one root body and the
/// blocks nested in it, dense from 0. `UNCACHED` is what the reference
/// lowering emits, so every send it compiles runs the full lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub number: u32,
}

impl CallSite {
    pub const UNCACHED: CallSite = CallSite { number: u32::MAX };
}

// `CompiledCode` loses `call_sites` and gains:
//     /// One past the highest call-site number among this body's own sends
//     /// and its nested blocks' `family_size`s; 0 when there is none.
//     pub family_size: u32,
```

```rust
// crates/ns-runtime/src/interpreter/inline_cache.rs (Task 5)

/// Where an activation's code family starts in its heap's cache table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackBase(u32);

impl FeedbackBase {
    /// A closure decoded from an image, or a method step the uncached path
    /// decided: the range is found or assigned when the activation is pushed.
    pub const NOT_ASSIGNED: FeedbackBase = FeedbackBase(u32::MAX);
}
```

```rust
// crates/ns-runtime/src/interpreter/resolve.rs (Task 6)

/// Which lookup found a target -- and so which class object its levels are
/// counted from, and where a failed primitive continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chain {
    /// §5.5's lookupPublic, from the receiver's class.
    Public,
    /// §5.5 on the receiver's metaclass (§6.3); the receiver is a class object.
    PublicClassSide,
    /// §5.9's lookupProtected, from the class of `o`.
    Protected,
    /// §5.9 on `o`'s metaclass.
    ProtectedClassSide,
    /// §5.9's private rule: N's own member, N itself the level.
    Private,
    /// §5.10: lookupProtected from S, the superclass of `current_class`.
    Super,
    /// §5.5's `doesNotUnderstand:`, defined for the receiver's class.
    DoesNotUnderstand,
    /// §5.10's `doesNotUnderstand:`, defined for S.
    SuperDoesNotUnderstand,
}

/// What a send resolved to, with its defining class as a handle: the form
/// `act` executes, uncached or materialized from a cache entry.
pub(crate) enum Resolved<'gc> {
    Method { method: Rc<MixinMethod>, defining_class: ClassReference<'gc>, chain: Chain },
    Slot { kind: AccessorKind, field: usize, chain: Chain },
    NestedClass { mixin: Rc<Mixin>, declaring_class: ClassReference<'gc>, chain: Chain },
    Primitive { primitive: Primitive, level: ClassReference<'gc>, chain: Chain },
    /// `try_factory_send` answers the send (§6.1); `chain` is `Public` or
    /// `SuperDoesNotUnderstand`.
    Factory { chain: Chain },
    /// Nothing handles it, not even `doesNotUnderstand:`. Never recorded.
    Unhandled,
}
```

```rust
// crates/ns-runtime/src/interpreter/inline_cache.rs (Task 7)

/// A cache entry's target: a `Resolved` with its defining class replaced by
/// the number of superclass steps from the chain's start, so that it holds no
/// handle. `Vacant`, `Polymorphic` and `Megamorphic` are site states, guarded
/// by `DispatchId::VACANT`.
#[derive(Clone)]
pub(crate) enum Target {
    Vacant,
    Method { method: Rc<MixinMethod>, feedback_base: FeedbackBase, level: u16, chain: Chain },
    Slot { field: u32, kind: AccessorKind, chain: Chain },
    NestedClass { mixin: Rc<Mixin>, level: u16, chain: Chain },
    Primitive { primitive: Primitive, level: u16, chain: Chain },
    Factory { chain: Chain },
    Polymorphic { block: u32, private: bool },
    Megamorphic { private: bool },
}

#[derive(Clone)]
pub(crate) struct CacheEntry {
    pub(crate) guard: DispatchId,
    pub(crate) target: Target,
}

/// The rule a global-cache key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CacheRule {
    Public,
    Lexical,
    LexicalPrivate,
    Super,
}
```

## Measurement protocol

Used by every task that says "measure". Every compared binary is measured
**in one sitting**; a figure from another session is never compared with one
from this.

The sources: the three fixtures `tests-fixtures\bench_loop.ns`,
`bench_send.ns` and `bench_block.ns`, and the four throwaway sources under
`target\bench-lookup\` (ignored by git): `bench_do.ns` (prints `275000`),
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` (each prints `20000`).
They exist on the machine the plan was written on. If one is missing, write
it from `docs/design/execution.md`, "What spec-conformant lookup moved"
(`bench_new*`, with the renamed slots) and the 2c plan's "Measurement
protocol" (`bench_do.ns`), tab-indented. Check that each prints what it
should before timing it.

Every binary — the current tree included — is built with the protocol's
flags, in PowerShell:

```powershell
git worktree add ..\ns-next-base be3eb40
git worktree add ..\ns-next-start <START>
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --release -p ns-cli --manifest-path ..\ns-next-base\Cargo.toml
cargo build --release -p ns-cli --manifest-path ..\ns-next-start\Cargo.toml
cargo build --release -p ns-cli
Remove-Item Env:RUSTFLAGS
dumpbin /dependents ..\ns-next-base\target\release\ns-next.exe   # no VCRUNTIME140.dll
$bins = [ordered]@{
  base  = "..\ns-next-base\target\release\ns-next.exe"
  start = "..\ns-next-start\target\release\ns-next.exe"
  after = ".\target\release\ns-next.exe"
}
$sources = "tests-fixtures\bench_loop.ns", "tests-fixtures\bench_send.ns",
           "tests-fixtures\bench_block.ns", "target\bench-lookup\bench_do.ns",
           "target\bench-lookup\bench_new.ns", "target\bench-lookup\bench_new2.ns",
           "target\bench-lookup\bench_newarg.ns"
foreach ($round in 1..5) {
  $names = @($bins.Keys)
  $order = $names[($round % $names.Count)..($names.Count - 1)] + $names[0..(($round % $names.Count) - 1)]
  foreach ($source in $sources) {
    foreach ($name in $order) {
      "$round $name $source"
      & $bins[$name] bench $source --iterations 30   # prints median, min, max
    }
  }
}
git worktree remove ..\ns-next-base
git worktree remove ..\ns-next-start
```

(For round 5 with three binaries `$round % 3` is 2; the expression rotates the
order each round. If PowerShell's range expression yields an empty slice at
`$round % 3 == 0`, use `$order = $names` for that round.) A task that compares
against its own parent commit instead of START adds that commit as a fourth
worktree and binary.

Record the five medians per source and binary, and each group's mean. The
gate: a change is **no slower** when, for every source, the after-group does
not lie entirely above the group it is compared with, and **faster** when it
lies entirely below. The comparison with `be3eb40` is reported, not gated.
Two sittings are two complete passes of the loop, one after the other, with
nothing else running on the machine. The release-only size pins run with
`cargo test --release -p ns-runtime --lib stays_the_size`.

**The shape to watch for** (`execution.md`, "What the next step must not
repeat"): 3a's first decision path cost 5.5–22.4% because every send returned
a 64-byte enum through memory. If a task in this plan measures a source
entirely above its comparison, look first at what the send path now returns
and through how many function boundaries, before anything else: keep `act`
`#[inline(always)]` into its callers, keep the cold continuations out of line,
and never widen what the hot path hands back.

---

### Task 1: Profile 3a's head

The ceiling the caches can reach, measured before anything is built: a cache
removes the lookup and nothing else.

**Files:**
- Modify: `docs/design/execution.md` ("What the profiles show")
- Modify: `docs/PROGRESS.md` (the "next piece is 3b's plan" paragraph)

**Interfaces:** Consumes nothing; produces the profile table later tasks and
the final report cite.

- [ ] **Step 1: Build a release binary with symbols and the protocol's flags**

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
$env:CARGO_PROFILE_RELEASE_DEBUG = "true"
cargo build --release -p ns-cli
Remove-Item Env:RUSTFLAGS
Remove-Item Env:CARGO_PROFILE_RELEASE_DEBUG
```

- [ ] **Step 2: Record the seven sources**

`samply` records through ETW and needs an elevated terminal: ask the user to
run, from the repository root, for each of the seven sources of the
measurement protocol (500 iterations, as the 2026-09-12 profile did):

```powershell
samply record --save-only -o target\profile-3a\bench_loop.json.gz target\release\ns-next.exe bench tests-fixtures\bench_loop.ns --iterations 500
```

and the same for `bench_send`, `bench_block`, `bench_do`, `bench_new`,
`bench_new2` and `bench_newarg`. If `samply` cannot record, use Windows
Performance Recorder (`wpr -start CPU`, `wpr -stop target\profile-3a\<name>.etl`)
and Windows Performance Analyzer, with `target\release` on the symbol path.
If neither works, stop and record in PROGRESS.md that the profile could not be
taken and why; do not guess an attribution.

- [ ] **Step 3: Read the profiles**

`samply load <file>` opens the viewer. Count only samples under
`timed_send` (parsing, compiling and the heap drop at exit also land in a
profile). If ns-next's own frames show as addresses, point the viewer's symbol
settings at `target\release\ns_next.pdb`; if that fails, use WPA as above.
For each source, read the **inclusive** share of the timed samples of:

- deciding a send: `Mutator::decide_send`, `sends::decide_lexical_send`,
  `sends::decide_super_send`, and inside them `dispatch::lookup_public`,
  `lookup_protected`, `private_member`, `metaclass_of`;
- `builtins::execute` (the primitives themselves, which a cache does not
  remove);
- `Mutator::push_step` and `Heap::push_activation`;
- `Heap::pop_activation` and `Heap::reify`;
- `vm::run`'s own (self) time.

The lookup share — deciding minus what it calls that is not a lookup
(`builtins::execute`, `decide_slot_accessor`, `nested_class`,
`decide_instantiation`) — is the caches' ceiling for that source.

- [ ] **Step 4: Record the finding**

Append to `execution.md`'s "What the profiles show" a paragraph and a table
headed "3a's head, before the caches (2026-09-16)": the build (release,
debug info, the protocol's flags), the tool, 500 iterations, and per source
the shares above and the lookup ceiling. In `PROGRESS.md`, replace "It opens
with a fresh profile, taken against 3a's head rather than carried over from
before it, so that the caches' ceiling is measured on the send path they will
actually sit in front of" with one sentence saying the profile was taken and
where it is recorded. Commit:

```bash
git add docs/design/execution.md docs/PROGRESS.md
git commit -m "docs: profile 3a's head for the inline caches' ceiling" -m "<trailer lines>"
```

---

### Task 2: Pin the outcomes the caches must keep

Every scenario below was run on START under both lowerings before this plan
was written, and printed exactly what is recorded here. They are the programs
a wrong cache entry would change: each has at least one site that sees
receivers a cache must tell apart, or must not.

**Files:**
- Create: `tests-fixtures/cache_nested_classes_share_behaviour.ns` and `.expected`
- Create: `tests-fixtures/cache_polymorphic_sites.ns` and `.expected`
- Create: `tests-fixtures/cache_slots_under_two_applications.ns` and `.expected`
- Create: `tests-fixtures/cache_private_lexical_site.ns` and `.expected`
- Create: `tests-fixtures/cache_class_objects_and_instances_at_one_site.ns` and `.expected`
- Create: `tests-fixtures/cache_super_send_reaches_the_factory_check.ns` and `.expected`
- Modify: `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:** Consumes nothing; produces regression guards every later task
must keep green.

- [ ] **Step 1: Write the six fixtures.** Tab-indented, as the existing
  fixtures are; every `.expected` ends with a newline.

`tests-fixtures/cache_nested_classes_share_behaviour.ns` — two `Inner`
instances from different `Outer` instances share one application, and so one
monomorphic entry, while their class objects stay distinct (§6):

```
Newspeak3
'Test'
class CacheNestedClassesShareBehaviour usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Outer tag: label = Object new (| name = label. |) (
		public class Inner = Object new () (
			public describe = ( ^'inner of ', name )
		)
	)
	public main: platform args: args = (
		| inners |
		inners := {(Outer tag: 'a') Inner new. (Outer tag: 'b') Inner new. (Outer tag: 'c') Inner new}.
		inners do: [:each | transcript show: each describe; newLine].
		transcript show: ((Outer tag: 'a') Inner == (Outer tag: 'a') Inner) printString; newLine.
	)
) : ()
```

`.expected`:

```
inner of a
inner of b
inner of c
false
```

`tests-fixtures/cache_polymorphic_sites.ns` — one site that sees seven
receiver types (six classes and a block) three times over, so it goes
monomorphic, polymorphic and megamorphic; a bimorphic site on `true` and
`false`; and a site over immediates, a string, a symbol and an instance:

```
Newspeak3
'Test'
class CachePolymorphicSites usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class One = Object new () ( public value = ( ^1 ) )
	class Two = Object new () ( public value = ( ^2 ) )
	class Three = Object new () ( public value = ( ^3 ) )
	class Four = Object new () ( public value = ( ^4 ) )
	class Five = Object new () ( public value = ( ^5 ) )
	class Six = Object new () ( public value = ( ^6 ) )
	public main: platform args: args = (
		| sum all round |
		sum := 0.
		round := 0.
		all := {One new. Two new. Three new. Four new. Five new. Six new. [7]}.
		[round < 3] whileTrue: [
			all do: [:each | sum := sum + each value].
			round := round + 1].
		transcript show: sum printString; newLine.
		{true. false. true. 3 < 4. 4 < 3} do: [:each | transcript show: each not printString; show: ' '].
		transcript newLine.
		{1. 2.5. 'x'. #y. nil. true. One new} do: [:each | transcript show: each printString; show: ' '].
		transcript newLine.
	)
) : ()
```

`.expected` (two lines end in a space before the newline):

```
84
false true false false true 
1 2.5 'x' #y an Object true an Object 
```

`tests-fixtures/cache_slots_under_two_applications.ns` — one mixin, `Cell`,
applied under a one-slot and a three-slot superclass, so its own slots sit at
different absolute fields; a public slot read and written from outside, the
same slot through a lexical send and a `self` send, a private slot through a
lexical send (§5.9, the private rule), and `super` in the mixin reaching two
different superclasses:

```
Newspeak3
'Test'
class CacheSlotsUnderTwoApplications usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Narrow = Object new (| first ::= 'narrow a'. |) (
		public describe = ( ^'narrow' )
	)
	class Wide = Object new (| first ::= 'wide a'. second ::= 'wide b'. third ::= 'wide c'. |) (
		public describe = ( ^'wide ', first, ' ', third )
	)
	class Box on: base = Object new (| base = base. |) (
		public class Cell = base new (| public held ::= 'cell v'. private hidden ::= 'hidden'. |) (
			public describe = ( ^super describe, ' cell' )
			public both = ( ^held, ' / ', self held )
			public touch = ( hidden: hidden, '!'. ^hidden )
		)
	)
	public main: platform args: args = (
		| cells |
		cells := {(Box on: Narrow) Cell new. (Box on: Wide) Cell new. (Box on: Narrow) Cell new}.
		cells do: [:each | transcript show: each held; show: ' | '; show: each describe; newLine].
		cells do: [:each | each held: 'set ', each describe].
		cells do: [:each | transcript show: each both; show: ' | '; show: each touch; newLine].
		cells do: [:each | transcript show: each describe; show: ' | '; show: each touch; newLine].
	)
) : ()
```

`.expected`:

```
cell v | narrow cell
cell v | wide wide a wide c cell
cell v | narrow cell
set narrow cell / set narrow cell | hidden!
set wide wide a wide c cell / set wide wide a wide c cell | hidden!
set narrow cell / set narrow cell | hidden!
narrow cell | hidden!!
wide wide a wide c cell | hidden!!
narrow cell | hidden!!
```

`tests-fixtures/cache_private_lexical_site.ns` — a private lexical send whose
`o` changes object and class from send to send: §5.9 invokes N's private
method on every one of them, although `Sub` declares a public `secret` of its
own:

```
Newspeak3
'Test'
class CachePrivateLexicalSite usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Outer named: label = Object new (| name = label. |) (
		private secret = ( ^'private of ', name )
		public class Inner = Object new () (
			public reveal = ( ^secret )
		)
	)
	class Sub named: label = Outer named: label () (
		public secret = ( ^'public of sub ', name )
	)
	public main: platform args: args = (
		{Outer named: 'a'. Sub named: 'b'. Outer named: 'c'. Sub named: 'd'}
			do: [:each | transcript show: each Inner new reveal; newLine].
		transcript show: (Sub named: 'e') secret; newLine.
	)
) : ()
```

`.expected`:

```
private of a
private of b
private of c
private of d
public of sub e
```

`tests-fixtures/cache_class_objects_and_instances_at_one_site.ns` — one site
that sees a factory send, a class-side method and an instance method; one that
sees `Object`'s own `printString` on class objects, a class-side override and
an instance override:

```
Newspeak3
'Test'
class CacheClassObjectsAndInstancesAtOneSite usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Maker make = Object new () (
		public describe = ( ^'a made maker' )
	)
	class Plain = Object new () (
		public describe = ( ^'a plain' )
	) : (
		public make = ( ^Plain new )
	)
	class Custom = Object new () (
		public describe = ( ^'a custom' )
		public make = ( ^self )
		public printString = ( ^'a custom instance' )
	) : (
		public printString = ( ^'the custom class' )
	)
	public main: platform args: args = (
		| receivers |
		receivers := {Maker. Plain. Custom new. Maker. Plain. Custom new}.
		receivers do: [:each | transcript show: each make describe; newLine].
		{Plain. Custom. Custom new. Maker. Plain new. Plain. Custom}
			do: [:each | transcript show: each printString; newLine].
	)
) : ()
```

`.expected`:

```
a made maker
a plain
a custom
a made maker
a plain
a custom
an Object
the custom class
a custom instance
an Object
an Object
an Object
the custom class
```

`tests-fixtures/cache_super_send_reaches_the_factory_check.ns` — `super new`
in a class-side method `Keyed` inherits: nothing above answers `new`, so §5.10
falls to the factory check on `self`, which instantiates `Simple` (factory
`new`) and not `Keyed` (factory `with:`), whose send ends in the
`doesNotUnderstand:` defined for S. Both orders, so that neither a recorded
factory nor a recorded handler may answer the other receiver:

```
Newspeak3
'Test'
class CacheSuperSendReachesTheFactoryCheck usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
	class Base = Object new () (
	) : (
		public doesNotUnderstand: message = ( ^'base class side does not understand' )
	)
	class Simple = Base new () (
		public printString = ( ^'a simple' )
	) : (
		public build = ( ^super new )
	)
	class Keyed with: seed = Simple new () (
	)
	public main: platform args: args = (
		{Simple. Keyed. Simple. Keyed} do: [:each | transcript show: each build printString; newLine].
		{Keyed. Simple. Keyed. Simple} do: [:each | transcript show: each build printString; newLine].
	)
) : ()
```

`.expected`:

```
a simple
'base class side does not understand'
a simple
'base class side does not understand'
'base class side does not understand'
a simple
'base class side does not understand'
a simple
```

- [ ] **Step 2: Pin the two outcomes that end in an error.** Append to
  `crates/ns-cli/tests/program_outcomes.rs`:

```rust
/// One site that reaches a user `doesNotUnderstand:`, the setter of a
/// mutable slot named `doesNotUnderstand` (§6.3.2: a method like any other,
/// so it is the handler, and answers the receiver), and `Object`'s own
/// handler, which ends the run. A cache that recorded one handler must not
/// answer another receiver with it (`docs/design/dispatch.md`, "3b — The
/// caches").
#[test]
fn a_does_not_understand_site_with_three_kinds_of_handler() {
    assert_outcome(
        "cache_dnu_handlers",
        indoc! {"
            Newspeak3
            'Test'
            class CacheDoesNotUnderstandHandlers usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
                class Catcher = Object new () (
                    public doesNotUnderstand: message = ( ^'caught ', (message at: 1) printString )
                )
                class Recorder = Object new (| public doesNotUnderstand ::= nil. |) (
                )
                public main: platform args: args = (
                    | recorder |
                    recorder := Recorder new.
                    {Catcher new. recorder. Catcher new. recorder} do: [:each |
                        transcript show: (each zork == each) printString; newLine].
                    transcript show: (recorder doesNotUnderstand at: 1) printString; newLine.
                    transcript show: (recorder doesNotUnderstand at: 2) size printString; newLine.
                    {Catcher new. 3} do: [:each | transcript show: each zork printString; newLine].
                )
            ) : ()
        "},
        1,
        "false\ntrue\nfalse\ntrue\n#zork\n0\n'caught #zork'\n",
        "Uncaught Newspeak error: an object does not understand #zork\n",
    );
}

/// A primitive that fails where its entry was recorded: `=` and
/// `printString` on a user subclass of `String`, whose instances carry no
/// string, fall to `Object`'s; `show:` answers two transcript streams and
/// fails on the platform object, which shares their class, so the third send
/// at the same site -- the same dispatch id -- ends in `doesNotUnderstand:`
/// (`docs/design/dispatch.md`, "A primitive can fail").
#[test]
fn a_cached_primitive_that_fails_on_a_receiver_of_the_same_class() {
    assert_outcome(
        "cache_failing_primitive",
        indoc! {"
            Newspeak3
            'Test'
            class CacheFailingPrimitive usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
                class Text = String new () (
                )
                public main: platform args: args = (
                    | text |
                    text := Text new.
                    {'abc'. text. 'abc'. text} do: [:each |
                        transcript show: (each = each) printString; show: ' '; show: each printString; newLine].
                    {platform transcript. platform. platform transcript} do: [:each |
                        transcript show: (each = each) printString; newLine].
                    {'abc'. #(1 2)} do: [:each | transcript show: each size printString; newLine].
                    {platform transcript. platform transcript. platform} do: [:each | each show: 'shown'. each newLine].
                )
            ) : ()
        "},
        1,
        "true 'abc'\ntrue an Object\ntrue 'abc'\ntrue an Object\ntrue\ntrue\ntrue\n3\n2\nshown\nshown\n",
        "Uncaught Newspeak error: an object does not understand #show:\n",
    );
}
```

(The `indoc!` bodies are indented with spaces, as `CLAUDE.md` requires; the
fixtures above use tabs, as the existing fixtures do.)

- [ ] **Step 3: Run them; they must pass on START.**

Run: `cargo test -p ns-cli --test run_fixtures` and
`cargo test -p ns-cli --test program_outcomes cache`
Expected: PASS. If any fails, stop and report — the plan's expectations rest
on these.

- [ ] **Step 4: Gates and commit.**

```bash
git add tests-fixtures/cache_*.ns tests-fixtures/cache_*.expected crates/ns-cli/tests/program_outcomes.rs
git commit -m "test: pin the outcomes an inline cache could change" -m "<trailer lines>"
```

---

### Task 3: Dispatch ids

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`DispatchId`, `DispatchIds`, `MixinApplication.dispatch_ids`)
- Modify: `crates/ns-runtime/src/types.rs` (`ClassData.dispatch_ids`, `ClassData::new`)
- Modify: `crates/ns-runtime/src/heap.rs` (`dispatch_id`, `class_dispatch_ids`, the immediates' table, `set_class_application`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (the counter's initial value)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator.next_dispatch_id`, `apply_mixin`, `new_class_object`, `build_builtin_classes`, `build_self_referential_object_class`, `bootstrap_builtin_classes`)
- Modify: `crates/ns-runtime/src/trace.rs` (`ClassData`'s destructuring)
- Modify: every other `ClassData { .. }` and `MixinApplication { .. }` literal: `dispatch.rs` (tests), `heap.rs` (tests), `types.rs` (tests), `image/decode.rs`, `tests/image.rs`, `tests/vm_initializer.rs`
- Test: `crates/ns-runtime/src/heap.rs` (unit tests), `crates/ns-runtime/tests/heap.rs`

**Interfaces:**
- Produces: `DispatchId`, `DispatchIds` (Shared definitions);
  `MixinApplication.dispatch_ids: DispatchIds`; `ClassData.dispatch_ids: DispatchIds`;
  `ClassData::new(application, enclosing, declaring_class, superclass, builtin) -> ClassData` (copies the ids, `metaclass: None`);
  `Heap::dispatch_id(&self, value: &Value<'gc>) -> DispatchId`;
  `Heap::class_dispatch_ids(&self, class: ClassReference<'gc>) -> DispatchIds` (`NONE` for a non-class);
  `pub(crate) Heap::set_immediate_dispatch_ids(&mut self, ids: [DispatchId; 5])`;
  `pub(crate) Mutator::mint_dispatch_ids(&mut self) -> DispatchIds`.

- [ ] **Step 1: Write the failing tests** in `crates/ns-runtime/tests/heap.rs`
  (it already has `load` and `expect_ok` helpers; reuse them, or copy the
  pair from `tests/stress.rs` if not):

```rust
/// Two `Inner` class objects of two `Outer` instances share one application,
/// and so one dispatch id for their instances and one for their class side,
/// while the class objects themselves differ (`docs/design/dispatch.md`, "A
/// cache guards on the application, not on the class object").
#[test]
fn instances_of_a_nested_class_share_a_dispatch_id_across_enclosing_objects() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new (
        ) (
          public class Outer = Object new () (
            public class Inner = Object new () ()
          )
          public class Other = Object new () ()
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let root = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let mut make_inner = || {
        let outer_class = expect_ok(interpreter.send(&root, Symbol::intern("Outer"), &[]), "Outer");
        let outer = expect_ok(interpreter.send(&outer_class, Symbol::NEW, &[]), "Outer new");
        let inner_class = expect_ok(interpreter.send(&outer, Symbol::intern("Inner"), &[]), "Inner");
        let inner = expect_ok(interpreter.send(&inner_class, Symbol::NEW, &[]), "Inner new");
        (inner_class, inner)
    };
    let (first_class, first) = make_inner();
    let (second_class, second) = make_inner();
    let other_class = expect_ok(interpreter.send(&root, Symbol::intern("Other"), &[]), "Other");
    let other = expect_ok(interpreter.send(&other_class, Symbol::NEW, &[]), "Other new");
    interpreter.enter(|mutator| {
        let heap = mutator.heap();
        let id = |root: &Root| heap.dispatch_id(&mutator.fetch(root));
        assert_eq!(id(&first), id(&second), "one application, one instance id");
        assert_eq!(id(&first_class), id(&second_class), "one application, one class-side id");
        assert_ne!(id(&first), id(&first_class), "instances and class side differ");
        assert_ne!(id(&first), id(&other), "another application");
        assert_ne!(id(&first), DispatchId::NONE);
    });
}

/// Every immediate kind answers its builtin class's instance id, `true` and
/// `false` two different ones, since they are instances of two classes.
#[test]
fn immediates_answer_their_builtin_classes_instance_ids() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let expected = |mutator: &Mutator, kind: BuiltinClass| {
            mutator.heap().class_dispatch_ids(mutator.builtin_class(kind)).instances
        };
        let heap = mutator.heap();
        assert_eq!(heap.dispatch_id(&Value::Nil), expected(mutator, BuiltinClass::Nil));
        assert_eq!(heap.dispatch_id(&Value::Boolean(true)), expected(mutator, BuiltinClass::True));
        assert_eq!(heap.dispatch_id(&Value::Boolean(false)), expected(mutator, BuiltinClass::False));
        assert_eq!(heap.dispatch_id(&Value::SmallInteger(3)), expected(mutator, BuiltinClass::Integer));
        assert_eq!(heap.dispatch_id(&Value::Float(2.5)), expected(mutator, BuiltinClass::Float));
        assert_ne!(expected(mutator, BuiltinClass::True), expected(mutator, BuiltinClass::False));
        let array = mutator.new_array(vec![]);
        assert_eq!(
            mutator.heap().dispatch_id(&Value::Object(array)),
            expected(mutator, BuiltinClass::Array)
        );
    });
}
```

Add the imports
the tests need (`ns_runtime::{BuiltinClass, DispatchId, Mutator}`); Step 3
re-exports `DispatchId` and `DispatchIds` from `lib.rs` beside `Symbol`.

Also add a unit test at the end of `heap.rs`'s test module:

```rust
/// A class object whose data is not a class -- only a crafted image makes an
/// instance of one -- answers `NONE`, which nothing caches.
#[test]
fn an_instance_of_a_non_class_answers_no_dispatch_id() {
    let mut interpreter = crate::Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let not_a_class = mutator
            .heap
            .allocate_self_referential_class(ObjectData::Instance(vec![]));
        let instance = mutator.heap.allocate_object(not_a_class, ObjectData::Instance(vec![]));
        assert_eq!(
            mutator.heap.dispatch_id(&Value::Object(instance)),
            crate::mixin::DispatchId::NONE
        );
    });
}
```

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime --test heap dispatch_id`
Expected: FAIL to compile (`dispatch_id` not found).

- [ ] **Step 3: Implement.**

In `mixin.rs`, add the Shared definitions' `DispatchId` and `DispatchIds`,
and to `MixinApplication` the field

```rust
    /// The ids an inline cache guards on (`DispatchId`), minted by
    /// `Mutator::apply_mixin` when it creates the application, so an id never
    /// comes from an image's bytes. `DispatchIds::NONE` for an application
    /// built anywhere else, which only tests do.
    pub dispatch_ids: DispatchIds,
```

Re-export both from `lib.rs` next to the other `mixin` items (follow how
`lib.rs` already exports; if it does not export `mixin` items, add
`pub use mixin::{DispatchId, DispatchIds};`).

In `types.rs`, add to `ClassData`

```rust
    /// A copy of `application.dispatch_ids`, so that a cache's guard reads
    /// it from the class object without following the `Rc`
    /// (`Heap::dispatch_id`). Kept equal to the application's by
    /// `ClassData::new` and `Heap::set_class_application`, the only ways
    /// either changes.
    pub dispatch_ids: crate::mixin::DispatchIds,
```

and

```rust
impl<'gc> ClassData<'gc> {
    /// A class object's data around `application`, with no metaclass yet
    /// (`Mutator::metaclass_of` builds it on first use).
    pub fn new(
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
        superclass: Option<ClassReference<'gc>>,
        builtin: Option<BuiltinClass>,
    ) -> ClassData<'gc> {
        ClassData {
            dispatch_ids: application.dispatch_ids,
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin,
            metaclass: None,
        }
    }
}
```

Replace every `ClassData { .. }` literal under `crates/ns-runtime/src` with
`ClassData::new(...)` (`interpreter/mod.rs` three times, `image/decode.rs`,
and the test helpers in `dispatch.rs`, `heap.rs`, `types.rs`); the literals in
`tests/image.rs` and `tests/vm_initializer.rs` may do the same. Add
`dispatch_ids: DispatchIds::NONE` to every `MixinApplication { .. }` literal
outside `apply_mixin`. In `trace.rs`, add `dispatch_ids: _,` to `ClassData`'s
destructuring with the comment `// not a handle`.

In `heap.rs`: add to `Heap` the field

```rust
    /// The instance ids of the builtin classes of the immediates, indexed by
    /// `immediate_index`: an immediate has no class field to read one from.
    /// Set once by the bootstrap; `NONE` before it.
    immediate_dispatch_ids: [DispatchId; 5],
```

initialized to `[DispatchId::NONE; 5]` in `Heap::new`, and

```rust
/// Where `immediate_dispatch_ids` keeps an immediate's id.
fn immediate_index(value: &Value<'_>) -> usize {
    match value {
        Value::Nil => 0,
        Value::Boolean(false) => 1,
        Value::Boolean(true) => 2,
        Value::SmallInteger(_) => 3,
        Value::Float(_) => 4,
        Value::Object(_) => unreachable!("an object carries its own class"),
    }
}
```

and on `Heap`:

```rust
    /// The dispatch id an inline cache guards `value`'s send on
    /// (`docs/design/dispatch.md`, "The dispatch id"): an instance's class's
    /// instance id -- two arena loads -- a class object's own class-side id,
    /// and an immediate's from a table. Nothing outside this function knows
    /// where the id lives, which is what lets step 3c move it.
    #[inline(always)]
    pub fn dispatch_id(&self, value: &Value<'gc>) -> DispatchId {
        let Value::Object(object) = value else {
            return self.immediate_dispatch_ids[immediate_index(value)];
        };
        self.check_object_origin(*object);
        let entry = &self.objects[object.index as usize];
        if let ObjectData::Class(data) = &entry.data {
            return data.dispatch_ids.class_side;
        }
        self.check_object_origin(entry.class);
        match &self.objects[entry.class.index as usize].data {
            ObjectData::Class(data) => data.dispatch_ids.instances,
            _ => DispatchId::NONE,
        }
    }

    /// `class`'s ids, or `NONE` when it is not a class object: the key of a
    /// `super` send and of a private lexical send.
    pub fn class_dispatch_ids(&self, class: ClassReference<'gc>) -> DispatchIds {
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(data) => data.dispatch_ids,
            _ => DispatchIds::NONE,
        }
    }

    pub(crate) fn set_immediate_dispatch_ids(&mut self, ids: [DispatchId; 5]) {
        self.immediate_dispatch_ids = ids;
    }
```

(If `check_object_origin` has a different name for a `ClassReference`, use
the one `class_application` uses.) In `set_class_application`, also assign
`data.dispatch_ids = application.dispatch_ids;` before the application.

In `interpreter/mod.rs`: add to `Mutator`

```rust
    /// The next dispatch id `apply_mixin` mints. Past the last id below
    /// `DispatchId::VACANT` it stops minting and applications get `NONE`,
    /// which is never cached: slower, never wrong.
    pub(crate) next_dispatch_id: u32,
```

(initialized to 0 in `World::new`) and

```rust
    pub(crate) fn mint_dispatch_ids(&mut self) -> DispatchIds {
        let first = self.next_dispatch_id;
        let Some(after) = first.checked_add(2) else {
            return DispatchIds::NONE;
        };
        if after > u32::MAX - 1 {
            return DispatchIds::NONE;
        }
        self.next_dispatch_id = after;
        DispatchIds {
            instances: DispatchId::from_counter(first),
            class_side: DispatchId::from_counter(first + 1),
        }
    }
```

In `apply_mixin`, mint once per new application (after the cache miss, not
before it) and set `dispatch_ids` in the literal. In
`bootstrap_builtin_classes`, after `build_builtin_classes`:

```rust
        let instances = |mutator: &Mutator<'gc>, kind| {
            mutator.heap.class_dispatch_ids(mutator.builtin_class(kind)).instances
        };
        let ids = [
            instances(self, BuiltinClass::Nil),
            instances(self, BuiltinClass::False),
            instances(self, BuiltinClass::True),
            instances(self, BuiltinClass::Integer),
            instances(self, BuiltinClass::Float),
        ];
        self.heap.set_immediate_dispatch_ids(ids);
```

- [ ] **Step 4: Run the tests; they pass.**

Run: `cargo test -p ns-runtime --test heap dispatch_id` and
`cargo test -p ns-runtime --lib an_instance_of_a_non_class`
Expected: PASS. Then `cargo test --workspace`: nothing else changes.

- [ ] **Step 5: Gates and commit.** Run the gates, then measure nothing — no
  send reads the ids yet. Check `cargo test --release -p ns-runtime --lib
  stays_the_size` still passes (`ClassData` grew by 8 bytes; if the
  `Object`-size or amplification figures in `memory.md` are pinned by a test,
  update the figure and record the change in Task 10's list).

```bash
git add -A crates/ns-runtime
git commit -m "feat: mint a dispatch id per mixin application" -m "<trailer lines>"
```

---

### Task 4: Call sites numbered per family, and the wire change

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`CallSite`, `CompiledCode.family_size`, `family_size_of`, `disassemble`)
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (numbering)
- Modify: `crates/ns-runtime/src/compiler/reference.rs` (`UNCACHED`)
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (every `Lowerer::new` call)
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (test `CompiledCode` literals)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`, `tests/compiler.rs`, `tests/image.rs`
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)
- Modify: `crates/ns-cli/tests/disasm_snapshots.rs` snapshots if they print `@N`

**Interfaces:**
- Produces: `CallSite { number: u32 }`, `CallSite::UNCACHED`;
  `CompiledCode.family_size: u32` (and no `call_sites`);
  `pub(crate) fn family_size_of(instructions: &[Instruction]) -> u32` in `bytecode.rs`;
  `pub fn Instruction::call_site(&self) -> Option<CallSite>` in `bytecode.rs`;
  `OPCODE_SEND_LEXICAL = 21`, `OPCODE_SEND_PUBLIC = 22`, `OPCODE_SEND_INLINABLE = 23`,
  `OPCODE_SEND_SUPER = 24`, `RETIRED_OPCODES: [u8; 4] = [7, 8, 9, 10]`;
  `DecodeError::CallSiteListNotEmpty`, `DecodeError::CallSitesNotDense`.

- [ ] **Step 1: Write the failing tests.**

In `lower.rs`'s test module, beside `a_block_literal_compiles_to_its_own_code_carried_by_make_closure`:

```rust
/// A family's sends are numbered densely in the order they are emitted,
/// blocks included, and every body records one past the highest number in
/// itself and the blocks nested in it (`docs/design/dispatch.md`, "Code
/// families").
#[test]
fn a_family_numbers_its_sends_densely_across_nested_blocks() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new (
        ) (
          public run: items = (
            items size.
            items do: [:each | each printString. [:inner | inner foo] value: each].
            ^items isEmpty
          )
        ) : ()
    "};
    let code = lower_whole_method(source, 0);
    let numbers = |code: &CompiledCode| -> Vec<u32> {
        code.instructions
            .iter()
            .filter_map(Instruction::call_site)
            .map(|site| site.number)
            .collect()
    };
    let first_block = |code: &CompiledCode| -> Rc<CompiledCode> {
        code.instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::MakeClosure { code: block, .. } => Some(block.clone()),
                _ => None,
            })
            .expect("the body creates a block")
    };
    // Emission order: `size` 0; the `do:` block's `printString` 1; the inner
    // block's `foo` 2, lowered when its literal is reached; `value:` 3;
    // `do:` 4, after its block literal; `isEmpty` 5. Verified against
    // `ns-next disasm` on START when the plan was written.
    assert_eq!(numbers(&code), vec![0, 4, 5]);
    assert_eq!(code.family_size, 6);
    let block = first_block(&code);
    assert_eq!(numbers(&block), vec![1, 3]);
    assert_eq!(block.family_size, 4);
    let inner = first_block(&block);
    assert_eq!(numbers(&inner), vec![2]);
    assert_eq!(inner.family_size, 3);
}
```

`lower_whole_method(source, 0)` is the test module's existing helper; check
that index 0 names `run:` (the module declares one method).

In `tests/reference_lowering.rs` (which has `load(source, lowering)`; add
`CallSite` to its `bytecode` import):

```rust
/// The reference lowering is the uncached oracle: every send it emits is
/// `CallSite::UNCACHED`, and none of its bodies has a family.
#[test]
fn the_reference_lowering_emits_every_send_uncached() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new (
        ) (
          public run: items = (
            items do: [:each | each printString].
            ^items size
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source, Lowering::Reference);
    let code = interpreter.enter(|mutator| {
        let Value::Object(object) = mutator.fetch(&class) else {
            panic!("the module's class is a class object")
        };
        let application = mutator.heap().class_application(object).expect("a class");
        application
            .mixin
            .compiled_method(Symbol::intern("run:"))
            .expect("run: is compiled")
            .code
            .clone()
    });
    let mut pending = vec![code];
    let mut sends = 0;
    while let Some(body) = pending.pop() {
        assert_eq!(body.family_size, 0);
        for instruction in &body.instructions {
            if let Instruction::MakeClosure { code: block, .. } = instruction {
                pending.push(block.clone());
            }
            if let Some(site) = instruction.call_site() {
                assert_eq!(site, CallSite::UNCACHED);
                sends += 1;
            }
        }
    }
    assert_eq!(sends, 3, "do:, printString and size");
}
```

In `tests/opcode_tags.rs`, change the four send assertions to 21–24, add them
to `no_two_tags_share_a_number` unchanged in name, and add:

```rust
/// A retired tag stays retired: no live instruction takes its number
/// (`image/opcodes.rs`).
#[test]
fn retired_instruction_tags_are_never_reused() {
    assert_eq!(RETIRED_OPCODES, [7, 8, 9, 10]);
    let live = [
        OPCODE_LOAD_LITERAL, OPCODE_LOAD_SELF, OPCODE_LOAD_BUILTIN_CLASS, OPCODE_MOVE,
        OPCODE_STORE_LOCAL, OPCODE_LOAD_OUTER_LOCAL, OPCODE_STORE_OUTER_LOCAL,
        OPCODE_SEND_LEXICAL, OPCODE_SEND_PUBLIC, OPCODE_SEND_INLINABLE, OPCODE_SEND_SUPER,
        OPCODE_MAKE_CLOSURE, OPCODE_MAKE_ARRAY, OPCODE_INITIALIZE_SLOT, OPCODE_RETURN,
        OPCODE_IMPLICIT_RETURN, OPCODE_NON_LOCAL_RETURN, OPCODE_JUMP,
        OPCODE_JUMP_IF_NOT_BOOLEAN, OPCODE_BRANCH_IF_FALSE, OPCODE_BRANCH_IF_TRUE,
    ];
    for retired in RETIRED_OPCODES {
        assert!(!live.contains(&retired), "tag {retired} is retired");
    }
}
```

In `tests/image.rs`, beside `an_invalid_instruction_tag_is_a_decode_error_not_a_panic`
(which itself must change: it writes `OPCODE_BRANCH_IF_TRUE + 1`, 21, "one
past the last real tag", and 21 is now `SendLexical`; make it write the
literal `25` and expect `tag: 25`), add:

```rust
/// One Method body's code table: no statements, `site_count` call-site
/// entries (the retired list), and `instruction_count` instructions that
/// `write_instructions` writes.
fn one_method_code_table(
    site_count: u32,
    instruction_count: u32,
    write_instructions: impl FnOnce(&mut encode::Writer),
) -> Vec<u8> {
    let mut writer = encode::Writer::new();
    writer.write_u32(1); // one code entry
    writer.write_u8(encode::code_kind_tag(CodeKind::Method));
    writer.write_u16(0); // argument_count
    writer.write_u16(2); // register_count
    writer.write_u32(0); // source.start
    writer.write_u32(0); // source.end
    writer.write_u32(0); // no statement_program_counter entries
    writer.write_u32(site_count);
    for _ in 0..site_count {
        writer.write_u32(0);
    }
    writer.write_u32(instruction_count);
    write_instructions(&mut writer);
    writer.into_bytes()
}

/// A unary send of symbol 0 from register 0 into register 1, under `tag`.
fn write_unary_send(writer: &mut encode::Writer, tag: u8, number: u32) {
    writer.write_u8(tag);
    writer.write_u32(0); // selector: symbol 0
    writer.write_u16(0); // receiver register
    writer.write_u16(0); // no argument registers
    writer.write_u16(1); // destination
    writer.write_u32(number);
}

fn write_return(writer: &mut encode::Writer) {
    writer.write_u8(OPCODE_RETURN);
    writer.write_u16(1);
}

/// A body written with the retired `SendPublic` tag -- a stale artifact from
/// before the call sites were widened -- is refused, not misread.
#[test]
fn a_retired_send_tag_is_refused() {
    let bytes = one_method_code_table(0, 2, |writer| {
        write_unary_send(writer, 8, 0);
        write_return(writer);
    });
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &table_with(&["size"])),
        Err(decode::DecodeError::InvalidTag {
            what: "instruction",
            tag: 8
        })
    );
}

/// A code record whose call-site list is not empty was written before the
/// list was retired; it is refused before its instructions are read.
#[test]
fn a_code_record_with_a_call_site_list_is_refused() {
    let bytes = one_method_code_table(1, 1, write_return);
    let mut reader = decode::Reader::new(&bytes);
    assert_eq!(
        decode::decode_code_table(&mut reader, &symbols::Table::empty()),
        Err(decode::DecodeError::CallSiteListNotEmpty)
    );
}

/// Two sends of one family with one number would share a cache entry, and
/// a number past the family's count would let a crafted family allocate a
/// range its bytes do not pay for.
#[test]
fn a_family_whose_call_sites_are_not_dense_is_refused() {
    let shared = one_method_code_table(0, 3, |writer| {
        write_unary_send(writer, OPCODE_SEND_PUBLIC, 0);
        write_unary_send(writer, OPCODE_SEND_PUBLIC, 0);
        write_return(writer);
    });
    let sparse = one_method_code_table(0, 2, |writer| {
        write_unary_send(writer, OPCODE_SEND_PUBLIC, 5);
        write_return(writer);
    });
    for bytes in [shared, sparse] {
        let mut reader = decode::Reader::new(&bytes);
        let table = decode::decode_code_table(&mut reader, &table_with(&["size"]))
            .expect("each body decodes on its own");
        assert_eq!(
            decode::verify_code_table(&table),
            Err(decode::DecodeError::CallSitesNotDense)
        );
    }
}

/// The same body numbered densely is accepted: the check refuses the
/// defect, not the shape.
#[test]
fn a_family_numbered_densely_is_accepted() {
    let bytes = one_method_code_table(0, 3, |writer| {
        write_unary_send(writer, OPCODE_SEND_PUBLIC, 1);
        write_unary_send(writer, OPCODE_SEND_PUBLIC, 0);
        write_return(writer);
    });
    let mut reader = decode::Reader::new(&bytes);
    let table = decode::decode_code_table(&mut reader, &table_with(&["size"]))
        .expect("the body decodes");
    assert_eq!(table.codes[0].family_size, 2);
    assert_eq!(decode::verify_code_table(&table), Ok(()));
}
```

If `Return` is not encoded as a tag and a `u16`, or a register list not as a
`u16` count and the registers, follow `encode_instruction` instead; the
comments name each operand so the layout can be checked against it.

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime family` then `cargo test -p ns-runtime retired`,
`cargo test -p ns-runtime call_site` and
`cargo test -p ns-runtime the_reference_lowering_emits`
Expected: FAIL (compile errors on `family_size`, `call_site()`, `RETIRED_OPCODES`).

- [ ] **Step 3: Implement.**

`bytecode.rs`: replace `CallSite` with the Shared definition; in
`CompiledCode` replace `call_sites` with `family_size` and its doc comment;
add

```rust
impl Instruction {
    /// A send's call site; `None` for every other instruction.
    pub fn call_site(&self) -> Option<CallSite> {
        match self {
            Instruction::SendLexical { call_site, .. }
            | Instruction::SendPublic { call_site, .. }
            | Instruction::SendInlinable { call_site, .. }
            | Instruction::SendSuper { call_site, .. } => Some(*call_site),
            _ => None,
        }
    }
}

/// One past the highest call-site number among `instructions`' sends and
/// the `family_size` of every block they create; 0 when there is none.
/// `UNCACHED` sends are not numbered. The compiler and the decoder both
/// compute it here, so the two cannot disagree about a body.
pub(crate) fn family_size_of(instructions: &[Instruction]) -> u32 {
    instructions
        .iter()
        .map(|instruction| match instruction {
            Instruction::MakeClosure { code, .. } => code.family_size,
            other => match other.call_site() {
                Some(site) if site != CallSite::UNCACHED => site.number + 1,
                _ => 0,
            },
        })
        .max()
        .unwrap_or(0)
}
```

(A number of `u32::MAX - 1` makes `+ 1` equal `u32::MAX`, still in range.)
`format_instruction` prints `@{}` with `call_site.number`, and `@-` for
`UNCACHED`: add a small `fn format_call_site(site: CallSite) -> String`.
Update the module's own tests (`CallSite { number: .. }`, `family_size: 0`,
the disassembly's expected text is unchanged for `@1`).

`lower.rs`: replace `call_sites: Vec<CallSite>` with
`next_call_site: u32`; `Lowerer::new` takes a last parameter
`first_call_site: u32` and stores it there; every caller in `compiler/mod.rs`
and in the test module passes `0`. `next_call_site()` becomes:

```rust
    /// The next send's number within this family. Numbers are handed out in
    /// emission order across the root and every block nested in it, so a
    /// family's sends are dense from 0 and none shares a cache entry
    /// (`docs/design/dispatch.md`, "Code families").
    fn take_call_site(&mut self) -> CallSite {
        let number = self.next_call_site;
        self.next_call_site = number
            .checked_add(1)
            .filter(|next| *next < CallSite::UNCACHED.number)
            .expect("internal: more than u32::MAX - 1 sends in one code family");
        CallSite { number }
    }
```

`emit_send` calls `take_call_site` and no longer pushes to a list.
`lower_block_literal` passes `self.next_call_site` as the inner lowerer's
`first_call_site`, and after `inner.lower_body(..)` reads
`let after = inner.next_call_site;` before `inner.finish(..)` and sets
`self.next_call_site = after;`. `finish` sets
`family_size: crate::bytecode::family_size_of(&self.instructions)`.
A family's number overflowing is a compiler-invariant `expect`, which the
"compilation cannot fail" invariant allows: no parser-accepted module comes
near it.

`reference.rs`: its `emit_send` uses `CallSite::UNCACHED`; its `finish`
sets `family_size: crate::bytecode::family_size_of(&self.instructions)`
(which answers 0); the two literal `CallSite { call_site_id: id }` at its
lines ~660/676 become `CallSite::UNCACHED`. Remove its `call_sites` field.

`opcodes.rs`: renumber the four send constants to 21–24 with their doc
comments, change "Next free number: 21." to "Next free number: 25.", and add:

```rust
/// Instruction tags that no longer name anything and are never reused:
/// 7–10 were the four sends while their call site was a `u16` instruction
/// index. Widening it changed what their operands mean, so the sends took
/// 21–24 and a stale artifact meets an unknown tag.
pub const RETIRED_OPCODES: [u8; 4] = [7, 8, 9, 10];
```

`encode.rs`: `encode_code` writes `writer.write_u32(0)` for the call-site
count, with a comment ("The call-site list is retired; the count stays in
the record, written empty, so that a stale body with sends is refused before
its instructions are read"); `encode_call_site` writes `site.number` as a
`u32`.

`decode.rs`: add the two variants to `DecodeError` with `Display` arms
("a code record carries a call-site list, which the format no longer has"
and "a code family's call sites are not numbered densely from 0"); in
`decode_code`, read the count and refuse a non-zero one with
`CallSiteListNotEmpty`; `decode_call_site` reads a `u32`; the decoded
`CompiledCode` gets `family_size: crate::bytecode::family_size_of(&instructions)`.
The unknown-opcode arm already refuses 7–10 once no constant names them;
make sure `decode_instruction`'s `match` has no arm for the retired numbers.

In `verify_code_table`, after the loop, add:

```rust
    verify_call_site_numbering(table, &parents)?;
```

and

```rust
/// Every family -- a body no `MakeClosure` creates, with every block under
/// it -- numbers its sends densely: each number once, all below the root's
/// `family_size`, and exactly `family_size` of them. Two sends sharing a
/// number would share a cache entry and run each other's targets, and a
/// sparse family would make its first activation allocate a range its bytes
/// do not pay for. `UNCACHED` sends are not counted. Iterative: a family's
/// depth is bounded, but its width is the image's.
fn verify_call_site_numbering(
    table: &DecodedCodeTable,
    parents: &[Option<usize>],
) -> Result<(), DecodeError> {
    for (index, root) in table.codes.iter().enumerate() {
        if parents[index].is_some() {
            continue;
        }
        let size = root.family_size as usize;
        let mut seen = vec![false; size];
        let mut count = 0usize;
        let mut pending: Vec<&CompiledCode> = vec![root];
        while let Some(code) = pending.pop() {
            for instruction in &code.instructions {
                if let Instruction::MakeClosure { code: block, .. } = instruction {
                    pending.push(block);
                    continue;
                }
                let Some(site) = instruction.call_site() else { continue };
                if site == CallSite::UNCACHED {
                    continue;
                }
                let number = site.number as usize;
                if number >= size || seen[number] {
                    return Err(DecodeError::CallSitesNotDense);
                }
                seen[number] = true;
                count += 1;
            }
        }
        if count != size {
            return Err(DecodeError::CallSitesNotDense);
        }
    }
    Ok(())
}
```

`size` is bounded by the image: `family_size` is one past a number some send
in the family carries, and `count == size` is checked only after every send
was seen, so `vec![false; size]` can be as large as a `u32` allows before the
check fails. **Bound it first**: before allocating `seen`, count the family's
numbered sends in a first pass and refuse with `CallSitesNotDense` when that
count differs from `size`; only then allocate `seen` (whose length now equals
a count of instructions actually decoded). Write the function that way.

Update every other use: `heap.rs`'s test literals (`family_size: 0`),
`tests/compiler.rs` (`CallSite { number: 3 }`), and anything the compiler
reports.

- [ ] **Step 4: Regenerate the golden image and run everything.**

```bash
cargo test -p ns-runtime --test image regenerate_the_golden_fixture -- --ignored
cargo test --workspace
```

The golden fixture changes because each send's site is two bytes wider and
the tags moved; record its old and new size for Task 10. Disasm snapshots in
`crates/ns-cli/tests/disasm_snapshots.rs` may need their `@N` numbers updated:
the numbers are now family numbers, not instruction indices — check each
changed snapshot against the rule above before updating it.

- [ ] **Step 5: Gates and commit.**

```bash
git add -A crates
git commit -m "feat: number call sites per code family and retire the u16 send tags" -m "<trailer lines>"
```

---

### Task 5: `feedback_base` on every activation

Every activation carries the start of its family's range; nothing reads a
cache yet. Measured on its own, because this repository has seen a single
field on a hot type move timings by 4–8%.

**Files:**
- Create: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`FeedbackBase`, `InlineCaches` with families only)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod inline_cache;`, `Mutator.caches`, `Step::Method.feedback_base`, `push_step`, `push_initializer`, the clause pushes)
- Modify: `crates/ns-runtime/src/heap.rs` (`ActivationHeader.feedback_base`, `StackEntry.feedback_base`, `activation_feedback_base`, `closure_activation`, `set_closure_feedback_base`)
- Modify: `crates/ns-runtime/src/types.rs` (`ClosureData.feedback_base`)
- Modify: `crates/ns-runtime/src/trace.rs`, `image/decode.rs`, `image/encode.rs` (closure literal and destructuring)
- Modify: `crates/ns-runtime/src/vm.rs` (`Flow::Switch.feedback_base`, `MakeClosure`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (initialize `caches`)
- Test: `crates/ns-runtime/src/interpreter/inline_cache.rs`

**Interfaces:**
- Consumes: `CompiledCode.family_size` (Task 4).
- Produces: `pub struct FeedbackBase(u32)` with `NOT_ASSIGNED` and
  `pub(crate) fn index(self) -> usize`;
  `pub(crate) struct InlineCaches` with
  `pub(crate) fn base_for(&mut self, code: &Rc<CompiledCode>) -> FeedbackBase`,
  `pub(crate) fn fresh_range(&mut self, size: u32) -> FeedbackBase`,
  `pub(crate) fn table_len(&self) -> usize`;
  `Mutator.caches: InlineCaches`;
  `Step::Method { .., feedback_base: FeedbackBase }`;
  `ActivationHeader.feedback_base`, `ClosureData.feedback_base`;
  `Heap::activation_feedback_base(activation) -> FeedbackBase`;
  `Heap::closure_activation` answering a fourth element, the closure's `FeedbackBase`;
  `Heap::set_closure_feedback_base(object, FeedbackBase)`;
  `Flow::Switch { activation, code, program_counter, feedback_base }`.

- [ ] **Step 1: Write the failing tests** at the end of the new
  `inline_cache.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{CodeKind, CompiledCode, Instruction};
    use ns_syntax::Span;

    fn body(family_size: u32) -> Rc<CompiledCode> {
        Rc::new(CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![Instruction::Return { register: 0 }],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![],
            family_size,
            source: Span::new(0, 0),
        })
    }

    /// A family gets one contiguous range the first time it is asked for,
    /// and the same one every time after; ranges never overlap.
    #[test]
    fn a_family_keeps_the_range_it_was_first_given() {
        let mut caches = InlineCaches::new();
        let (first, second) = (body(3), body(2));
        let first_base = caches.base_for(&first);
        let second_base = caches.base_for(&second);
        assert_eq!(caches.base_for(&first), first_base);
        assert_eq!(first_base.index(), 0);
        assert_eq!(second_base.index(), 3);
        assert_eq!(caches.table_len(), 5);
    }

    /// A body with no sends needs no range and never allocates one.
    #[test]
    fn a_family_without_sends_takes_no_room() {
        let mut caches = InlineCaches::new();
        let base = caches.base_for(&body(0));
        assert_ne!(base, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(caches.table_len(), 0);
    }

    /// A decoded closure's range is fresh each time it is asked for: any
    /// range of the family's size is a correct one.
    #[test]
    fn a_fresh_range_is_appended() {
        let mut caches = InlineCaches::new();
        caches.base_for(&body(2));
        assert_eq!(caches.fresh_range(4).index(), 2);
        assert_eq!(caches.table_len(), 6);
    }
}
```

The behaviour of the whole — which families take room once a program runs —
is pinned in Task 8, which adds the statistics that test reads; until then
every fixture and golden pins that carrying the base changes no answer.

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime --lib inline_cache`
Expected: FAIL (module does not exist).

- [ ] **Step 3: Implement.**

`inline_cache.rs` starts as:

```rust
//! Inline caches (`docs/design/dispatch.md`, "3b — The caches"): the table of
//! every call site's entry, per heap, reached from an activation's
//! `feedback_base` -- V8's split of shared code and per-context feedback,
//! reached the way HotSpot's interpreter reaches `MethodData`.

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;

// FeedbackBase: the Shared definition, plus
impl FeedbackBase {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

pub(crate) struct InlineCaches {
    /// One slot per call site of every family that has run. Task 7 makes
    /// these `CacheEntry`s; until then the length is all that is kept.
    site_count: usize,
    /// Code identity -> the start of its family's range.
    families: IdentityMap<usize, FeedbackBase>,
    /// Every body in `families`, held so that its address -- the key -- is
    /// never reused by another body while the key is in the map.
    family_codes: Vec<Rc<CompiledCode>>,
}

impl InlineCaches {
    pub(crate) fn new() -> InlineCaches {
        InlineCaches {
            site_count: 0,
            families: IdentityMap::default(),
            family_codes: Vec::new(),
        }
    }

    /// `code`'s range, assigned the first time its family's root is
    /// activated, so that a family that never runs takes no room -- as V8
    /// allocates a feedback vector lazily. A body with no sends is given
    /// base 0 and no room: no site of it is ever indexed.
    pub(crate) fn base_for(&mut self, code: &Rc<CompiledCode>) -> FeedbackBase {
        if code.family_size == 0 {
            return FeedbackBase(0);
        }
        let key = Rc::as_ptr(code) as usize;
        if let Some(base) = self.families.get(&key) {
            return *base;
        }
        let base = self.fresh_range(code.family_size);
        self.families.insert(key, base);
        self.family_codes.push(code.clone());
        base
    }

    /// A new range of `size` sites at the end of the table. `NOT_ASSIGNED`
    /// if the table cannot grow that far, which leaves every site of the
    /// family uncached.
    pub(crate) fn fresh_range(&mut self, size: u32) -> FeedbackBase {
        let start = self.site_count;
        match u32::try_from(start + size as usize) {
            Ok(end) if end < FeedbackBase::NOT_ASSIGNED.0 => {
                self.site_count = end as usize;
                FeedbackBase(start as u32)
            }
            _ => FeedbackBase::NOT_ASSIGNED,
        }
    }

    pub(crate) fn table_len(&self) -> usize {
        self.site_count
    }
}
```

`NOT_ASSIGNED` answered by `fresh_range` must never be taken for an assigned
base by a site lookup: Task 7's `site_index` treats it as uncached. The one
place a `NOT_ASSIGNED` activation base is replaced is the push (below).

`interpreter/mod.rs`: `pub(crate) mod inline_cache;`, and `Mutator` gets

```rust
    /// The inline caches (`inline_cache.rs`): per heap, since a dispatch id
    /// means nothing in another heap.
    pub(crate) caches: inline_cache::InlineCaches,
```

initialized with `InlineCaches::new()` in `World::new`. `Step::Method` gains
`feedback_base: inline_cache::FeedbackBase` — the three existing
constructions (and the test at the end of the file) pass
`FeedbackBase::NOT_ASSIGNED`. **Check the size pin**:
`a_decision_stays_the_size_the_hot_path_was_measured_at` (56 in release)
and its debug twin (64). If either grows, stop and report before
continuing: the step's measurement has to decide on it.

`push_step`'s `Step::Method` arm:

```rust
                let feedback_base = if feedback_base == FeedbackBase::NOT_ASSIGNED {
                    self.caches.base_for(&code)
                } else {
                    feedback_base
                };
```

and passes it in the header. The `Step::Block` arm reads the closure's base
from `closure_activation`'s fourth element; if it is `NOT_ASSIGNED` (a
decoded closure), it takes `self.caches.fresh_range(code.family_size)` and
writes it back with `self.heap.set_closure_feedback_base(closure, base)` —
"a decoded closure is given a fresh range of its family's size when it is
first activated". `push_initializer`, `Step::ClauseMessage` and
`Step::SuperclassClause` take `self.caches.base_for(&code)`.

`heap.rs`: `ActivationHeader` and `StackEntry` gain
`feedback_base: FeedbackBase` (after `slot_base`), copied in
`push_activation`; add

```rust
    /// Where `activation`'s code family starts in the heap's cache table.
    pub(crate) fn activation_feedback_base(&self, activation: ActivationReference) -> FeedbackBase
```

following `activation_code`'s shape, and `set_closure_feedback_base`
following the other closure mutators (it answers nothing; on a non-closure it
does nothing). `closure_activation` answers `(code, parameter_count,
captured_frame, feedback_base)`.

`types.rs`: `ClosureData` gains

```rust
    /// The start of its family's range in the heap's cache table, copied from
    /// the activation that created it, since a block belongs to its method's
    /// family. Never serialized: a decoded closure carries `NOT_ASSIGNED`
    /// and is given a fresh range when it is first activated.
    pub feedback_base: crate::interpreter::inline_cache::FeedbackBase,
```

(`FeedbackBase` must then be reachable as `pub` from `types.rs`: make the
module `pub(crate)` and re-export `FeedbackBase` from `interpreter` if the
compiler requires a public path for a `pub` field — follow what the compiler
asks for, with the least visibility that compiles.) `trace.rs` adds
`feedback_base: _,` with `// not a handle`; `decode.rs`'s closure literal
passes `NOT_ASSIGNED`; `encode.rs` destructures it with `feedback_base: _`
and writes nothing for it; the two `ClosureData` literals in `heap.rs`'s
tests pass `NOT_ASSIGNED`.

`vm.rs`: `Flow::Switch` gains `feedback_base: FeedbackBase`; `run` keeps a
local `feedback_base`, read with the code at the start
(`activation_feedback_base(base)`) and replaced on every switch; `send`'s
switch takes it from `mutator.heap.activation_feedback_base(callee)` (or have
`push_step` answer it beside the activation — whichever keeps `send`'s shape;
measure says which), `finish`'s continuation switch the same, and
`resume_caller` reads the caller's. `execute` takes `feedback_base` as a
parameter and does nothing with it yet except pass it to `MakeClosure`,
which sets `feedback_base: mutator.heap.activation_feedback_base(activation)`
— or the parameter, which is the same value — in the new closure. Prefer the
parameter: it costs no load.

- [ ] **Step 4: Run the tests.**

Run: `cargo test --workspace`
Expected: PASS, every fixture and golden unchanged.

- [ ] **Step 5: Measure `feedback_base` on its own.** By the protocol, with a
  fourth binary: this task's parent commit (Task 4's head) as `parent`, the
  gate against it. Report both sittings' tables against `parent`, START and
  `be3eb40`. If a source's after-group lies entirely above `parent`'s, try,
  one at a time and each measured: (a) `push_step` answering the base with the
  activation instead of `send` loading it; (b) `feedback_base` placed next to
  `slot_base` versus after `counted` in `StackEntry`; (c) `Flow::Switch`
  carrying it versus `run` loading it from the heap on switch. Keep what is
  level; report all of it.

- [ ] **Step 6: Gates and commit.**

```bash
git add -A crates
git commit -m "feat: carry each activation's feedback base" -m "<trailer lines>"
```

---

### Task 6: `resolve`, then `act` — uncached

The decision path is split so that a cache can sit between the two halves.
Behaviour is unchanged; nothing is cached yet.

**Files:**
- Create: `crates/ns-runtime/src/interpreter/resolve.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`decide_send`, the removed `decide_*_lookup` family)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs` (`decide_lexical_send`, `decide_super_send`)
- Modify: `crates/ns-runtime/src/interpreter/slots.rs` (`decide_slot_accessor`'s parameters)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`try_factory_send` visibility; `is_factory_send`)

**Interfaces:**
- Consumes: `Step::Method.feedback_base` (Task 5).
- Produces (in `interpreter::resolve`):
  `Chain`, `Resolved<'gc>` (Shared definitions);
  `impl Mutator { pub(crate) fn resolve_public(&mut self, receiver: Value<'gc>, selector: Symbol) -> Resolved<'gc> }`;
  `pub(crate) fn resolve_lexical(&mut self, enclosing: Value<'gc>, lexical_class: Option<ClassReference<'gc>>, selector: Symbol) -> Resolved<'gc>`;
  `pub(crate) fn resolve_super(&mut self, self_value: Value<'gc>, superclass: ClassReference<'gc>, selector: Symbol) -> Resolved<'gc>`;
  `#[inline(always)] pub(crate) fn act(&mut self, resolved: Resolved<'gc>, receiver: Value<'gc>, superclass: Option<ClassReference<'gc>>, selector: Symbol, arguments: &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>>`;
  `pub(crate) fn decide_slot_accessor(&mut self, receiver: Value<'gc>, kind: AccessorKind, selector: Symbol, field: usize, arguments: &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>>`;
  `pub(crate) fn is_factory_send(heap: &Heap<'gc>, receiver: &Value<'gc>, selector: Symbol) -> bool` in `instantiate.rs`.

- [ ] **Step 1: Write the failing tests** at the end of `resolve.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Interpreter, Root, SourceOrigin};
    use indoc::indoc;

    // copy `load` and `expect_ok` from interpreter/mod.rs's test module

    /// `resolve` decides and runs nothing; each send kind's resolution names
    /// the chain it was found on (`docs/design/dispatch.md`, "Resolve, then
    /// act").
    #[test]
    fn resolving_names_the_chain_and_runs_nothing() {
        let source = indoc! {"
            Newspeak3
            'T'
            class Subject = Object new (
            ) (
              public foo = ( ^1 )
            ) : (
              public make = ( ^2 )
            )
        "};
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&instance);
            let class_object = mutator.fetch(&class);
            assert!(matches!(
                mutator.resolve_public(receiver, Symbol::intern("foo")),
                Resolved::Method { chain: Chain::Public, .. }
            ));
            assert!(matches!(
                mutator.resolve_public(class_object, Symbol::intern("make")),
                Resolved::Method { chain: Chain::PublicClassSide, .. }
            ));
            assert!(matches!(
                mutator.resolve_public(class_object, Symbol::NEW),
                Resolved::Factory { chain: Chain::Public }
            ));
            assert!(matches!(
                mutator.resolve_public(Value::SmallInteger(3), Symbol::intern("+")),
                Resolved::Primitive { chain: Chain::Public, .. }
            ));
            assert!(matches!(
                mutator.resolve_public(receiver, Symbol::intern("zork")),
                Resolved::Primitive { chain: Chain::DoesNotUnderstand, .. }
            ));
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
        });
    }

    /// A resolution that `act` runs answers what `decide_send` answered
    /// before the split: a native leaf is its value.
    #[test]
    fn acting_on_a_primitive_answers_its_value() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let resolved = mutator.resolve_public(Value::SmallInteger(3), Symbol::intern("+"));
            match mutator.act(
                resolved,
                Value::SmallInteger(3),
                None,
                Symbol::intern("+"),
                &[Value::SmallInteger(4)],
            ) {
                Ok(Step::Value(Value::SmallInteger(7))) => {}
                _ => panic!("3 + 4 is 7"),
            }
        });
    }

    /// Every resolution the hot path returns stays small: 3a measured a
    /// 64-byte decision on the return path at 5.5-22.4% (`execution.md`,
    /// "What primitives as method-table entries moved").
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_resolution_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<Resolved>(), 24);
    }
}
```

If `size_of::<Resolved>()` is not 24 in a release build once implemented,
set the pin to what it is and report the figure; Task 6's measurement decides
whether it is acceptable.

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime --lib resolve`
Expected: FAIL (module does not exist).

- [ ] **Step 3: Implement `resolve.rs`.** The resolutions reproduce today's
  decision order exactly; the comments of the functions they replace move
  with them (the §-citations, the measured `#[inline(always)]`/`#[cold]`
  rationale). The mapping:

```rust
//! A send's decision in two halves (`docs/design/dispatch.md`, "Resolve,
//! then act"): `resolve_*` is the lookup, in today's order and by today's
//! rules, answering what it found as a `Resolved`; `act` turns a `Resolved`
//! into the step. The uncached path is `act(resolve(..))`; a cache hit is
//! `act` on a `Resolved` rebuilt from an entry. So there is one place that
//! executes a target, and a hit cannot do anything a miss would not.

impl<'gc> Mutator<'gc> {
    /// §5.5, the explicit-receiver send (was `decide_send`'s lookup): the
    /// factory check first (§6.1, `try_factory_send`'s condition), then a
    /// class object's class side (§6.3), then `lookupPublic` on the class,
    /// then `doesNotUnderstand:`.
    pub(crate) fn resolve_public(&mut self, receiver: Value<'gc>, selector: Symbol) -> Resolved<'gc> {
        if let Value::Object(object) = receiver {
            if self.heap.is_class(object) {
                if is_factory_send(&self.heap, &receiver, selector) {
                    return Resolved::Factory { chain: Chain::Public };
                }
                if let Some(resolved) = self.resolve_class_side(object, selector, Chain::PublicClassSide) {
                    return resolved;
                }
            }
        }
        let class = self.class_of(&receiver);
        let lookup = dispatch::lookup_public(&self.heap, class, selector);
        self.resolved_public(lookup, receiver)
    }

    /// `lookup` as a resolution on the public chain; nothing found is
    /// `doesNotUnderstand:` for the receiver's class (§5.5).
    #[inline(always)]
    fn resolved_public(&mut self, lookup: Lookup<'gc>, receiver: Value<'gc>) -> Resolved<'gc> {
        match lookup {
            Lookup::NotFound => self.resolve_does_not_understand(receiver, Chain::DoesNotUnderstand),
            found => resolved(found, Chain::Public),
        }
    }

    /// The class side, first (§6.3): a metaclass that answers is the
    /// resolution; one that does not falls through to the class object's
    /// own behaviour. `#[inline(never)]`: only a class object reaches it.
    /// A metaclass primitive is resolved like any member; its failure
    /// continues on the metaclass chain and then falls through, in `act`.
    #[inline(never)]
    fn resolve_class_side(&mut self, class_object: ObjectReference<'gc>, selector: Symbol, chain: Chain) -> Option<Resolved<'gc>> {
        let metaclass = self.metaclass_of(class_object)?;
        let lookup = match chain {
            Chain::PublicClassSide => dispatch::lookup_public(&self.heap, metaclass, selector),
            _ => dispatch::lookup_protected(&self.heap, metaclass, selector),
        };
        match lookup {
            Lookup::NotFound => None,
            // §5.9 on a class side (was `decide_lexical_send`'s filter):
            // only a real declaration stops the search.
            Lookup::Primitive(..) if chain == Chain::ProtectedClassSide => None,
            found => Some(resolved(found, chain)),
        }
    }

    /// §5.7-§5.9, the lexical send (was `decide_lexical_send`'s lookup):
    /// N's private member first (`lexical_class` is N, `None` when the
    /// chain is shorter than the depth), then the class side of `o` when
    /// `o` is a class object, then lookupProtected on `o`'s class; nothing
    /// found is an ordinary send to `o`.
    pub(crate) fn resolve_lexical(&mut self, enclosing: Value<'gc>, lexical_class: Option<ClassReference<'gc>>, selector: Symbol) -> Resolved<'gc> {
        if let Some(lexical_class) = lexical_class {
            match dispatch::private_member(&self.heap, lexical_class, selector) {
                Lookup::NotFound => {}
                found => return resolved(found, Chain::Private),
            }
        }
        if let Value::Object(object) = enclosing {
            if self.heap.is_class(object) {
                if let Some(resolved) = self.resolve_class_side(object, selector, Chain::ProtectedClassSide) {
                    return resolved;
                }
            }
        }
        let class = self.class_of(&enclosing);
        match dispatch::lookup_protected(&self.heap, class, selector) {
            Lookup::NotFound => self.resolve_public(enclosing, selector),
            found => resolved(found, Chain::Protected),
        }
    }

    /// §5.10 (was `decide_super_send`'s lookup, after its two errors):
    /// lookupProtected from S; nothing found is the factory check on `self`,
    /// then the `doesNotUnderstand:` defined for S.
    pub(crate) fn resolve_super(&mut self, self_value: Value<'gc>, superclass: ClassReference<'gc>, selector: Symbol) -> Resolved<'gc> {
        match dispatch::lookup_protected(&self.heap, superclass, selector) {
            Lookup::NotFound => self.resolve_super_does_not_understand(self_value, superclass, selector),
            found => resolved(found, Chain::Super),
        }
    }

    #[cold]
    #[inline(never)]
    fn resolve_super_does_not_understand(&mut self, self_value: Value<'gc>, superclass: ClassReference<'gc>, selector: Symbol) -> Resolved<'gc> {
        if is_factory_send(&self.heap, &self_value, selector) {
            return Resolved::Factory { chain: Chain::SuperDoesNotUnderstand };
        }
        self.resolve_handler(superclass, Chain::SuperDoesNotUnderstand)
    }

    /// §5.5's fallback, access-blind (`lookup_defined_for`), from the
    /// receiver's class. `#[cold]` for the reason
    /// `decide_does_not_understand` gave.
    #[cold]
    #[inline(never)]
    fn resolve_does_not_understand(&mut self, receiver: Value<'gc>, chain: Chain) -> Resolved<'gc> {
        let class = self.class_of(&receiver);
        self.resolve_handler(class, chain)
    }

    fn resolve_handler(&mut self, class: ClassReference<'gc>, chain: Chain) -> Resolved<'gc> {
        match dispatch::lookup_defined_for(&self.heap, class, Symbol::DOES_NOT_UNDERSTAND) {
            // A nested declaration can never be the handler (was
            // `decide_does_not_understand_lookup`'s comment).
            Lookup::NotFound | Lookup::NestedClass(..) => Resolved::Unhandled,
            found => resolved(found, chain),
        }
    }
}

/// A found `Lookup` as a resolution on `chain`. `NotFound` is the caller's.
#[inline(always)]
fn resolved<'gc>(lookup: Lookup<'gc>, chain: Chain) -> Resolved<'gc> {
    match lookup {
        Lookup::Method(method, defining_class) => Resolved::Method { method, defining_class, chain },
        Lookup::SlotAccessor(accessor, field) => Resolved::Slot { kind: accessor.kind, field, chain },
        Lookup::NestedClass(mixin, declaring_class) => Resolved::NestedClass { mixin, declaring_class, chain },
        Lookup::Primitive(primitive, level) => Resolved::Primitive { primitive, level, chain },
        Lookup::NotFound => unreachable!("internal: the caller resolves what nothing found"),
    }
}
```

`act` reproduces every arm of the removed `decide_public_lookup`,
`decide_class_side_lookup`, `decide_lexical_lookup`, `decide_super_lookup`
and `decide_does_not_understand_lookup`:

```rust
impl<'gc> Mutator<'gc> {
    /// Runs what a send resolved to (was the five `decide_*_lookup`
    /// functions). `receiver` is the send's receiver -- `o` for a lexical
    /// send, `self` for `super` -- and `superclass` is S for a `super` send,
    /// `None` otherwise. `#[inline(always)]` so that the caller holding the
    /// `Resolved` also builds the step (`execution.md`, "What primitives as
    /// method-table entries moved").
    #[inline(always)]
    pub(crate) fn act(&mut self, resolved: Resolved<'gc>, receiver: Value<'gc>, superclass: Option<ClassReference<'gc>>, selector: Symbol, arguments: &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>> {
        match resolved {
            Resolved::Method { method, defining_class, chain } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    self.act_handler_method(method, defining_class, chain, receiver, selector, arguments)
                }
                _ => Ok(Step::Method {
                    receiver,
                    method,
                    defining_class,
                    arguments: StepArguments::Send,
                    feedback_base: FeedbackBase::NOT_ASSIGNED,
                }),
            },
            Resolved::Slot { kind, field, chain } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    self.act_handler_slot(kind, field, chain, receiver, selector, arguments)
                }
                // §6.3.2 (was `decide_public_lookup`'s comment).
                _ => self.decide_slot_accessor(receiver, kind, selector, field, arguments),
            },
            Resolved::NestedClass { mixin, declaring_class, .. } => {
                nested::nested_class(self, &receiver, &mixin, declaring_class)
            }
            Resolved::Primitive { primitive, level, chain } => match chain {
                Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand => {
                    self.act_handler_primitive(primitive, level, chain, receiver, selector, arguments)
                }
                _ => match crate::builtins::execute(self, primitive, receiver, selector, arguments) {
                    Some(result) => result,
                    None => self.act_above(level, chain, receiver, superclass, selector, arguments),
                },
            },
            Resolved::Factory { chain } => self.act_factory(chain, receiver, superclass, selector, arguments),
            Resolved::Unhandled => Err(unhandled_error(receiver, selector)),
        }
    }
}
```

and the cold helpers, each a transcription of the arm it replaces:

- `act_above(level, chain, receiver, superclass, selector, arguments)`,
  `#[cold] #[inline(never)]` — where a failed primitive continues:
  - `Chain::Public`: `lookup_above(Rule::Public, level)`, then
    `resolved_public`, then `act` (was `decide_public_above`);
  - `Chain::PublicClassSide`: `lookup_above(Rule::Public, level)`; a found
    member is `act(resolved(found, PublicClassSide))`; `NotFound` falls
    through to the class object's own behaviour:
    `lookup_public(class_of(receiver))` → `resolved_public` → `act` (was
    `decide_class_side_lookup`'s primitive arm returning `None` into
    `decide_send`);
  - `Chain::Protected`, `Chain::ProtectedClassSide`, `Chain::Private`:
    `lookup_above(Rule::Protected, level)`; found → `act(resolved(found,
    Protected))`; `NotFound` → `act(resolve_public(receiver))` (was
    `decide_lexical_above`, whose `NotFound` was `decide_send(enclosing)`);
  - `Chain::Super`: `lookup_above(Rule::Protected, level)`; found →
    `act(resolved(found, Super))`; `NotFound` →
    `act(resolve_super_does_not_understand(receiver, superclass?))` (was
    `decide_super_above`); `superclass` is `Some` on this chain by
    construction — an `expect("internal: a super chain carries S")` is a
    compiler invariant, never input-reachable, since only `send_super` and
    `decide_super_send` build this chain and both pass S;
  - the two handler chains never reach here (their primitives go through
    `act_handler_primitive`).
- `act_factory(chain, receiver, superclass, selector, arguments)`:
  `try_factory_send(&receiver, selector, arguments)` → `Some(result)` is the
  answer (the re-check `dispatch.md` requires); `None` → for `Chain::Public`,
  `act(resolve_public(receiver))` — which cannot answer `Factory` again,
  since it asks the same condition; for `Chain::SuperDoesNotUnderstand`,
  `act(resolve_handler(S, SuperDoesNotUnderstand))`.
- `act_handler_method` / `act_handler_slot` / `act_handler_primitive`: for
  `Chain::SuperDoesNotUnderstand`, first `try_factory_send(&receiver,
  selector, arguments)` and answer it if `Some` (refinement 3 above); then
  build the message exactly as `decide_does_not_understand_in` did
  (`new_symbol(selector)`, `new_array(arguments.to_vec())`,
  `new_array(vec![selector_value, arguments_value])`) and:
  - method: `Step::Method { .., arguments: StepArguments::Owned(vec![message]), feedback_base: NOT_ASSIGNED }`;
  - slot: `decide_slot_accessor(receiver, kind, Symbol::DOES_NOT_UNDERSTAND, field, &[message])`;
  - primitive: `builtins::execute(self, primitive, receiver,
    Symbol::DOES_NOT_UNDERSTAND, &[message])`; `None` →
    `lookup_above(Rule::DefinedFor, level, DOES_NOT_UNDERSTAND)`, and the
    same three arms again on what it found, `NotFound`/`NestedClass` being
    `unhandled_error` — a loop or a recursion over the chain, as
    `decide_does_not_understand_lookup` was; build the message once, before
    the loop, and pass it along.
- `unhandled_error(receiver, selector) -> Signal`: the text of today's
  `decide_does_not_understand_lookup` `NotFound` arm, moved, word for word.

Then the callers:

```rust
    // interpreter/mod.rs
    pub(crate) fn decide_send(&mut self, receiver: Value<'gc>, selector: Symbol, arguments: &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>> {
        let resolved = self.resolve_public(receiver, selector);
        self.act(resolved, receiver, None, selector, arguments)
    }

// interpreter/sends.rs -- the doc comments stay, the bodies become:
pub(crate) fn decide_lexical_send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    depth: u16,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let (self_value, defining_class) = mutator
        .heap
        .activation_receiver_and_current_class(activation);
    let enclosing =
        dispatch::kth_enclosing_object(&mutator.heap, &self_value, defining_class.as_ref(), depth);
    let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
    let resolved = mutator.resolve_lexical(enclosing, lexical_class, selector);
    mutator.act(resolved, enclosing, None, selector, arguments)
}

pub(crate) fn decide_super_send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    // The two `let ... else` blocks raising "super send with no enclosing
    // current_class ..." and "super send with no superclass ...", word for
    // word as they are today, binding `superclass`; then:
    let self_value = mutator.heap.activation_receiver(activation);
    let resolved = mutator.resolve_super(self_value, superclass, selector);
    mutator.act(resolved, self_value, Some(superclass), selector, arguments)
}
```

`decide_super_does_not_understand`, `decide_does_not_understand`,
`decide_does_not_understand_in` and the five `decide_*_lookup`/`*_above`
functions are removed once nothing calls them. `slots.rs`'s
`decide_slot_accessor` takes `kind` and `selector` instead of a
`SlotAccessor` (it read only those two). In `instantiate.rs`, add

```rust
/// Whether `selector` sent to `receiver` is §6.1's instantiation: the
/// receiver is a class object and the selector its primary factory selector.
/// The one condition `try_factory_send` acts on, asked without acting.
pub(crate) fn is_factory_send<'gc>(heap: &Heap<'gc>, receiver: &Value<'gc>, selector: Symbol) -> bool {
    match receiver {
        Value::Object(object) => primary_factory_selector(heap, *object) == Some(selector),
        _ => false,
    }
}
```

and make `try_factory_send` call it for its check; widen `try_factory_send`
to `pub(crate)` if `resolve.rs` needs it.

**The five `interpreter/mod.rs` tests that call `decide_send`** keep passing
unchanged; the `a_lookup_stays_the_size…` pin stays.

- [ ] **Step 4: Run the tests.**

Run: `cargo test --workspace`
Expected: PASS, every fixture and golden unchanged.

- [ ] **Step 5: Measure.** By the protocol, with this task's parent (Task 5's
  head) as a fourth binary; the gate is against the parent. If a source lies
  entirely above it, apply "The shape to watch for" and measure each change on
  its own; record every variant tried in the task report. Do not continue to
  Task 7 with a source entirely above the parent in both sittings; report to
  the controller instead.

- [ ] **Step 6: Gates and commit.** One commit for the split; each
  measured `perf:` reshaping, if any, its own commit.

```bash
git add -A crates/ns-runtime
git commit -m "refactor: split a send's decision into resolve and act" -m "<trailer lines>"
```

---

### Task 7: The caches

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (entries, sites, pool, global cache, the three cached sends, recording and materializing, the test view)
- Modify: `crates/ns-runtime/src/vm.rs` (the four send arms)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (public test view on `Interpreter`)
- Modify: `crates/ns-runtime/src/lib.rs` (re-export the view types)
- Test: `crates/ns-runtime/tests/inline_caches.rs` (new), unit tests in `inline_cache.rs`

**Interfaces:**
- Consumes: `DispatchId` (Task 3), `CallSite`/`family_size` (Task 4),
  `FeedbackBase`/`InlineCaches` (Task 5), `resolve_*`/`act`/`Chain`/`Resolved` (Task 6).
- Produces:
  `Target`, `CacheEntry`, `CacheRule` (Shared definitions);
  `pub(crate) fn send_public<'gc>(mutator: &mut Mutator<'gc>, feedback_base: FeedbackBase, family_size: u32, site: CallSite, receiver: Value<'gc>, selector: Symbol, arguments: &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>>`;
  `pub(crate) fn send_lexical<'gc>(mutator, activation: ActivationReference, feedback_base, family_size, site, depth: u16, selector, arguments) -> Result<Step<'gc>, Signal<'gc>>`;
  `pub(crate) fn send_super<'gc>(mutator, activation, feedback_base, family_size, site, selector, arguments) -> Result<Step<'gc>, Signal<'gc>>`;
  public, for tests: `pub enum CallSiteState { Vacant, Monomorphic(CachedTarget), Polymorphic(Vec<CachedTarget>), Megamorphic }`,
  `pub struct CachedTarget { pub kind: CachedTargetKind, pub private: bool }`,
  `pub enum CachedTargetKind { Method(Symbol), Slot { field: u32 }, NestedClass(Symbol), Primitive, Factory, DoesNotUnderstand }`;
  `pub enum MethodSide { Instance, Class }`;
  `impl Interpreter { pub fn call_site_states(&mut self, class: &Root, side: MethodSide, method: Symbol, sent: Symbol) -> Vec<CallSiteState> }`.

- [ ] **Step 1: Write the failing tests** in the new
  `crates/ns-runtime/tests/inline_caches.rs`. Each test names the requirement
  it pins from `dispatch.md`'s "Verification". The answers were worked out by
  hand from each module; the sites' states follow from the transitions in
  Step 5.

```rust
//! The inline caches (`docs/design/dispatch.md`, "3b — The caches"): what a
//! site records for the receivers it sees. Every program here also answers
//! what the uncached path answers; the fixtures and goldens pin that under
//! both lowerings, these pin the caches' own states.

use indoc::indoc;
use ns_runtime::{
    CachedTarget, CachedTargetKind, CallSiteState, HostResult, HostSignal, Interpreter,
    Lowering, MethodSide, Root, SourceOrigin, Symbol, Value,
};

fn load_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

fn load(source: &str) -> (Interpreter, Root) {
    load_with(source, Lowering::Optimized)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// Sends `selector` to `receiver` with `arguments`, expecting a value.
fn send(interpreter: &mut Interpreter, receiver: &Root, selector: &str, arguments: &[Root]) -> Root {
    expect_ok(
        interpreter.send(receiver, Symbol::intern(selector), arguments),
        selector,
    )
}

fn instantiate(interpreter: &mut Interpreter, class: &Root) -> Root {
    expect_ok(interpreter.instantiate(class, &[]), "instantiate")
}

fn small_integer(interpreter: &mut Interpreter, root: &Root) -> i64 {
    interpreter.enter(|mutator| match mutator.fetch(root) {
        Value::SmallInteger(integer) => integer,
        _ => panic!("expected a small integer"),
    })
}

fn integer_argument(interpreter: &Interpreter, integer: i64) -> Root {
    interpreter.stash(Value::SmallInteger(integer))
}

fn states(interpreter: &mut Interpreter, class: &Root, side: MethodSide, method: &str, sent: &str) -> Vec<CallSiteState> {
    interpreter.call_site_states(class, side, Symbol::intern(method), Symbol::intern(sent))
}

fn public(kind: CachedTargetKind) -> CachedTarget {
    CachedTarget { kind, private: false }
}

fn method(selector: &str) -> CachedTarget {
    public(CachedTargetKind::Method(Symbol::intern(selector)))
}

/// Two `Inner` instances from two `Outer` instances share one monomorphic
/// entry: the cache guards on the application, not on the class object.
#[test]
fn nested_class_instances_of_two_enclosing_objects_share_one_entry() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          class Outer = Object new () (
            public class Inner = Object new () ( public describe = ( ^1 ) )
          )
          public run = (
            | sum |
            sum := 0.
            {Outer new Inner new. Outer new Inner new. Outer new Inner new}
              do: [:each | sum := sum + each describe].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 3);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "describe"),
        vec![CallSiteState::Monomorphic(method("describe"))]
    );
}

const FIVE_KINDS: &str = indoc! {"
    Newspeak3
    'T'
    class Subject = Object new () (
      class KindOne = Object new () ( public value = ( ^1 ) )
      class KindTwo = Object new () ( public value = ( ^2 ) )
      class KindThree = Object new () ( public value = ( ^3 ) )
      class KindFour = Object new () ( public value = ( ^4 ) )
      class KindFive = Object new () ( public value = ( ^5 ) )
      public run: count = (
        | sum index all |
        sum := 0.
        index := 0.
        all := {KindOne new. KindTwo new. KindThree new. KindFour new. KindFive new}.
        [index < count] whileTrue: [
          index := index + 1.
          sum := sum + (all at: index) value].
        ^sum
      )
    ) : ()
"};

/// A site that sees a second type goes polymorphic, keeps four in the order
/// first seen, and a fifth makes it megamorphic.
#[test]
fn a_site_goes_polymorphic_then_megamorphic() {
    let (mut interpreter, class) = load(FIVE_KINDS);
    let instance = instantiate(&mut interpreter, &class);
    let value_states = |interpreter: &mut Interpreter| {
        states(interpreter, &class, MethodSide::Instance, "run:", "value")
    };
    let two = integer_argument(&interpreter, 2);
    let answer = send(&mut interpreter, &instance, "run:", &[two]);
    assert_eq!(small_integer(&mut interpreter, &answer), 3);
    assert_eq!(
        value_states(&mut interpreter),
        vec![CallSiteState::Polymorphic(vec![method("value"), method("value")])]
    );
    let four = integer_argument(&interpreter, 4);
    send(&mut interpreter, &instance, "run:", &[four]);
    assert_eq!(
        value_states(&mut interpreter),
        vec![CallSiteState::Polymorphic(vec![method("value"); 4])]
    );
    let five = integer_argument(&interpreter, 5);
    let answer = send(&mut interpreter, &instance, "run:", &[five]);
    assert_eq!(small_integer(&mut interpreter, &answer), 15);
    assert_eq!(value_states(&mut interpreter), vec![CallSiteState::Megamorphic]);
}

/// `true` and `false` are instances of two classes: a site that is not
/// inlined and sees both is bimorphic.
#[test]
fn a_site_on_true_and_false_is_bimorphic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          public run = (
            | count |
            count := 0.
            {true. false. true} do: [:each | each not ifTrue: [count := count + 1]].
            ^count
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 1);
    let primitive = public(CachedTargetKind::Primitive);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "not"),
        vec![CallSiteState::Polymorphic(vec![primitive.clone(), primitive])]
    );
}

const TWO_APPLICATIONS: &str = indoc! {"
    Newspeak3
    'T'
    class Subject = Object new () (
      class Narrow = Object new (| first ::= 1. |) ( public describe = ( ^1 ) )
      class Wide = Object new (| first ::= 1. second ::= 2. third ::= 3. |) ( public describe = ( ^2 ) )
      class Box on: base = Object new (| base = base. |) (
        public class Cell = base new (| public held ::= 10. |) (
          public describe = ( ^super describe + 10 )
        )
      )
      public narrowCell = ( ^(Box on: Narrow) Cell )
      public run = (
        | sum |
        sum := 0.
        {(Box on: Narrow) Cell new. (Box on: Wide) Cell new}
          do: [:each | sum := sum + each held + each describe].
        ^sum
      )
    ) : ()
"};

/// One slot accessor under two applications of one mixin with different
/// inherited slots: two entries, each with its own absolute field -- `held`
/// after one inherited slot, and after three.
#[test]
fn a_slot_accessor_under_two_applications_has_two_fields() {
    let (mut interpreter, class) = load(TWO_APPLICATIONS);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 43);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "held"),
        vec![CallSiteState::Polymorphic(vec![
            public(CachedTargetKind::Slot { field: 1 }),
            public(CachedTargetKind::Slot { field: 3 }),
        ])]
    );
}

/// `super` in a mixin applied under two superclasses: the key is the running
/// method's application, so the one `super describe` site holds one entry per
/// application of `Cell`, each naming its own superclass's method.
#[test]
fn a_super_send_in_a_mixin_applied_twice() {
    let (mut interpreter, class) = load(TWO_APPLICATIONS);
    let instance = instantiate(&mut interpreter, &class);
    send(&mut interpreter, &instance, "run", &[]);
    let cell = send(&mut interpreter, &instance, "narrowCell", &[]);
    assert_eq!(
        states(&mut interpreter, &cell, MethodSide::Instance, "describe", "describe"),
        vec![CallSiteState::Polymorphic(vec![method("describe"), method("describe")])]
    );
}

/// A private lexical site whose `o` changes object and class stays
/// monomorphic: its guard is N's application, which does not change, and
/// §5.9 runs N's private method on every `o`.
#[test]
fn a_private_lexical_site_with_a_changing_receiver_stays_monomorphic() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          class Outer named: label = Object new (| name = label. |) (
            private secret = ( ^name )
            public class Inner = Object new () (
              public reveal = ( ^secret )
            )
          )
          class Sub named: label = Outer named: label () (
            public secret = ( ^0 )
          )
          public anInner = ( ^(Outer named: 1) Inner )
          public run = (
            | sum |
            sum := 0.
            {Outer named: 1. Sub named: 2. Outer named: 3. Sub named: 4}
              do: [:each | sum := sum + each Inner new reveal].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 10);
    let inner = send(&mut interpreter, &instance, "anInner", &[]);
    assert_eq!(
        states(&mut interpreter, &inner, MethodSide::Instance, "reveal", "secret"),
        vec![CallSiteState::Monomorphic(CachedTarget {
            kind: CachedTargetKind::Method(Symbol::intern("secret")),
            private: true,
        })]
    );
}

/// Class objects and instances at one site: a factory, a class-side method
/// and an instance method, three entries.
#[test]
fn class_objects_and_instances_share_a_site() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          class Maker make = Object new () ( public describe = ( ^1 ) )
          class Plain = Object new () ( public describe = ( ^2 ) ) : ( public make = ( ^Plain new ) )
          class Custom = Object new () ( public describe = ( ^3 ) public make = ( ^self ) )
          public run = (
            | sum |
            sum := 0.
            {Maker. Plain. Custom new} do: [:each | sum := sum + each make describe].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 6);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "make"),
        vec![CallSiteState::Polymorphic(vec![
            public(CachedTargetKind::Factory),
            method("make"),
            method("make"),
        ])]
    );
}

const SUPER_FACTORY: &str = indoc! {"
    Newspeak3
    'T'
    class Subject = Object new () (
      class Base = Object new () () : ( public doesNotUnderstand: message = ( ^0 ) )
      class Simple = Base new () ( public value = ( ^1 ) ) : ( public build = ( ^super new ) )
      class Keyed with: seed = Simple new () ()
      public simple = ( ^Simple )
      public run: order = (
        | sum |
        sum := 0.
        order do: [:each | | built |
          built := each build.
          sum := sum * 10 + ((built == 0) ifTrue: [0] ifFalse: [built value])].
        ^sum
      )
      public simpleFirst = ( ^self run: {Simple. Keyed. Simple. Keyed} )
      public keyedFirst = ( ^self run: {Keyed. Simple. Keyed. Simple} )
    ) : ()
"};

/// The `super new` in `Simple`'s inherited class-side `build` misses and
/// reaches the factory check on `self`: `Simple` instantiates, `Keyed`
/// (factory `with:`) reaches the handler defined for S. Whichever the site
/// records first, the other receiver must still get its own answer, under
/// both lowerings.
#[test]
fn a_super_factory_entry_is_rechecked_against_self() {
    for (entry, expected, first) in [
        ("simpleFirst", 1010, CachedTargetKind::Factory),
        ("keyedFirst", 101, CachedTargetKind::DoesNotUnderstand),
    ] {
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            let (mut interpreter, class) = load_with(SUPER_FACTORY, lowering);
            let instance = instantiate(&mut interpreter, &class);
            let answer = send(&mut interpreter, &instance, entry, &[]);
            assert_eq!(small_integer(&mut interpreter, &answer), expected, "{entry}");
            if lowering == Lowering::Optimized {
                let simple = send(&mut interpreter, &instance, "simple", &[]);
                assert_eq!(
                    states(&mut interpreter, &simple, MethodSide::Class, "build", "new"),
                    vec![CallSiteState::Monomorphic(public(first.clone()))],
                    "{entry}"
                );
            }
        }
    }
}

/// `doesNotUnderstand:` with a user handler and with a slot setter named
/// `doesNotUnderstand` (§6.3.2) at one site: two entries.
#[test]
fn does_not_understand_handlers_are_entries_like_any_other() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          class Catcher = Object new () ( public doesNotUnderstand: message = ( ^1 ) )
          class Recorder = Object new (| public doesNotUnderstand ::= nil. |) ()
          public run = (
            | sum |
            sum := 0.
            {Catcher new. Recorder new. Catcher new} do: [:each | | answer |
              answer := each zork.
              sum := sum + ((answer == each) ifTrue: [10] ifFalse: [answer])].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 12);
    let handler = public(CachedTargetKind::DoesNotUnderstand);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "zork"),
        vec![CallSiteState::Polymorphic(vec![handler.clone(), handler])]
    );
}

/// A cached primitive that fails on a receiver of its own class -- the
/// `transcript` stream shares the `Platform` class with the platform object
/// -- keeps its entry, and the send continues uncached into
/// `doesNotUnderstand:`.
#[test]
fn a_failing_primitive_keeps_its_entry() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          public run: platform = (
            | stream |
            stream := platform transcript.
            {platform. stream} do: [:each | each transcript].
            ^0
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let platform = interpreter.platform();
    match interpreter.send(&instance, Symbol::intern("run:"), &[platform]) {
        Err(HostSignal::Error(error)) => assert!(
            error.message.contains("does not understand #transcript"),
            "unexpected message: {}",
            error.message
        ),
        _ => panic!("the stream does not understand `transcript`"),
    }
    let primitive = CallSiteState::Monomorphic(public(CachedTargetKind::Primitive));
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run:", "transcript"),
        vec![primitive.clone(), primitive]
    );
}
```

`CachedTargetKind`, `CachedTarget` and `CallSiteState` derive `Debug`,
`Clone`, `PartialEq` and `Eq`. Unit tests in `inline_cache.rs`'s test
module:

```rust
    /// A site index past its code's family, or from an unassigned base, is
    /// never used: the send runs uncached.
    #[test]
    fn a_site_outside_its_family_is_uncached() {
        assert_eq!(site_index(FeedbackBase(4), 3, CallSite { number: 3 }), None);
        assert_eq!(site_index(FeedbackBase::NOT_ASSIGNED, 3, CallSite { number: 0 }), None);
        assert_eq!(site_index(FeedbackBase(4), 3, CallSite::UNCACHED), None);
        assert_eq!(site_index(FeedbackBase(4), 3, CallSite { number: 2 }), Some(6));
    }

    /// An entry holds no handle and is the size the pool's blocks are laid
    /// out for.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn an_entry_stays_the_size_the_caches_were_measured_at() {
        assert_eq!(std::mem::size_of::<Target>(), 16);
        assert_eq!(std::mem::size_of::<CacheEntry>(), 24);
    }
```

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime --test inline_caches` and `cargo test -p ns-runtime --lib inline_cache`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the table, the pool and the global cache.**

`InlineCaches` becomes:

```rust
pub(crate) struct InlineCaches {
    /// Every call site of every family that has run, `VACANT` until filled.
    sites: Vec<CacheEntry>,
    /// Polymorphic blocks, four entries each in the order first seen --
    /// V8's width, Strongtalk's shape.
    pool: Vec<[CacheEntry; POLYMORPHIC_WIDTH]>,
    /// Blocks a megamorphic transition or an invalidation gave back.
    free_blocks: Vec<u32>,
    global: GlobalCache,
    families: IdentityMap<usize, FeedbackBase>,
    family_codes: Vec<Rc<CompiledCode>>,
}

pub(crate) const POLYMORPHIC_WIDTH: usize = 4;
pub(crate) const GLOBAL_CACHE_ENTRIES: usize = 1024;
```

`fresh_range` now `resize`s `sites` with `CacheEntry::VACANT`
(`const VACANT: CacheEntry = CacheEntry { guard: DispatchId::VACANT, target: Target::Vacant }`
— `Target` holds `Rc`s, so this is a `fn vacant() -> CacheEntry`, not a
`const`). `table_len` answers `sites.len()`.

```rust
/// The index of `site` in its activation's range, or `None` when the send
/// runs uncached: the reference lowering's sends, a site at or past its
/// code's family, and an activation whose range could not be assigned.
#[inline(always)]
pub(crate) fn site_index(feedback_base: FeedbackBase, family_size: u32, site: CallSite) -> Option<usize> {
    if site.number >= family_size || feedback_base == FeedbackBase::NOT_ASSIGNED {
        return None;
    }
    Some(feedback_base.index() + site.number as usize)
}
```

(`UNCACHED` is `u32::MAX`, never below a `family_size`, so the first test
covers it.) Every read of `sites` uses `get`/`get_mut` and treats `None` as
uncached, so no crafted base can index out of range.

The global cache:

```rust
struct GlobalEntry {
    guard: DispatchId,
    selector: Symbol,
    rule: CacheRule,
    target: Target,
}

pub(crate) struct GlobalCache {
    entries: Vec<GlobalEntry>,
    mask: usize,
}

impl GlobalCache {
    /// Two probes at two hashes of the key, as Primordial Soup's
    /// `LookupCache` probes (`prior-art/primordialsoup/vm/lookup_cache.h`).
    fn probes(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> [usize; 2] {
        let hash = crate::hash::IdentityBuildHasher::default().hash_one((guard, selector, rule));
        [hash as usize & self.mask, (hash >> 32) as usize & self.mask]
    }

    fn get(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> Option<&Target> { /* either probe whose key matches */ }

    /// The first probe if it is vacant or already holds this key, else the
    /// second if vacant, else the first, evicting it -- Primordial Soup's
    /// `InsertOrdinary`.
    fn insert(&mut self, guard: DispatchId, selector: Symbol, rule: CacheRule, target: Target) { .. }

    fn clear(&mut self) { /* every entry to guard VACANT, target Vacant */ }

    pub(crate) fn with_entries(entries: usize) -> GlobalCache {
        assert!(entries.is_power_of_two());
        // entries vacant
    }
}
```

Read Primordial Soup's `InsertOrdinary` in
`prior-art/primordialsoup/vm/lookup_cache.cc` before writing `insert`, and
cite it.

- [ ] **Step 4: Implement recording and materializing.**

A cached send knows its key before it looks at the table; everything the
three send kinds share takes it as one value:

```rust
/// What a cached send carries besides its site: the guard, the rule the
/// global cache files it under, and what `materialize` and `act` need --
/// the receiver (`o` for a lexical send, `self` for `super`), N for a
/// private lexical site, S for `super`.
#[derive(Clone, Copy)]
struct SendKey<'gc> {
    guard: DispatchId,
    rule: CacheRule,
    receiver: Value<'gc>,
    lexical_class: Option<ClassReference<'gc>>,
    superclass: Option<ClassReference<'gc>>,
}

impl SendKey<'_> {
    fn private(&self) -> bool {
        self.rule == CacheRule::LexicalPrivate
    }
}

/// Where `chain`'s levels are counted from for this send. `None` when the
/// start does not exist -- only a crafted image makes that -- which makes
/// the send a miss.
fn chain_start<'gc>(mutator: &mut Mutator<'gc>, chain: Chain, key: SendKey<'gc>) -> Option<ClassReference<'gc>> {
    match chain {
        Chain::Public | Chain::Protected | Chain::DoesNotUnderstand => Some(mutator.class_of(&key.receiver)),
        Chain::PublicClassSide | Chain::ProtectedClassSide => match key.receiver {
            Value::Object(object) => mutator.metaclass_of(object),
            _ => None,
        },
        Chain::Private => key.lexical_class,
        Chain::Super | Chain::SuperDoesNotUnderstand => key.superclass,
    }
}

/// How many superclass steps from `start` reach `level`, if at most
/// `u16::MAX`. Only the miss path counts, once per recorded target.
fn levels_between<'gc>(heap: &Heap<'gc>, start: ClassReference<'gc>, level: ClassReference<'gc>) -> Option<u16> {
    let mut current = start;
    for steps in 0..=u16::MAX {
        if current.same(level) {
            return Some(steps);
        }
        current = heap.class_superclass(current)?;
    }
    None
}

/// `level` superclass steps up from `start`: the defining class a hit
/// recomputes, since a class object is per enclosing instance and so is
/// never recorded (`docs/design/dispatch.md`, "An entry").
fn level_above<'gc>(heap: &Heap<'gc>, start: ClassReference<'gc>, level: u16) -> Option<ClassReference<'gc>> {
    let mut current = start;
    for _ in 0..level {
        current = heap.class_superclass(current)?;
    }
    Some(current)
}

/// The chain a resolution was found on; `None` for `Unhandled`.
impl Resolved<'_> {
    pub(crate) fn chain(&self) -> Option<Chain> {
        match self {
            Resolved::Method { chain, .. }
            | Resolved::Slot { chain, .. }
            | Resolved::NestedClass { chain, .. }
            | Resolved::Primitive { chain, .. }
            | Resolved::Factory { chain } => Some(*chain),
            Resolved::Unhandled => None,
        }
    }
}

/// What an entry records for `resolved`, or `None` for what is never
/// recorded: `Unhandled`, a level too far up, a field past `u32`. It runs
/// nothing; `base_for` may assign the method's family its range.
fn record<'gc>(mutator: &mut Mutator<'gc>, resolved: &Resolved<'gc>, key: SendKey<'gc>) -> Option<Target> {
    let chain = resolved.chain()?;
    let needs_start = matches!(
        resolved,
        Resolved::Method { .. } | Resolved::NestedClass { .. } | Resolved::Primitive { .. }
    );
    let start = if needs_start { chain_start(mutator, chain, key) } else { None };
    Some(match resolved {
        Resolved::Method { method, defining_class, .. } => Target::Method {
            method: method.clone(),
            feedback_base: mutator.caches.base_for(&method.code),
            level: levels_between(&mutator.heap, start?, *defining_class)?,
            chain,
        },
        Resolved::Slot { kind, field, .. } => Target::Slot {
            field: u32::try_from(*field).ok()?,
            kind: *kind,
            chain,
        },
        Resolved::NestedClass { mixin, declaring_class, .. } => Target::NestedClass {
            mixin: mixin.clone(),
            level: levels_between(&mutator.heap, start?, *declaring_class)?,
            chain,
        },
        Resolved::Primitive { primitive, level, .. } => Target::Primitive {
            primitive: *primitive,
            level: levels_between(&mutator.heap, start?, *level)?,
            chain,
        },
        Resolved::Factory { .. } => Target::Factory { chain },
        Resolved::Unhandled => return None,
    })
}

/// The `Resolved` an entry's target names for this send, with the base a
/// method's activation takes, or `None` when a step up does not exist,
/// which makes the send a miss.
fn materialize<'gc>(mutator: &mut Mutator<'gc>, target: &Target, key: SendKey<'gc>) -> Option<(Resolved<'gc>, FeedbackBase)> {
    Some(match target {
        Target::Method { method, feedback_base, level, chain } => {
            let start = chain_start(mutator, *chain, key)?;
            let defining_class = level_above(&mutator.heap, start, *level)?;
            (
                Resolved::Method { method: method.clone(), defining_class, chain: *chain },
                *feedback_base,
            )
        }
        Target::Slot { field, kind, chain } => (
            Resolved::Slot { kind: *kind, field: *field as usize, chain: *chain },
            FeedbackBase::NOT_ASSIGNED,
        ),
        Target::NestedClass { mixin, level, chain } => {
            let start = chain_start(mutator, *chain, key)?;
            let declaring_class = level_above(&mutator.heap, start, *level)?;
            (
                Resolved::NestedClass { mixin: mixin.clone(), declaring_class, chain: *chain },
                FeedbackBase::NOT_ASSIGNED,
            )
        }
        Target::Primitive { primitive, level, chain } => {
            let start = chain_start(mutator, *chain, key)?;
            let level = level_above(&mutator.heap, start, *level)?;
            (
                Resolved::Primitive { primitive: *primitive, level, chain: *chain },
                FeedbackBase::NOT_ASSIGNED,
            )
        }
        Target::Factory { chain } => (Resolved::Factory { chain: *chain }, FeedbackBase::NOT_ASSIGNED),
        // Site states are guarded by `VACANT`, which no receiver has, so a
        // guard that matched never names one.
        Target::Vacant | Target::Polymorphic { .. } | Target::Megamorphic { .. } => return None,
    })
}
```

(`ClassReference`'s identity comparison is `same`, as `dispatch.rs`'s tests
use it.) A hit's `act` must carry the entry's `feedback_base` into the step:
add to `resolve.rs`

```rust
    /// `act`, with the base a method step's activation takes -- a cache hit
    /// knows its target's family, so the push need not ask the map.
    #[inline(always)]
    pub(crate) fn act_with_base(&mut self, resolved: Resolved<'gc>, feedback_base: FeedbackBase, receiver: Value<'gc>, superclass: Option<ClassReference<'gc>>, selector: Symbol, arguments: &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>>
```

whose body is `act`'s, with `feedback_base` in the ordinary
`Step::Method` arm instead of `NOT_ASSIGNED`, and make `act` call it with
`FeedbackBase::NOT_ASSIGNED` — so there is still one executor.

- [ ] **Step 5: Implement the three cached sends.**

```rust
/// An explicit-receiver send (§5.5), cached (`docs/design/dispatch.md`, "A
/// send, step by step").
#[inline(always)]
pub(crate) fn send_public<'gc>(
    mutator: &mut Mutator<'gc>,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some(index) = site_index(feedback_base, family_size, site) else {
        return mutator.decide_send(receiver, selector, arguments);
    };
    let key = SendKey {
        guard: mutator.heap.dispatch_id(&receiver),
        rule: CacheRule::Public,
        receiver,
        lexical_class: None,
        superclass: None,
    };
    cached(mutator, index, key, selector, arguments, |mutator| {
        mutator.resolve_public(receiver, selector)
    })
}

/// A lexical send (§5.7-§5.9), cached. The site's privacy, decided when it
/// was first filled, chooses the guard: N's application for a private site,
/// `o`'s otherwise, so a non-private hit never computes N.
#[inline(always)]
pub(crate) fn send_lexical<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    depth: u16,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let privacy = site_index(feedback_base, family_size, site).and_then(|index| {
        mutator
            .caches
            .sites
            .get(index)
            .map(|entry| (index, entry.target.site_privacy()))
    });
    let Some((index, privacy)) = privacy else {
        return decide_lexical_send(mutator, activation, depth, selector, arguments);
    };
    let (self_value, defining_class) = mutator
        .heap
        .activation_receiver_and_current_class(activation);
    let enclosing =
        dispatch::kth_enclosing_object(&mutator.heap, &self_value, defining_class.as_ref(), depth);
    let Some(private) = privacy else {
        return send_lexical_first(mutator, index, enclosing, defining_class, depth, selector, arguments);
    };
    let key = if private {
        let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
        SendKey {
            guard: lexical_class.map_or(DispatchId::NONE, |class| {
                mutator.heap.class_dispatch_ids(class).instances
            }),
            rule: CacheRule::LexicalPrivate,
            receiver: enclosing,
            lexical_class,
            superclass: None,
        }
    } else {
        SendKey {
            guard: mutator.heap.dispatch_id(&enclosing),
            rule: CacheRule::Lexical,
            receiver: enclosing,
            lexical_class: None,
            superclass: None,
        }
    };
    cached(mutator, index, key, selector, arguments, |mutator| {
        let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
        mutator.resolve_lexical(enclosing, lexical_class, selector)
    })
}

/// A vacant lexical site's first send: whether §5.9's private rule applies
/// is not known until it is resolved, so it skips the global cache, whose
/// entry for `o` may come from a site whose N declares nothing private
/// (refinement 2 of the plan that built this).
#[inline(never)]
fn send_lexical_first<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    enclosing: Value<'gc>,
    defining_class: Option<ClassReference<'gc>>,
    depth: u16,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
    let resolved = mutator.resolve_lexical(enclosing, lexical_class, selector);
    let private = resolved.chain() == Some(Chain::Private);
    let (guard, rule) = if private {
        let guard = lexical_class.map_or(DispatchId::NONE, |class| {
            mutator.heap.class_dispatch_ids(class).instances
        });
        (guard, CacheRule::LexicalPrivate)
    } else {
        (mutator.heap.dispatch_id(&enclosing), CacheRule::Lexical)
    };
    let key = SendKey { guard, rule, receiver: enclosing, lexical_class, superclass: None };
    fill(mutator, index, key, selector, &resolved);
    mutator.act(resolved, enclosing, None, selector, arguments)
}

/// A `super` send (§5.10), cached, keyed by the running method's
/// application. Its two errors are raised by the uncached path, which is
/// where a send without a `current_class` or a superclass goes.
#[inline(always)]
pub(crate) fn send_super<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let current_class = mutator.heap.activation_current_class(activation);
    let superclass = current_class.and_then(|class| mutator.heap.class_superclass(class));
    let (Some(index), Some(current_class), Some(superclass)) = (
        site_index(feedback_base, family_size, site),
        current_class,
        superclass,
    ) else {
        return decide_super_send(mutator, activation, selector, arguments);
    };
    let self_value = mutator.heap.activation_receiver(activation);
    let key = SendKey {
        guard: mutator.heap.class_dispatch_ids(current_class).instances,
        rule: CacheRule::Super,
        receiver: self_value,
        lexical_class: None,
        superclass: Some(superclass),
    };
    cached(mutator, index, key, selector, arguments, |mutator| {
        mutator.resolve_super(self_value, superclass, selector)
    })
}

/// A monomorphic hit, or everything else out of line.
#[inline(always)]
fn cached<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    resolve: impl FnOnce(&mut Mutator<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    if let Some(entry) = mutator.caches.sites.get(index) {
        if entry.guard == key.guard {
            let target = entry.target.clone();
            if let Some((resolved, base)) = materialize(mutator, &target, key) {
                return mutator.act_with_base(resolved, base, key.receiver, key.superclass, selector, arguments);
            }
        }
    }
    miss(mutator, index, key, selector, arguments, resolve)
}

/// The site's polymorphic block, then the global cache, then `resolve`,
/// whose answer is recorded in both before it is acted on. Out of line so
/// that a monomorphic hit keeps its registers; not `#[cold]`, since a
/// polymorphic hit comes through here.
#[inline(never)]
fn miss<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    resolve: impl FnOnce(&mut Mutator<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    if key.guard != DispatchId::NONE {
        if let Some(target) = mutator.caches.polymorphic_hit(index, key.guard) {
            if let Some((resolved, base)) = materialize(mutator, &target, key) {
                return mutator.act_with_base(resolved, base, key.receiver, key.superclass, selector, arguments);
            }
        }
        let global = mutator.caches.global.get(key.guard, selector, key.rule).cloned();
        if let Some(target) = global {
            if let Some((resolved, base)) = materialize(mutator, &target, key) {
                mutator.caches.advance(index, key.guard, target, key.private());
                return mutator.act_with_base(resolved, base, key.receiver, key.superclass, selector, arguments);
            }
        }
    }
    let resolved = resolve(mutator);
    fill(mutator, index, key, selector, &resolved);
    mutator.act(resolved, key.receiver, key.superclass, selector, arguments)
}

/// Records `resolved` in the global cache and at the site -- unless the key
/// has no id, or the resolution's privacy is not the site's (only a crafted
/// image makes a site's N change; refinement 1). Before `act`, which may run
/// code.
fn fill<'gc>(mutator: &mut Mutator<'gc>, index: usize, key: SendKey<'gc>, selector: Symbol, resolved: &Resolved<'gc>) {
    if key.guard == DispatchId::NONE {
        return;
    }
    if (resolved.chain() == Some(Chain::Private)) != key.private() {
        return;
    }
    if let Some(target) = record(mutator, resolved, key) {
        mutator.caches.global.insert(key.guard, selector, key.rule, target.clone());
        mutator.caches.advance(index, key.guard, target, key.private());
    }
}
```

`InlineCaches` methods the sends use (Strongtalk's and V8's transitions):

- `advance(index, guard, target, private)`, a no-op for an index `get_mut`
  does not find:
  - a `Vacant` site becomes `{guard, target}` (monomorphic);
  - a monomorphic site `{other_guard, other}` with `other_guard != guard`
    takes a block (`free_blocks.pop()`, or a new one pushed onto `pool`),
    whose first two slots become `{other_guard, other}` and
    `{guard, target}` and the rest vacant, and becomes
    `{VACANT, Polymorphic { block, private }}`;
  - a `Polymorphic { block, .. }` site fills the first vacant slot of its
    block; when none is left, it becomes `{VACANT, Megamorphic { private }}`
    and its block is vacated and pushed onto `free_blocks`;
  - a `Megamorphic` site stays so until an invalidation;
  - when the site's privacy (`site_privacy()`) is `Some(p)` with
    `p != private`, nothing changes.
- `polymorphic_hit(index, guard) -> Option<Target>`: a clone of the target
  of the block slot whose guard is `guard`, when the site is polymorphic.
- `Target::site_privacy() -> Option<bool>`: `Vacant` → `None`;
  `Polymorphic { private, .. }` and `Megamorphic { private }` →
  `Some(private)`; any other target → `Some(chain == Chain::Private)`.

**`vm.rs`**: `execute` has `code` and `feedback_base`; the four send arms
become

```rust
        Instruction::SendPublic { selector, receiver_register, argument_registers, destination, call_site } => {
            let receiver = read_local(&mutator.heap, activation, *receiver_register);
            let family_size = code.family_size;
            send(mutator, activation, program_counter, argument_registers, *destination, floor, false,
                |mutator, arguments| send_public(mutator, feedback_base, family_size, *call_site, receiver, *selector, arguments))
        }
```

`SendInlinable` stays `mutator.decide_send(..)` (the reference lowering's
only, uncached by construction); `SendLexical` and `SendSuper` call
`send_lexical`/`send_super`. The module doc comment of `vm.rs` names
`inline_cache::send_*` beside the `decide_*` functions. No other change.

- [ ] **Step 6: The test view.** In `inline_cache.rs`:

```rust
impl InlineCaches {
    /// The states of the sites in `code`'s family that send `sent`, in
    /// emission order, or all vacant when the family has not run. For tests
    /// (`Interpreter::call_site_states`).
    pub(crate) fn states_of(&self, code: &Rc<CompiledCode>, sent: Symbol) -> Vec<CallSiteState> { .. }
}
```

walking `code` and its nested blocks with an explicit stack in emission
order (a block's sends before the `MakeClosure`'s successors: push nested
bodies and visit them where the `MakeClosure` is — or simply collect
`(number, selector)` pairs from the whole family and sort by number, which is
emission order), keeping sends whose selector is `sent`, and translating each
site's entry (and a polymorphic block's filled entries) into the public view:
`Method` → `Method(method.selector)`, `Slot` → `Slot { field }`,
`NestedClass` → `NestedClass(mixin.name)`, `Primitive` → `Primitive`,
`Factory` → `Factory` whatever its chain, and any other target on a handler
chain (`DoesNotUnderstand`, `SuperDoesNotUnderstand`) → `DoesNotUnderstand`
whatever its kind; `private` is `chain == Chain::Private`.

On `Interpreter`:

```rust
    /// The inline-cache states of `method`'s sends of `sent`, `method` looked
    /// up on `class`'s own mixin (`side` chooses the class side). For tests
    /// only; empty when `class` is not a class object or has no such method.
    pub fn call_site_states(&mut self, class: &Root, side: MethodSide, method: Symbol, sent: Symbol) -> Vec<CallSiteState> {
        self.enter(|mutator| {
            let Value::Object(object) = mutator.fetch(class) else { return Vec::new() };
            let Some(application) = mutator.heap.class_application(object) else { return Vec::new() };
            let mixin = match side {
                MethodSide::Instance => Some(application.mixin.clone()),
                MethodSide::Class => application.mixin.class_side.clone(),
            };
            let Some(code) = mixin.and_then(|mixin| mixin.compiled_method(method).map(|found| found.code.clone())) else {
                return Vec::new();
            };
            mutator.caches.states_of(&code, sent)
        })
    }
```

Re-export `CallSiteState`, `CachedTarget`, `CachedTargetKind` and
`MethodSide` from `lib.rs`.

- [ ] **Step 7: Run everything.**

Run: `cargo test --workspace`
Expected: PASS — the new tests, and every fixture, golden and corpus answer
unchanged under both lowerings. A fixture that now differs between the
lowerings is a cache defect: find it with `NS_NEXT_STRESS_MOVES` off, by
bisecting the program, never by changing an expectation.

- [ ] **Step 8: Audit `resolve` for reads the key does not cover.** Go through
  every line `resolve_*` and the functions it calls execute, and write the
  result into the task report as a table (read, covered by, or re-checked
  in `act`). The plan's own audit, to check against:

| Read | Covered by |
|---|---|
| the receiver's class (`class_of`) | an instance's application is its class's (`dispatch_id`); every class object's class is the one placeholder |
| `is_class(receiver)` | the class-side id differs from every instance id |
| the factory selector (`try_factory_send`, `is_factory_send`) | the class object's application; **re-checked** in `act` for `Factory` and, under `super`, for every handler target |
| `ClassData.builtin` (Array, OrderedCollection) | each builtin has its own application |
| `metaclass_of(receiver)` | the metaclass's application is a function of the class object's application chain; the class object itself is recomputed per send |
| `kth_enclosing_object` | is the key's `o` |
| `kth_lexical_class`, `private_member` | the private guard is N's application; a non-private site assumes N's mixin is fixed (well-formed code), and a crafted site that breaks it gets a non-private answer, wrong but safe |
| the `super` errors (no `current_class`, no superclass) | raised before the key exists, never recorded |
| a primitive's payload check | re-run by `act` on every send; a failure continues uncached and the entry stays |
| the nested-class cache slot | read by `nested_class` on every send |
| an accessor's field, the argument count | read by `decide_slot_accessor` and `push_step` on every send |

Anything the audit finds that the table does not list is either added to
`act` as a re-check or reported.

- [ ] **Step 9: Measure.** Two sittings, by the protocol, against START,
  `be3eb40` and this task's parent (Task 6's head). The caches must make
  `bench_send.ns` and `bench_loop.ns` faster than the parent; report
  anything that is not, with the profile-guided reason if one is needed
  (re-profile one source as in Task 1).

- [ ] **Step 10: Gates and commit.**

```bash
git add -A crates
git commit -m "feat: cache every send at its call site, with a global cache behind" -m "<trailer lines>"
```

---

### Task 8: Invalidation, the cache stress mode, and the remaining pins

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`invalidate`, `statistics`, `set_global_cache_size`)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (`stress_caches_from_env`, `global_cache_entries_from_env`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Interpreter.stress_caches`, `invalidate_caches`, `cache_statistics`, `set_global_cache_size`, `set_stress_caches`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`invalidate_caches`, `cache_statistics`)
- Modify: `crates/ns-cli/src/main.rs`, `commands/bench.rs`, `commands/run.rs` (`--cache-statistics`)
- Test: `crates/ns-runtime/tests/inline_caches.rs`, `tests/stress.rs`, `tests/image.rs`,
  `crates/ns-cli/tests/run_fixtures.rs`

**Interfaces:**
- Produces: the environment variables `NS_NEXT_STRESS_CACHES` and
  `NS_NEXT_GLOBAL_CACHE_ENTRIES` (debug builds only);
  `pub struct CacheStatistics { pub site_entries: usize, pub filled_sites: usize, pub polymorphic_blocks: usize, pub polymorphic_blocks_in_use: usize, pub megamorphic_sites: usize, pub global_entries: usize, pub filled_global_entries: usize, pub entry_bytes: usize, pub global_entry_bytes: usize }`;
  `Interpreter::invalidate_caches(&mut self)`, `Interpreter::cache_statistics(&mut self) -> CacheStatistics`,
  `Interpreter::set_global_cache_size(&mut self, entries: usize)` (a power of two; vacates it),
  `#[cfg(debug_assertions)] Interpreter::set_stress_caches(&mut self, enabled: bool)`,
  `#[cfg(debug_assertions)] Interpreter::cache_invalidations(&self) -> usize`;
  the environment variable `NS_NEXT_STRESS_CACHES`.

- [ ] **Step 1: Write the failing tests.**

In `tests/inline_caches.rs` (which has `FIVE_KINDS`, `TWO_APPLICATIONS` and
the helpers from Task 7):

```rust
/// An invalidation vacates every site, every polymorphic block and the
/// global cache, and keeps every family's range, since running activations
/// keep their bases; the next run fills the sites again and answers the same
/// (`docs/design/dispatch.md`, "Invalidation").
#[test]
fn an_invalidation_vacates_every_site_and_keeps_the_ranges() {
    let (mut interpreter, class) = load(FIVE_KINDS);
    let instance = instantiate(&mut interpreter, &class);
    let five = integer_argument(&interpreter, 5);
    send(&mut interpreter, &instance, "run:", &[five]);
    let before = interpreter.cache_statistics();
    assert_eq!(before.megamorphic_sites, 1, "only the `value` site saw five kinds");
    assert!(before.filled_global_entries > 0);
    interpreter.invalidate_caches();
    let after = interpreter.cache_statistics();
    assert_eq!(after.site_entries, before.site_entries, "the ranges stay");
    assert_eq!(after.filled_sites, 0);
    assert_eq!(after.megamorphic_sites, 0);
    assert_eq!(after.polymorphic_blocks_in_use, 0);
    assert_eq!(after.filled_global_entries, 0);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run:", "value"),
        vec![CallSiteState::Vacant]
    );
    let one = integer_argument(&interpreter, 1);
    let answer = send(&mut interpreter, &instance, "run:", &[one]);
    assert_eq!(small_integer(&mut interpreter, &answer), 1);
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run:", "value"),
        vec![CallSiteState::Monomorphic(method("value"))]
    );
}

/// A global cache of one or two entries, where inserts evict each other,
/// answers what a full one answers: the megamorphic `value` site resolves
/// through it on every send.
#[test]
fn a_tiny_global_cache_answers_like_a_full_one() {
    for entries in [1, 2] {
        let (mut interpreter, class) = load(FIVE_KINDS);
        interpreter.set_global_cache_size(entries);
        let instance = instantiate(&mut interpreter, &class);
        for _ in 0..3 {
            let five = integer_argument(&interpreter, 5);
            let answer = send(&mut interpreter, &instance, "run:", &[five]);
            assert_eq!(small_integer(&mut interpreter, &answer), 15);
        }
        assert_eq!(interpreter.cache_statistics().global_entries, entries);
    }
}

/// Only the families that ran take room in the table: `used`'s two sends
/// (`+` in its block, `do:`), and nothing for `unused` or for an initializer
/// with no sends.
#[test]
fn only_the_families_that_ran_take_room_in_the_cache_table() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new () (
          public used = (
            | sum |
            sum := 0.
            #(1 2) do: [:each | sum := sum + each].
            ^sum
          )
          public unused = ( ^1 + 2 + 3 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    assert_eq!(interpreter.cache_statistics().site_entries, 0);
    let answer = send(&mut interpreter, &instance, "used", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 3);
    assert_eq!(interpreter.cache_statistics().site_entries, 2);
}
```

In `crates/ns-cli/tests/run_fixtures.rs`:

```rust
/// Every fixture answers alike when the global cache has one entry, so that
/// every insert evicts the one before it (`NS_NEXT_GLOBAL_CACHE_ENTRIES`,
/// read by debug builds only).
#[cfg(debug_assertions)]
#[test]
fn every_fixture_answers_alike_with_a_one_entry_global_cache() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    for entry in std::fs::read_dir(&directory).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("ns") {
            continue;
        }
        let expected = std::fs::read_to_string(path.with_extension("expected"))
            .expect("every fixture has its expected output");
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&path)
            .env("NS_NEXT_GLOBAL_CACHE_ENTRIES", "1")
            .output()
            .expect("run ns-next");
        assert!(output.status.success(), "{} failed", path.display());
        assert_eq!(
            normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
            normalize_newlines(&expected),
            "{} with a one-entry global cache",
            path.display()
        );
    }
}
```

In `tests/stress.rs` (debug only, like the rest of that file), factor the
instantiate-and-send part of `run_every_continuation` into

```rust
/// Instantiates `class` and answers what its `run` answers.
fn run_sum(interpreter: &mut Interpreter, class: &Root) -> i64 {
    let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
    let sum = expect_ok(interpreter.send(&instance, Symbol::intern("run"), &[]), "run");
    interpreter.enter(|mutator| match mutator.fetch(&sum) {
        Value::SmallInteger(sum) => sum,
        _ => panic!("run answers a small integer"),
    })
}
```

(use it in `run_every_continuation` too), and add

```rust
/// The cache stress mode vacates every cache at every safepoint, so filling,
/// transitions and invalidation all run under a program with every kind of
/// continuation; its answer is the one without the mode.
#[test]
fn the_cache_stress_mode_invalidates_at_every_safepoint_and_changes_no_answer() {
    let (mut calm_interpreter, calm_class) = load(EVERY_CONTINUATION);
    let calm = run_sum(&mut calm_interpreter, &calm_class);
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_caches(true);
    let stressed = run_sum(&mut interpreter, &class);
    assert_eq!(calm, stressed);
    assert!(
        interpreter.cache_invalidations() > 10,
        "an invalidation at every safepoint of the run"
    );
}
```

In `tests/image.rs` (which has `load`, `expect_ok`, `class_reference` and
imports `image`, `Origin` and `PathBuf`):

```rust
/// A snapshot taken with the caches full loads with them empty -- no cache
/// state and no base is serialized -- and a decoded closure is given a fresh
/// range of its family's size when it is first activated.
#[test]
fn a_snapshot_with_full_caches_loads_with_empty_ones() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Keeper = Object new (
            | keep ::= nil. |
        ) (
            public prepare = ( keep := [:each | each + 1]. ^self )
            public apply: value = ( ^keep value: value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(interpreter.send(&instance, Symbol::intern("prepare"), &[]), "prepare");
    let one = interpreter.stash(Value::SmallInteger(1));
    expect_ok(interpreter.send(&instance, Symbol::intern("apply:"), &[one]), "apply:");
    assert!(interpreter.cache_statistics().filled_sites > 0);
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        let instance = mutator.fetch(&instance);
        image::snapshot(
            mutator,
            None,
            &Origin::File(PathBuf::from("keeper.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });
    let mut loaded = image::load(&bytes).expect("load");
    assert_eq!(loaded.interpreter.cache_statistics().site_entries, 0);
    let forty_one = loaded.interpreter.stash(Value::SmallInteger(41));
    let answer = expect_ok(
        loaded
            .interpreter
            .send(&loaded.roots[0], Symbol::intern("apply:"), &[forty_one]),
        "apply: after load",
    );
    let answer = loaded.interpreter.enter(|mutator| match mutator.fetch(&answer) {
        Value::SmallInteger(integer) => integer,
        _ => panic!("apply: answers a small integer"),
    });
    assert_eq!(answer, 42);
    // `apply:`'s family is two sends (`keep`, `value:`); the decoded block's
    // is one (`+`, numbered 0 in `prepare`'s family, before the `keep:`
    // setter that takes the block as its argument).
    assert_eq!(loaded.interpreter.cache_statistics().site_entries, 3);
}
```

If the last figure is not 3, check the two families' sizes with
`ns-next disasm` on the source before changing it; the property under test is
that only `apply:`'s family and one fresh range for the closure were added.

- [ ] **Step 2: Run them to see them fail.**

Run: `cargo test -p ns-runtime --test inline_caches`,
`cargo test -p ns-runtime --test stress cache_stress`,
`cargo test -p ns-runtime --test image a_snapshot_with_full_caches` and
`cargo test -p ns-cli --test run_fixtures one_entry`
Expected: FAIL to compile.

- [ ] **Step 3: Implement.**

`InlineCaches::invalidate`: every `sites` entry vacant, `global.clear()`,
`pool.clear()`, `free_blocks.clear()`; `families` and `family_codes` kept.
`statistics()` fills `CacheStatistics` (`entry_bytes` is
`size_of::<CacheEntry>()`, `global_entry_bytes` `size_of::<GlobalEntry>()`,
`filled_sites` counts sites whose target is neither `Vacant`, `Polymorphic`
nor `Megamorphic`, plus polymorphic sites; `megamorphic_sites` the
megamorphic ones). `set_global_cache_size(entries)` replaces the global cache
with `GlobalCache::with_entries(entries)`; an `entries` that is not a power of
two is a host's mistake and panics with a message saying so (it is a test
hook, not an input path).

In a debug build, `Interpreter::bootstrap_with` also reads
`NS_NEXT_GLOBAL_CACHE_ENTRIES`: a decimal power of two sets the global
cache's size, anything else is ignored.

```rust
/// A global-cache size the environment asks for, for the fixture run with
/// a cache small enough that every insert evicts (`run_fixtures.rs`).
/// Debug builds only.
#[cfg(debug_assertions)]
pub(crate) fn global_cache_entries_from_env() -> Option<usize> {
    std::env::var("NS_NEXT_GLOBAL_CACHE_ENTRIES")
        .ok()?
        .parse::<usize>()
        .ok()
        .filter(|entries| entries.is_power_of_two())
}
```

lives in `safepoint.rs` beside the stress switches.

`safepoint.rs`: the stress schedule gains a sibling

```rust
/// Whether `NS_NEXT_STRESS_CACHES` asks for the cache stress mode: set, and
/// neither empty nor `0`. Debug builds only. It vacates every inline cache
/// at every safepoint, so that filling, transitions and invalidation run
/// under the whole suite long before edit-and-continue needs them
/// (`docs/design/dispatch.md`, "Invalidation"). Not combined with the move
/// stress mode as a gate: an entry holds no handle for a move to break, and
/// emptied caches would leave the moves hardly a hit to run under.
#[cfg(debug_assertions)]
pub(crate) fn stress_caches_from_env() -> bool {
    std::env::var_os("NS_NEXT_STRESS_CACHES").is_some_and(|value| !value.is_empty() && value != "0")
}
```

`Interpreter` gets `#[cfg(debug_assertions)] stress_caches: bool` and
`#[cfg(debug_assertions)] cache_invalidations: usize`; `bootstrap_with`
sets the safepoint request when either mode is on. `safepoint` becomes:

```rust
    pub(crate) fn safepoint(&mut self) {
        #[cfg(debug_assertions)]
        {
            let mut stressed = false;
            if self.stress_caches {
                self.world.invalidate_caches();
                self.cache_invalidations += 1;
                stressed = true;
            }
            if let Some(stress) = &mut self.stress {
                if stress.due(self.world.traced_size()) {
                    self.world.move_everything();
                }
                stressed = true;
            }
            if stressed {
                return;
            }
        }
        self.world.clear_safepoint_request();
    }
```

`set_stress_caches(enabled)` sets the flag and the request
(`enabled || self.stress.is_some()`); `set_stress_moves` likewise keeps the
request set while `stress_caches` is on.

The CLI: `Run` and `Bench` get

```rust
        /// Print the inline caches' sizes to stderr after the run.
        #[arg(long)]
        cache_statistics: bool,
```

and, when set, print after the run (for `bench`, after the timed runs):

```
cache: <site_entries> site entries (<filled_sites> filled, <megamorphic_sites> megamorphic), <polymorphic_blocks_in_use>/<polymorphic_blocks> polymorphic blocks, <filled_global_entries>/<global_entries> global entries; entry <entry_bytes> bytes, global entry <global_entry_bytes> bytes
```

`run --cache-statistics` writes to stderr, so no fixture or golden sees it.

- [ ] **Step 4: Run everything, then the new gate.**

```bash
cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Record the stress suite's run time (Git Bash `time`) for Task 10; the design
expects it well below the move suite's three minutes.

- [ ] **Step 5: Gates and commit.** From here on the cache stress runs are
  part of every task's gates (Global Constraints).

```bash
git add -A crates
git commit -m "feat: invalidate the inline caches, and stress them at every safepoint" -m "<trailer lines>"
```

---

### Task 9: Measure the step whole, and the entry's layout

**Files:**
- Modify: nothing committed but, if a variant wins, the code it changes.

**Interfaces:** Consumes the whole branch; produces the tables Task 10
writes into `execution.md`.

- [ ] **Step 1: Measure the step.** Two sittings, by the protocol, against
  START and `be3eb40`. Also run, with the branch's release binary, every
  source with `bench --cache-statistics --iterations 30` and every
  whitelisted corpus module (`corpus/runnable.toml`) with
  `run --cache-statistics --send <its entry>`, and record the lines.

- [ ] **Step 2: Measure a 16-byte entry.** On a throwaway branch
  `experiment/sixteen-byte-entry` from the branch's head: `Target::Method`
  and `Target::NestedClass` hold a `u32` index into two `Vec`s on
  `InlineCaches` (`methods: Vec<Rc<MixinMethod>>`,
  `mixins: Vec<Rc<Mixin>>`, appended on record, cleared on invalidate)
  instead of the `Rc`; `Target` becomes 12 bytes and `CacheEntry` 16, which
  the size pins then say. Measure it against the branch's head in two
  sittings. The proposed rule, for the user to confirm: the faster wins;
  within noise, the smaller. Report both tables and the rule's outcome; if the
  16-byte entry wins and the user agrees, bring the change onto the branch as
  a `perf:` commit, with the pins updated; either way delete the experiment
  branch.

- [ ] **Step 3: The open finding.** PROGRESS.md's "A lexical send's decision
  is sensitive to codegen": the lexical hit path replaced that decision.
  Report whether `bench_send.ns` and `bench_block.ns` (the lexical-heavy
  sources) now move with the cache rather than with codegen — the two
  sittings' spread against the earlier steps' — so Task 10 can close it or
  carry it forward with the result.

- [ ] **Step 4: Report.** Everything above goes into the task report, with
  both sittings' tables in the shape of `execution.md`'s "What primitives as
  method-table entries moved". No commit unless Step 2 brought one.

---

### Task 10: Documents

**Files:**
- Modify: `docs/design/dispatch.md`, `docs/design/compiler.md`,
  `docs/design/image-format.md`, `docs/design/execution.md`,
  `docs/design/memory.md`, `docs/design/README.md`, `docs/PROGRESS.md`,
  `CLAUDE.md`

**Interfaces:** Consumes every task's report.

- [ ] **Step 1: `dispatch.md`.** "Planned: inline caches" says 3a and 3b are
  built. "3b — The caches" is rewritten from "Planned" to what the code does
  — present tense, the refinements of this plan folded in (the private guard,
  the vacant lexical site, the `super` handler re-check, a block's
  `family_size`, dense numbering), the real sizes (entry, block, global
  entry, global cache), the chains (`Chain`) and `Resolved`/`Target`, and
  `site_index`'s rule. "Decisions" gets 3b as built. "The send machinery"
  names `inline_cache.rs` and `resolve.rs`.
- [ ] **Step 2: `compiler.md`.** "Safepoints and call-site identity" describes
  family numbering, `family_size`, `UNCACHED`, and the reference lowering as
  the uncached oracle, present tense; the ceiling paragraph about the `u16`
  goes.
- [ ] **Step 3: `image-format.md`.** "The instruction encoding" lists 21–24
  for the sends, 7–10 as retired and why, the send's `u32` site, the
  code record's empty call-site list; "Verification" adds
  `CallSiteListNotEmpty` and `CallSitesNotDense` and what each guarantees;
  "What is not serialized" names cache state and `feedback_base`. Record the
  golden image's size change.
- [ ] **Step 4: `execution.md`.** A section "What the inline caches moved"
  after "What primitives as method-table entries moved": Task 5's, 6's, 7's
  and 9's tables, the variants tried and kept or rejected, the sizes after
  the benchmarks and the corpus, the stress suite's run time. "What an
  activation carries" names `feedback_base`.
- [ ] **Step 5: `memory.md`.** "What the collector traces": the caches hold no
  handle (an entry holds a guard id, `Rc`s and numbers), so nothing is traced
  there; `ClosureData.feedback_base` and `ClassData.dispatch_ids` are not
  handles. Update any object-size figure Task 3 changed.
- [ ] **Step 6: `CLAUDE.md`.** "Working in this repository" gains:

```bash
# the same suite with every inline cache vacated at every safepoint
# (debug builds only; Git Bash syntax)
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
# non-regression gate under the cache stress mode, same report
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

and the invariants section gains, after "`outer` walks the lexical
declaration chain": **A cache records what the lookup decided; it never
decides.** — a short paragraph: a hit is `act` on what `resolve` answered
for the same key, an entry holds no handle, anything `resolve` reads that
the key does not cover is re-checked in `act`, and the reference lowering
runs uncached as the oracle.
- [ ] **Step 7: `README.md` and `PROGRESS.md`.** The design map's status for
  dispatch; PROGRESS.md: an "M4 — Inline caches" milestone section in the
  shape of the earlier ones; "Where the next session starts" says 3b is done
  and names 3c's plan as the next piece (measured against 3b, the user decides
  on the numbers); the open findings updated (the lexical codegen finding
  closed or carried forward with Task 9's result; the stress suite's time
  with both modes).
- [ ] **Step 8: Check that a fresh session can start from the documents.**
  Read `CLAUDE.md`, `docs/design/README.md` and PROGRESS.md's "Where the next
  session starts" as a newcomer would, and fix whatever does not lead to 3c.
- [ ] **Step 9: Commit.**

```bash
git add CLAUDE.md docs
git commit -m "docs: record the inline caches as built and measured" -m "<trailer lines>"
```

---

## Self-review against the design

`dispatch.md`, "3b — The caches", section by section:

- **The state lives per heap** — Task 5 (`InlineCaches` on `Mutator`,
  `feedback_base` on activations and `Flow::Switch`), measured on its own in
  Task 5.
- **The dispatch id** — Task 3, two per application, copied into
  `ClassData`, the immediates' table, `Heap::dispatch_id` the one reader.
  Decoded applications go through `apply_mixin`; `set_class_application`
  keeps the copy equal.
- **An entry** — Task 7 (`Target`), with the chain in place of "class's or
  metaclass's"; `DoesNotUnderstand` is a chain rather than a variant.
- **A site's states** — Task 7 (`advance`), the pool and free list; the
  profile the optimizing tiers read is `call_site_states`' source.
- **The global cache** — Task 7, two probes, keyed by (id, selector, rule);
  the size differs and is reported (Task 10).
- **Code families** — Task 4 (numbering, `family_size`), Task 5 (ranges,
  blocks copying the base, decoded closures, initializers and clauses by the
  map). The `Factory`-entry carrying bases is not built: the design makes it
  conditional on the instantiation benchmarks showing the map, which Task 5's
  and 7's measurements report.
- **Resolve, then act** — Task 6, with the audit in Task 7, Step 8.
- **A send, step by step** — Task 7, Step 5; `vm.rs` passes base, family size
  and site only.
- **The private rule** — Task 7 with refinement 1.
- **No new error text** — Task 6 moves the texts word for word.
- **Invalidation** and **the stress mode** — Task 8.
- **Bytecode and image** — Task 4.
- **Verification** — Task 2 (outcomes under both lowerings) and Tasks 7–8
  (every listed targeted test: shared entry, transitions, bimorphic, slot
  under two applications, `super` in a mixin applied twice, private site,
  class objects, the `super` factory re-check, the three handlers, a failing
  primitive, mid-run invalidation (the stress test), snapshot and decoded
  closure, collisions, the tags).
- **Measurement** — Task 1 (profile), Tasks 5–7 (per part), Task 9 (whole,
  entry layout, sizes, the open finding).
