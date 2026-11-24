# Mark-Compact Collector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give ns-next a stop-the-world mark-compact collector over both arenas, so that a slot a program can no longer reach is reclaimed and the arenas stop growing with everything the process has ever allocated.

**Architecture:** Marking is a second `Tracer` over the one edge enumeration in `trace.rs`, driven from the roots by an iterative work list, with mark bits in a side table. Compaction slides the live slots of each arena to the front, truncates, and rewrites every handle through a flat `Vec<u32>` forwarding table — the same `Forwarder` the move stress mode already uses, with its mapping made a parameter. A collection is requested by comparing an arena's length against a limit at allocation time and is served only at a safepoint of the run loop.

**Tech Stack:** Rust 1.98.0, edition 2021, no new dependencies. `crates/ns-runtime` only, plus one flag and one printer in `crates/ns-cli`.

**Spec:** `docs/design/memory.md` — read "The shape: mark, compact, forward" through "How the collector is measured" before starting, and "Checked roots and a collector" for the machinery this builds on.

## Global Constraints

- **Every rule in `CLAUDE.md` applies.** Read it first. The ones this plan touches most: object, frame and activation access goes through `heap.rs`; a handle means nothing against a `Heap` that did not mint it; a running activation is an `ActivationReference` and only a `FrameReference` is ever stored; never `{:?}` a `Value` and never `.unwrap()`/`.expect()` an `EvaluationResult`.
- **Names are written out.** No identifier of one or two characters and no truncated word, in code or in a commit message. `clippy.toml`'s `min_ident_chars` (threshold 2, inclusive) refuses the short ones.
- **All code, identifiers, comments, doc comments and commit messages are in English.**
- **The crate forbids unsafe code** (`crates/ns-runtime/src/lib.rs:4`). Nothing in this plan needs any.
- **Add no external dependency.**
- **Conventional commits** (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`), and every commit ends with:

  ```
  Co-Authored-By: <the model that wrote the code> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01MTwLcp5EGCDDWRRGA5DCAf
  ```

- **The gates, run before every commit:**

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets      # must be warning-free
  cargo test --workspace
  NS_NEXT_STRESS_MOVES=1 cargo test --workspace
  NS_NEXT_STRESS_CACHES=1 cargo test --workspace
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 210, failed 0, expected-fail 1, stale 0"
  cargo run -p ns-cli -- run-corpus corpus/modules     # "ran 6, failed 0, skipped 205"
  NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
  NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
  ```

  From Task 7 on, `NS_NEXT_STRESS_COLLECT=1` joins both lists.
- **A golden's recorded outcome is never updated because the code now produces it.** A change that moves one is a behaviour change: establish the new output is right first. No task in this plan should move any golden.
- **The Newspeak sources in this plan's tests are statements of intent, not verified syntax.** They were written before any of them was run, and Task 3 found the first one did not parse. Two facts established there, by reading `crates/ns-runtime/src/builtins/`: `timesRepeat:` and `to:do:` are **not** in the bare bootstrap, while `whileTrue:`, `==`, `add:`, `printString` and the `OrderedCollection` class are. A module also needs its header — `Newspeak3`, the `'T'` tag, `class X = Object new () ( ... ) : ()`. The working template is the fixture in `crates/ns-runtime/tests/collector.rs` that Task 3 left behind: copy its shape, keep each test's stated intent (what is allocated, what is kept, what is discarded), rewrite the iteration as `whileTrue:`, and say in your report what you changed and why. Adapting the source is expected; changing what the test asserts is not.

- **Never judge performance from a debug build.** Only Task 9 measures, and it follows the protocol in `docs/design/execution.md`, "What removing the re-entries moved".

---

## File Structure

**Created:**

- `crates/ns-runtime/src/heap/mark.rs` — `MarkBits`, the `Marker` tracer and its work lists. A child of `heap`, so it may name `ObjectReference`'s private `index`; it holds no `Heap`, because the marking loop has to borrow the arena while the bits are borrowed.
- `crates/ns-runtime/tests/collector.rs` — the collector's behaviour through a real `Interpreter`: what survives, what does not, and what a collection does to the arena sizes.

**Modified:**

- `crates/ns-runtime/src/lib.rs` — `trace` stops being a debug-only module.
- `crates/ns-runtime/src/heap.rs` — `Relocation` and the generic `Forwarder`, `ForwardingTable`, the compaction, the collection limits, the weak symbol table, `heap_statistics`.
- `crates/ns-runtime/src/heap/world.rs` — `World::collect`, beside `World::move_everything`.
- `crates/ns-runtime/src/interpreter/mod.rs` — the collector's statistics on `Interpreter`, `collect_now`, the collect stress mode's field.
- `crates/ns-runtime/src/interpreter/safepoint.rs` — the safepoint serves a collection request; `StressCollections`.
- `crates/ns-cli/src/main.rs`, `crates/ns-cli/src/entry.rs`, `crates/ns-cli/src/commands/bench.rs` — `--collector-statistics`.
- `CLAUDE.md`, `docs/design/memory.md`, `docs/design/execution.md`, `docs/PROGRESS.md` — the third gate, the measurement, the status.

---

### Task 1: The forwarding mapping becomes a parameter, and the tracing exists in every build

Today `trace.rs`, `Forwarder` and `Rotation` are all `#[cfg(debug_assertions)]`, and `Forwarder` applies a `Rotation` it owns. The collector needs the same `Forwarder` in a release build, applying a forwarding table instead. This task is a refactor with no behaviour change: its gate is the existing suite, in both profiles.

The mapping is a generic parameter rather than an enum, because `apply` runs once per handle in the whole heap — a monomorphized call has no branch and no indirection there.

**Files:**
- Modify: `crates/ns-runtime/src/lib.rs:23-24`
- Modify: `crates/ns-runtime/src/heap.rs` (`Rotation`, `Forwarder`, `move_everything`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`World::move_everything`)
- Modify: `crates/ns-runtime/src/trace.rs` (module doc comment)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub(crate) trait Relocation { fn apply(&self, index: u32) -> u32; }` in `heap.rs`
  - `pub(crate) struct Forwarder<M: Relocation>` with `objects: M`, `frames: M`, and the debug-only `heap_id`, `from_epoch`, `to_epoch`
  - `Rotation` implements `Relocation` (debug builds only)
  - `Heap::move_everything(&mut self) -> Forwarder<Rotation>` (debug builds only, unchanged behaviour)

- [ ] **Step 1: Make `trace` unconditional**

In `crates/ns-runtime/src/lib.rs`, remove the `#[cfg(debug_assertions)]` above `pub(crate) mod trace;`. In `crates/ns-runtime/src/heap.rs:580`, remove the `#[cfg(debug_assertions)]` above `impl crate::trace::Trace for StackEntry<'_>` as well: the collector marks the activation stack through that impl in every build, and Task 2's `begin_marking` will not compile in a release build while it is gated. In `crates/ns-runtime/src/trace.rs`, replace the last line of the module comment — "Debug builds only, until the collector (step 5) needs it in every build." — with:

```rust
//! In every build: the collector marks through the same enumeration the
//! stress mode's forwarding walks, which is what keeps the two from
//! disagreeing about what holds a handle.
```

- [ ] **Step 2: Run the release build to see what is now dead**

Run: `cargo build --workspace --release`
Expected: it compiles. If `dead_code` warnings name items in `trace.rs`, they are reached again in Task 2; add no `allow` — instead finish Step 3, which is what release code will call.

- [ ] **Step 3: Introduce `Relocation` and make `Forwarder` generic**

In `crates/ns-runtime/src/heap.rs`, above the `Rotation` definition:

```rust
/// How a move maps an old slot index to its new one. Two answer it: the
/// stress mode's `Rotation`, which keeps every slot, and the collector's
/// `ForwardingTable`, which keeps only the live ones. A type parameter
/// rather than an enum or a trait object, because `apply` runs once per
/// handle in the whole heap and a monomorphized call leaves neither a
/// branch nor an indirection there.
pub(crate) trait Relocation {
    fn apply(&self, index: u32) -> u32;
}
```

Change `Rotation`'s inherent `apply` into the trait implementation, keeping `#[cfg(debug_assertions)]` on it — a rotation exists only for the stress mode:

```rust
#[cfg(debug_assertions)]
impl Relocation for Rotation {
    fn apply(&self, index: u32) -> u32 {
        if self.length == 0 {
            return index;
        }
        ((u64::from(index) + u64::from(self.shift)) % u64::from(self.length)) as u32
    }
}
```

Make `Forwarder` generic and un-gate it, keeping the checks debug-only:

```rust
/// The tracer a move or a collection forwards every handle with. In a debug
/// build it also checks each handle it forwards: one from another heap, or
/// one already stale, was stored in this heap by a mistake the rewrite would
/// otherwise make permanent.
pub(crate) struct Forwarder<M: Relocation> {
    #[cfg(debug_assertions)]
    heap_id: u32,
    #[cfg(debug_assertions)]
    from_epoch: u32,
    #[cfg(debug_assertions)]
    to_epoch: u32,
    objects: M,
    frames: M,
}

impl<M: Relocation> crate::trace::Tracer for Forwarder<M> {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                handle.origin, self.heap_id,
                "a handle from another heap is stored in this one"
            );
            assert_eq!(
                handle.epoch, self.from_epoch,
                "a stale handle is stored in the heap: minted in move epoch {}, found during move {}",
                handle.epoch, self.to_epoch
            );
            handle.epoch = self.to_epoch;
        }
        handle.index = self.objects.apply(handle.index);
    }

    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                handle.origin, self.heap_id,
                "a handle from another heap is stored in this one"
            );
            assert_eq!(
                handle.epoch, self.from_epoch,
                "a stale handle is stored in the heap: minted in move epoch {}, found during move {}",
                handle.epoch, self.to_epoch
            );
            handle.epoch = self.to_epoch;
        }
        handle.index = self.frames.apply(handle.index);
    }
}
```

`Heap::move_everything` keeps its `#[cfg(debug_assertions)]` and now names its answer `Forwarder<Rotation>`; `World::move_everything` names the same type where it holds one.

- [ ] **Step 4: Run every gate in both profiles**

Run, in order:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo build --workspace --release
cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: clippy warning-free, every suite green, `parsed 210, failed 0, expected-fail 1, stale 0`, `ran 6, failed 0, skipped 205` three times. No golden moved.

- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime/src/lib.rs crates/ns-runtime/src/heap.rs \
        crates/ns-runtime/src/heap/world.rs crates/ns-runtime/src/trace.rs
git commit -F <message file>
```

Subject: `refactor: make the forwarding mapping a parameter and the tracing unconditional`

---

### Task 2: Mark bits and the marker

Liveness, with nothing reclaimed yet: mark every object and frame reachable from the roots, and answer the bits. The marking loop lives on `Heap` because it has to lend the marker one arena record at a time while the bits and work lists are borrowed beside the arena.

**Files:**
- Create: `crates/ns-runtime/src/heap/mark.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (declare the child module; `begin_marking`, `drain_marking`; tests)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`World::mark_everything_reachable`)

**Interfaces:**
- Consumes: `crate::trace::{Trace, Tracer}` from Task 1.
- Produces:
  - `pub(crate) struct MarkBits` with `fn new(length: usize) -> MarkBits`, `fn set(&mut self, index: u32) -> bool`, `fn is_set(&self, index: u32) -> bool`, `fn count(&self) -> usize`
  - `pub(crate) struct Marker` with `objects: MarkBits`, `frames: MarkBits`, implementing `Tracer`
  - `Heap::begin_marking(&mut self) -> Marker` — marks the heap's own roots, drains nothing
  - `Heap::drain_marking(&mut self, marker: &mut Marker)` — runs the work lists empty
  - `World::mark_everything_reachable(&mut self) -> Marker` — begins, adds the step state's roots, drains

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/ns-runtime/src/heap.rs`:

```rust
/// Liveness is reachability from the roots, and the roots are what the
/// heap holds, not what a Rust local happens to name. `unreachable` is a
/// perfectly valid object that nothing can get to, which is exactly the
/// case the collector exists for.
#[test]
fn marking_reaches_what_a_root_reaches_and_nothing_else() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
    let reachable = heap.allocate_object(class, ObjectData::String("live".to_string()));
    let unreachable = heap.allocate_object(class, ObjectData::String("dead".to_string()));
    let activation = heap.push_activation(header(), &[Value::Object(reachable)], 1);

    let marker = heap.mark_from_roots_for_test();

    assert!(marker.objects.is_set(reachable.index));
    assert!(marker.objects.is_set(class.index));
    assert!(!marker.objects.is_set(unreachable.index));
    heap.pop_activation(activation);
}

/// An edge is followed, not just a root: the value inside a reachable
/// instance keeps its own target alive.
#[test]
fn marking_follows_an_edge_out_of_a_reachable_object() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
    let target = heap.allocate_object(class, ObjectData::String("target".to_string()));
    let holder = heap.allocate_object(class, ObjectData::Instance(vec![Value::Object(target)]));
    let activation = heap.push_activation(header(), &[Value::Object(holder)], 1);

    let marker = heap.mark_from_roots_for_test();

    assert!(marker.objects.is_set(target.index));
    heap.pop_activation(activation);
}

/// A reified frame is reachable from the entry it is married to, which is
/// what makes "a running activation's frame is never collected" fall out
/// of the root set instead of needing a rule of its own.
#[test]
fn marking_reaches_a_married_frame_through_its_entry() {
    let mut heap = Heap::new();
    let activation = heap.push_activation(header(), &[], 1);
    let frame = heap.reify(activation);

    let marker = heap.mark_from_roots_for_test();

    assert!(marker.frames.is_set(frame.index));
    heap.pop_activation(activation);
}
```

Add the test-only entry point beside them, inside `impl<'gc> Heap<'gc>`:

```rust
/// Marks from the heap's own roots and drains, for the heap's own tests.
/// A real collection goes through `World`, which owns the step state's
/// roots as well.
#[cfg(test)]
fn mark_from_roots_for_test(&mut self) -> crate::heap::mark::Marker {
    let mut marker = self.begin_marking();
    self.drain_marking(&mut marker);
    marker
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --lib heap::tests::marking`
Expected: FAIL — `begin_marking`, `drain_marking` and the `mark` module do not exist.

- [ ] **Step 3: Write `heap/mark.rs`**

```rust
//! Mark bits and the marker (`docs/design/memory.md`, "Marking").
//!
//! A child of `heap`, so it may name a handle's private index. It holds no
//! `Heap`: the marking loop is `Heap::drain_marking`, which lends the
//! marker one arena record at a time, so the bits and the work lists are
//! borrowed beside the arena rather than through it.

use crate::heap::{FrameReference, ObjectReference};
use crate::trace::Tracer;

/// One bit per slot of one arena -- 1/448 of the object arena, which is why
/// the bit is here and not in the object header: `Object` is pinned at 56
/// bytes by `an_object_is_the_size_the_amplification_figure_assumes`, and
/// the image format's amplification figures are computed from that number.
pub(crate) struct MarkBits {
    words: Vec<u64>,
    length: usize,
}

impl MarkBits {
    pub(crate) fn new(length: usize) -> MarkBits {
        MarkBits {
            words: vec![0; length.div_ceil(64)],
            length,
        }
    }

    /// Sets the bit for `index` and answers whether it had been clear.
    /// That answer is what keeps the traversal from visiting an object
    /// twice, and so from running forever on a cycle.
    fn set(&mut self, index: u32) -> bool {
        let index = index as usize;
        debug_assert!(index < self.length, "internal: a handle past its arena");
        let word = index / 64;
        let mask = 1u64 << (index % 64);
        let was_clear = self.words[word] & mask == 0;
        self.words[word] |= mask;
        was_clear
    }

    pub(crate) fn is_set(&self, index: u32) -> bool {
        let index = index as usize;
        debug_assert!(index < self.length, "internal: a handle past its arena");
        self.words[index / 64] & (1u64 << (index % 64)) != 0
    }

    pub(crate) fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }
}

/// The tracer that marks. Marking and forwarding share one edge
/// enumeration (`trace.rs`), so a field a later change adds is named in
/// exactly one place and cannot be missed by one of the two.
pub(crate) struct Marker {
    pub(crate) objects: MarkBits,
    pub(crate) frames: MarkBits,
    object_work: Vec<u32>,
    frame_work: Vec<u32>,
}

impl Marker {
    pub(crate) fn new(object_count: usize, frame_count: usize) -> Marker {
        Marker {
            objects: MarkBits::new(object_count),
            frames: MarkBits::new(frame_count),
            object_work: Vec::new(),
            frame_work: Vec::new(),
        }
    }

    /// The next record to visit, or `None` when the traversal is done. The
    /// work lists are explicit because the graph's depth is the program's
    /// to choose -- the reasoning that already made `MixinApplication`'s
    /// `Drop` iterative.
    pub(crate) fn next(&mut self) -> Option<Work> {
        if let Some(index) = self.object_work.pop() {
            return Some(Work::Object(index));
        }
        self.frame_work.pop().map(Work::Frame)
    }
}

/// One arena record the marker still has to visit.
pub(crate) enum Work {
    Object(u32),
    Frame(u32),
}

impl Tracer for Marker {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        if self.objects.set(handle.index) {
            self.object_work.push(handle.index);
        }
    }

    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        if self.frames.set(handle.index) {
            self.frame_work.push(handle.index);
        }
    }
}
```

- [ ] **Step 4: Write `begin_marking` and `drain_marking`**

Declare the module in `crates/ns-runtime/src/heap.rs`, beside `world`:

```rust
pub(crate) mod mark;
```

Add to `impl<'gc> Heap<'gc>`, beside `move_everything`:

```rust
/// Marks the heap's own roots -- the activation stack's entries and its
/// registers, the canonical symbol table and the host roots -- and answers
/// the marker without draining it, so that the caller can add the step
/// state's roots (`World::mark_everything_reachable`) before the traversal
/// runs.
///
/// The canonical symbol table is a root here and stops being one when it
/// becomes weak; the arenas are never roots, only what the roots reach.
pub(crate) fn begin_marking(&mut self) -> mark::Marker {
    use crate::trace::{Trace, Tracer};
    let mut marker = mark::Marker::new(self.objects.len(), self.frames.len());
    for entry in &mut self.stack.entries {
        entry.trace(&mut marker);
    }
    for value in &mut self.stack.values {
        marker.value(value);
    }
    for object in self.canonical_symbols.iter_mut().flatten() {
        marker.object(object);
    }
    let roots = self.host_roots.get_mut();
    roots.sweep();
    for slot in &roots.slots {
        let mut value = slot.get();
        marker.value(&mut value);
        slot.set(value);
    }
    marker
}

/// Runs the marker's work lists empty. Each record is traced with the
/// arena borrowed field by field, which is what lets the marker be
/// borrowed at the same time.
pub(crate) fn drain_marking(&mut self, marker: &mut mark::Marker) {
    use crate::trace::Trace;
    let Heap {
        objects, frames, ..
    } = self;
    while let Some(work) = marker.next() {
        match work {
            mark::Work::Object(index) => objects[index as usize].trace(marker),
            // A married frame's registers are its entry's, already traced
            // with the stack; `live` is a stack index and no entry moves.
            mark::Work::Frame(index) => frames[index as usize].frame.trace(marker),
        }
    }
}
```

In `crates/ns-runtime/src/heap/world.rs`, beside `move_everything`:

```rust
/// Marks everything reachable: the heap's own roots, then the step state's
/// -- the bootstrap's builtin classes and the `platform` singleton -- then
/// the traversal.
pub(crate) fn mark_everything_reachable(&mut self) -> crate::heap::mark::Marker {
    use crate::trace::Tracer;
    let mutator = &mut self.mutator;
    let mut marker = mutator.heap.begin_marking();
    for class in &mut mutator.builtin_classes {
        marker.object(class);
    }
    if let Some(platform) = &mut mutator.platform_singleton {
        marker.value(platform);
    }
    mutator.heap.drain_marking(&mut marker);
    marker
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --lib heap::tests::marking`
Expected: PASS, all three.

- [ ] **Step 6: Run every gate**

The Global Constraints list, all of it. Expected: unchanged results, no golden moved.

- [ ] **Step 7: Commit**

```bash
git add crates/ns-runtime/src/heap/mark.rs crates/ns-runtime/src/heap.rs \
        crates/ns-runtime/src/heap/world.rs
git commit -F <message file>
```

Subject: `feat: mark every object and frame reachable from the roots`

---

### Task 3: Compaction, forwarding and a collection a test can ask for

Marking answers what is live; this task slides it to the front, truncates, rewrites every handle, and gives a test a way to ask for a collection between runs — the analogue of `move_now`.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`ForwardingTable`, `compact_arena`, `Heap::compact`, `heap_statistics`, tests)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`World::collect`, `World::heap_statistics`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Interpreter::heap_statistics`)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (`Interpreter::collect_now`, debug builds only)
- Create: `crates/ns-runtime/tests/collector.rs`

**Interfaces:**
- Consumes: `Marker`, `MarkBits`, `World::mark_everything_reachable` (Task 2); `Relocation`, `Forwarder<M>` (Task 1).
- Produces:
  - `pub(crate) struct ForwardingTable` implementing `Relocation`
  - `Heap::compact(&mut self, marker: mark::Marker) -> Forwarder<ForwardingTable>`
  - `World::collect(&mut self)` — mark, compact, forward, including the step state's own roots
  - `pub struct HeapStatistics { pub live_objects: usize, pub object_slots: usize, pub live_frames: usize, pub frame_slots: usize }` — defined in `heap.rs` and re-exported from `lib.rs`, the way `CacheStatistics` is defined in `inline_cache.rs` and re-exported. `live_*` is the arena's length, `*_slots` its capacity.
  - `Interpreter::heap_statistics(&self) -> HeapStatistics` (every build)
  - `Interpreter::collect_now(&mut self)` (debug builds only)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/ns-runtime/src/heap.rs`:

```rust
/// The point of the step: a slot nothing can reach is given back, and the
/// arena is shorter afterwards.
#[test]
fn collecting_reclaims_what_no_root_reaches() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
    let reachable = heap.allocate_object(class, ObjectData::String("live".to_string()));
    heap.allocate_object(class, ObjectData::String("dead".to_string()));
    heap.allocate_object(class, ObjectData::String("also dead".to_string()));
    let activation = heap.push_activation(header(), &[Value::Object(reachable)], 1);
    let before = heap.objects.len();

    heap.collect_for_test();

    assert_eq!(before, 4);
    // The class, the reachable string, and nothing else.
    assert_eq!(heap.objects.len(), 2);
    heap.pop_activation(activation);
}

/// A survivor is reached again through the root that kept it alive, which
/// is what pins that the forwarding rewrote the register rather than
/// leaving it pointing at whatever now occupies the old index.
#[test]
fn a_survivor_is_still_itself_after_compaction() {
    let mut heap = Heap::new();
    let class = heap.allocate_self_referential_class(ObjectData::Instance(vec![]));
    heap.allocate_object(class, ObjectData::String("dead".to_string()));
    let reachable = heap.allocate_object(class, ObjectData::String("live".to_string()));
    let activation = heap.push_activation(header(), &[Value::Object(reachable)], 1);

    heap.collect_for_test();

    let Value::Object(survivor) = heap.activation_local(activation, 0) else {
        panic!("the register no longer holds an object");
    };
    assert_eq!(heap.str_value(survivor), Some("live".to_string()));
    heap.pop_activation(activation);
}
```

Create `crates/ns-runtime/tests/collector.rs`:

```rust
//! The collector through a real interpreter: what survives a collection,
//! what does not, and what that does to the arenas
//! (`docs/design/memory.md`, "The gates the collector needs").

use indoc::indoc;

/// Every test here unwraps through this rather than `Result::unwrap`:
/// `HostSignal` has no `Debug`, on purpose, so `unwrap` does not compile.
fn expect_ok(result: ns_runtime::HostResult) -> ns_runtime::Root {
    match result {
        Ok(value) => value,
        Err(ns_runtime::HostSignal::Error(error)) => {
            panic!("unexpected Newspeak error: {}", error.message)
        }
        Err(ns_runtime::HostSignal::Return) => panic!("unexpected non-local return"),
    }
}

/// A module whose entry allocates a great many short-lived closures and
/// keeps none of them. Collecting between runs must give the arena back.
#[test]
#[cfg(debug_assertions)]
fn a_collection_gives_back_what_a_run_discarded() {
    let source = indoc! {"
        class Churn = ()(
          public run: count = (
            | total ::= 0. |
            count timesRepeat: [
              | block = [:value | value + 1]. |
              total:: block value: total.
            ].
            ^total
          )
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Churn");
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash_small_integer(2000);
    let selector = interpreter.intern("run:");

    interpreter.collect_now();
    let before = interpreter.heap_statistics();
    let _ = expect_ok(interpreter.send(&instance, selector, &[count]));
    let after_run = interpreter.heap_statistics();
    interpreter.collect_now();
    let after_collection = interpreter.heap_statistics();

    assert!(
        after_run.live_objects > before.live_objects,
        "the run should have allocated"
    );
    assert!(
        after_collection.live_objects < after_run.live_objects,
        "the collection should have given the discarded closures back"
    );
    assert!(
        after_collection.object_slots <= after_run.object_slots,
        "the buffer must not have grown across a collection"
    );
}
```

The test needs three small host conveniences. If `ns_runtime::test_support::bootstrap_module`, `Interpreter::stash_small_integer` or `Interpreter::intern` do not already exist under those names, use whatever the neighbouring integration tests in `crates/ns-runtime/tests/` use for the same three jobs — copy from the nearest one rather than inventing a second spelling.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --lib heap::tests::collecting heap::tests::a_survivor` then `cargo test -p ns-runtime --test collector`
Expected: FAIL — `collect_for_test`, `collect_now` and `heap_statistics` do not exist.

- [ ] **Step 3: Write the forwarding table and the compaction**

In `crates/ns-runtime/src/heap.rs`, beside `Rotation`:

```rust
/// What a collection maps each old slot index to: its new index, or `DEAD`
/// for a slot that is gone. A flat table rather than a run-length list of
/// shifts (Nova's `CompactionList`) because an index arena can afford 4
/// bytes per slot for the length of one collection, and O(1) beats a binary
/// search once per handle in the heap.
pub(crate) struct ForwardingTable {
    table: Vec<u32>,
}

/// No live slot can map here: an arena is addressed by 32-bit indices and
/// `next_object_index` refuses to mint `u32::MAX` of them.
const DEAD: u32 = u32::MAX;

impl Relocation for ForwardingTable {
    fn apply(&self, index: u32) -> u32 {
        let destination = self.table[index as usize];
        debug_assert_ne!(
            destination, DEAD,
            "a handle to a collected slot survived marking: a root the tracer does not know about"
        );
        destination
    }
}

/// Slides the marked slots of one arena to the front, truncates the rest
/// away -- which is where the dead records drop, and their payloads with
/// them -- and answers the table that maps old index to new.
fn compact_arena<T>(slots: &mut Vec<T>, bits: &mark::MarkBits) -> ForwardingTable {
    let mut table = vec![DEAD; slots.len()];
    let mut destination = 0usize;
    for source in 0..slots.len() {
        if bits.is_set(source as u32) {
            table[source] = u32::try_from(destination)
                .expect("internal: an arena is addressed by 32-bit indices");
            if destination != source {
                slots.swap(destination, source);
            }
            destination += 1;
        }
    }
    slots.truncate(destination);
    // `truncate` drops the dead records but never lowers the capacity, so
    // without this the pages a collection frees stay with the process and
    // the peak this step exists to lower does not move. With hysteresis, so
    // that a heap which oscillates does not reallocate and copy at every
    // collection: shrink only when three quarters of the buffer is idle,
    // and leave the live set room to double.
    if slots.capacity() > destination.saturating_mul(4) {
        slots.shrink_to(destination.saturating_mul(2));
    }
    ForwardingTable { table }
}
```

Then, beside `move_everything`:

```rust
/// Compacts both arenas against `marker`'s bits and rewrites every handle
/// the heap itself holds into the next epoch. What the heap does not hold,
/// the caller forwards with the answer, exactly as for a move
/// (`World::collect`).
///
/// Sound only where nothing outside the heap and the interpreter holds a
/// handle: at a safepoint of the run loop, or between runs.
#[must_use = "the interpreter's own roots are forwarded through the answer; dropping it leaves them stale"]
pub(crate) fn compact(&mut self, marker: mark::Marker) -> Forwarder<ForwardingTable> {
    use crate::trace::{Trace, Tracer};
    #[cfg(debug_assertions)]
    let from_epoch = self.epoch;
    #[cfg(debug_assertions)]
    let to_epoch = from_epoch.wrapping_add(1);
    #[cfg(debug_assertions)]
    {
        self.epoch = to_epoch;
    }
    let objects = compact_arena(&mut self.objects, &marker.objects);
    let frames = compact_arena(&mut self.frames, &marker.frames);
    let mut forwarder = Forwarder {
        #[cfg(debug_assertions)]
        heap_id: self.heap_id,
        #[cfg(debug_assertions)]
        from_epoch,
        #[cfg(debug_assertions)]
        to_epoch,
        objects,
        frames,
    };
    for object in &mut self.objects {
        object.trace(&mut forwarder);
    }
    for slot in &mut self.frames {
        slot.frame.trace(&mut forwarder);
    }
    for entry in &mut self.stack.entries {
        entry.trace(&mut forwarder);
    }
    for value in &mut self.stack.values {
        forwarder.value(value);
    }
    for object in self.canonical_symbols.iter_mut().flatten() {
        forwarder.object(object);
    }
    let roots = self.host_roots.get_mut();
    roots.sweep();
    for slot in &roots.slots {
        let mut value = slot.get();
        forwarder.value(&mut value);
        slot.set(value);
    }
    forwarder
}

/// The arenas' occupancy (`Interpreter::heap_statistics`).
pub(crate) fn heap_statistics(&self) -> crate::HeapStatistics {
    crate::HeapStatistics {
        live_objects: self.objects.len(),
        object_slots: self.objects.capacity(),
        live_frames: self.frames.len(),
        frame_slots: self.frames.capacity(),
    }
}
```

And the test-only driver, beside `mark_from_roots_for_test`:

```rust
/// Marks from the heap's own roots and compacts, for the heap's own
/// tests. A real collection goes through `World`.
#[cfg(test)]
fn collect_for_test(&mut self) {
    let marker = self.mark_from_roots_for_test();
    let _ = self.compact(marker);
}
```

- [ ] **Step 4: Write `World::collect` and the interpreter's entry points**

In `crates/ns-runtime/src/heap/world.rs`:

```rust
/// One collection: mark everything reachable, compact both arenas, and
/// forward what the step state holds beside the heap -- the bootstrap's
/// builtin classes and the `platform` singleton -- exactly as a move does.
pub(crate) fn collect(&mut self) {
    use crate::trace::Tracer;
    self.check_quiescent_state();
    let marker = self.mark_everything_reachable();
    let mutator = &mut self.mutator;
    let mut forwarder = mutator.heap.compact(marker);
    for class in &mut mutator.builtin_classes {
        forwarder.object(class);
    }
    if let Some(platform) = &mut mutator.platform_singleton {
        forwarder.value(platform);
    }
}

pub(crate) fn heap_statistics(&self) -> crate::HeapStatistics {
    self.mutator.heap.heap_statistics()
}
```

`check_quiescent_state` is `#[cfg(debug_assertions)]` today; give the call the same gate, or un-gate the check — un-gating is the better answer, since what it asserts (the argument buffer empty, the spare instantiation boxes holding no handle) is exactly as true a requirement for a release collection. Un-gate it and keep the `assert!`s.

In `crates/ns-runtime/src/interpreter/mod.rs`, beside `cache_statistics`:

```rust
/// The arenas' occupancy. Available in every build: it is what a
/// measurement and a retention test both read.
pub fn heap_statistics(&self) -> crate::HeapStatistics {
    self.world.heap_statistics()
}
```

and export `HeapStatistics` from `lib.rs` beside `CacheStatistics`.

In `crates/ns-runtime/src/interpreter/safepoint.rs`, beside `move_now`:

```rust
/// Collects now, between runs, where only roots hold values. For tests;
/// the run loop collects at a safepoint instead.
pub fn collect_now(&mut self) {
    assert!(!self.running, "collect_now is for between runs");
    self.world.collect();
}
```

`move_now` is `#[cfg(debug_assertions)]`; `collect_now` need not be, but keep it beside its sibling with the same gate so that nothing outside a test reaches for it, and mark the integration test `#[cfg(debug_assertions)]` accordingly.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --lib heap::tests` then `cargo test -p ns-runtime --test collector`
Expected: PASS.

- [ ] **Step 6: Run every gate**

The Global Constraints list. Expected: unchanged results. If `NS_NEXT_STRESS_MOVES=1` now fails, the fault is in Task 1's generic `Forwarder`, not here — the move path must be untouched by this task.

- [ ] **Step 7: Commit**

Subject: `feat: compact both arenas and forward every handle`

---

### Task 4: The canonical symbol table becomes weak

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`begin_marking`, a new `prune_canonical_symbols`, the field's comment)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`World::collect` prunes between marking and compacting)
- Modify: `crates/ns-runtime/tests/collector.rs`

**Interfaces:**
- Consumes: `Marker`, `Heap::compact` (Tasks 2 and 3).
- Produces: `Heap::prune_canonical_symbols(&mut self, marker: &mark::Marker)`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/tests/collector.rs`:

```rust
/// §5.1.6: equal symbols are one object. A symbol nothing holds may be
/// collected, because anything that held it would have been a root and
/// kept it alive -- so the next one minted is again the only one.
#[test]
#[cfg(debug_assertions)]
fn a_symbol_nothing_holds_is_collected_and_minted_again() {
    let source = indoc! {"
        class Symbols = ()(
          public make = (^#transient)
          public same = (^#kept == #kept)
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Symbols");
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let make = interpreter.intern("make");
    let same = interpreter.intern("same");

    let _discarded = expect_ok(interpreter.send(&instance, make, &[]));
    drop(_discarded);
    interpreter.collect_now();

    // Identity still holds for a symbol both sides of a comparison reach.
    let answer = expect_ok(interpreter.send(&instance, same, &[]));
    assert!(interpreter.is_true(&answer));
}

/// The table is not a root: a collection with no live symbol leaves fewer
/// objects than one that keeps the table strong would.
#[test]
#[cfg(debug_assertions)]
fn the_symbol_table_does_not_keep_its_objects_alive() {
    let source = indoc! {"
        class Transient = ()(
          public make: count = (
            count timesRepeat: [ #one. #two. #three. ].
            ^nil
          )
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Transient");
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash_small_integer(10);
    let selector = interpreter.intern("make:");

    let _ = expect_ok(interpreter.send(&instance, selector, &[count]));
    interpreter.collect_now();
    let after = interpreter.heap_statistics();
    interpreter.collect_now();
    let stable = interpreter.heap_statistics();

    assert_eq!(
        after.live_objects, stable.live_objects,
        "a second collection with nothing new must find nothing new to free"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test collector`
Expected: the identity test passes already (the table is still strong), the second is the one that pins the new behaviour. If both pass before the change, strengthen the second until it fails: assert on `live_objects` against a run that makes no symbols at all.

- [ ] **Step 3: Take the table out of the root set and prune it**

In `crates/ns-runtime/src/heap.rs`, remove this from `begin_marking`:

```rust
    for object in self.canonical_symbols.iter_mut().flatten() {
        marker.object(object);
    }
```

and add, beside it:

```rust
/// Clears every canonical symbol the marking did not reach. The table is
/// the system's only weak structure: §5.1.6 survives it because
/// `Mutator::new_symbol` is the only way to make a symbol object, so one
/// that is collected was held by nothing and the next mint is again the
/// only one (`docs/design/memory.md`, "Weak references").
///
/// Between marking and compaction: after, the bits no longer line up with
/// the arena; before, it could resurrect.
pub(crate) fn prune_canonical_symbols(&mut self, marker: &mark::Marker) {
    for slot in &mut self.canonical_symbols {
        if let Some(object) = slot {
            if !marker.objects.is_set(object.index) {
                *slot = None;
            }
        }
    }
}
```

Update the field's doc comment on `canonical_symbols` — it says "A root the tracer forwards; a strong one until the collector makes it weak" — to state that it is weak, pruned by `prune_canonical_symbols` and forwarded by `compact`.

In `World::collect`, between marking and compacting:

```rust
    let marker = self.mark_everything_reachable();
    self.mutator.heap.prune_canonical_symbols(&marker);
    let mutator = &mut self.mutator;
    let mut forwarder = mutator.heap.compact(marker);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test collector`
Expected: PASS.

- [ ] **Step 5: Run every gate**

The Global Constraints list. Watch `run-corpus` in particular: a symbol wrongly collected shows up as a selector that no longer matches, and the recorded outputs catch it.

- [ ] **Step 6: Commit**

Subject: `feat: make the canonical symbol table weak`

---

### Task 5: The retention gate

False retention — keeping what is dead — is invisible to every existing gate. This task adds the tests that count instead of observing.

**Files:**
- Modify: `crates/ns-runtime/tests/collector.rs`

**Interfaces:**
- Consumes: `Interpreter::collect_now`, `Interpreter::heap_statistics` (Task 3).
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Write the tests**

```rust
/// What a program keeps, the collector keeps. A closure held in a slot
/// retains its captured frame, so n kept closures leave n frames behind --
/// the retention shape closure conversion left in place, and the one
/// `bench_block.ns` measures.
#[test]
#[cfg(debug_assertions)]
fn kept_closures_keep_their_frames() {
    let source = indoc! {"
        class Keeper = (| public blocks ::= nil. |)(
          public fill: count = (
            | made ::= OrderedCollection new. |
            count timesRepeat: [
              | captured = count. |
              made add: [captured + 1].
            ].
            blocks:: made.
            ^nil
          )
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Keeper");
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash_small_integer(50);
    let selector = interpreter.intern("fill:");

    let _ = expect_ok(interpreter.send(&instance, selector, &[count]));
    interpreter.collect_now();
    let kept = interpreter.heap_statistics();
    interpreter.collect_now();
    let stable = interpreter.heap_statistics();

    assert_eq!(
        kept.live_objects, stable.live_objects,
        "a second collection must free nothing further"
    );
    assert!(
        kept.live_frames >= 50,
        "each kept closure retains the frame it captured, {} found",
        kept.live_frames
    );
}

/// And a run that keeps nothing returns to where it started, which is the
/// whole point of the step.
#[test]
#[cfg(debug_assertions)]
fn discarding_everything_returns_to_the_starting_size() {
    let source = indoc! {"
        class Discard = ()(
          public fill: count = (
            count timesRepeat: [
              | captured = count. |
              [captured + 1] value.
            ].
            ^nil
          )
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Discard");
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let selector = interpreter.intern("fill:");

    let small = interpreter.stash_small_integer(5);
    let _ = expect_ok(interpreter.send(&instance, selector, &[small]));
    interpreter.collect_now();
    let after_small = interpreter.heap_statistics();

    let large = interpreter.stash_small_integer(500);
    let _ = expect_ok(interpreter.send(&instance, selector, &[large]));
    interpreter.collect_now();
    let after_large = interpreter.heap_statistics();

    assert_eq!(
        after_small.live_objects, after_large.live_objects,
        "a hundred times the work, nothing kept, and the same live set"
    );
    assert_eq!(after_small.live_frames, after_large.live_frames);
}
```

- [ ] **Step 2: Run them**

Run: `cargo test -p ns-runtime --test collector`
Expected: PASS. If `discarding_everything_returns_to_the_starting_size` fails with a larger live set after the larger run, something retains per iteration: find what before changing the assertion. The assertion is the requirement, not the observation.

- [ ] **Step 3: Run every gate, then commit**

Subject: `test: pin what the collector keeps and what it gives back`

---

### Task 6: The trigger and the growth policy

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the limits, `push_object`, `reify`, `reset_collection_limits`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`collect` resets the limits; a collection-requested accessor)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (the safepoint serves the request)
- Modify: `crates/ns-runtime/tests/collector.rs`

**Interfaces:**
- Consumes: `World::collect` (Task 3).
- Produces:
  - `Heap::collection_requested(&self) -> bool`, `Heap::clear_collection_request(&mut self)`
  - `Heap::set_collection_floor(&mut self, floor: usize)` (debug builds only, for tests)
  - `Interpreter::set_collection_floor(&mut self, floor: usize)` (debug builds only)

- [ ] **Step 1: Write the failing test**

```rust
/// Nothing asks for a collection until an arena reaches its limit, and
/// then the run loop's next safepoint serves it -- no allocation ever
/// collects (`docs/design/memory.md`, "The trigger and the growth
/// policy").
#[test]
#[cfg(debug_assertions)]
fn allocating_past_the_limit_collects_during_the_run() {
    let source = indoc! {"
        class Churn = ()(
          public run: count = (
            | total ::= 0. |
            count timesRepeat: [
              | block = [:value | value + 1]. |
              total:: block value: total.
            ].
            ^total
          )
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Churn");
    interpreter.set_collection_floor(256);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash_small_integer(5000);
    let selector = interpreter.intern("run:");

    let _ = expect_ok(interpreter.send(&instance, selector, &[count]));
    let after = interpreter.heap_statistics();

    assert!(
        after.live_objects < 5000,
        "the run collected nothing: {} live objects after 5000 closures",
        after.live_objects
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test collector allocating_past_the_limit`
Expected: FAIL — `set_collection_floor` does not exist, and nothing requests a collection.

- [ ] **Step 3: Add the limits**

In `crates/ns-runtime/src/heap.rs`, beside `safepoint_requested`:

```rust
/// The arena length at which the next allocation asks for a collection.
/// Checked against the length the allocation already has in hand, so the
/// hot path pays one predictable branch and carries no counter.
next_object_collection: usize,
next_frame_collection: usize,
/// Whether a limit has been reached since the last collection. Separate
/// from `safepoint_requested`, which only means "take the slow path" and
/// which a stress mode holds set for its own reasons.
collection_requested: bool,
/// The smallest limit a collection will set. A program that stays under
/// it never collects: `bench_loop.ns` and `bench_send.ns` run in 4.6 MB
/// and should not pay for a collector they do not need.
collection_floor: usize,
```

with `COLLECTION_FLOOR: usize = 4096` as the initial value of `collection_floor`, `next_object_collection` and `next_frame_collection` in `Heap::new`.

In `push_object`, after the push:

```rust
    fn push_object(&mut self, object: Object<'gc>) -> ObjectReference<'gc> {
        let index = self.next_object_index();
        self.objects.push(object);
        if self.objects.len() >= self.next_object_collection {
            self.request_collection();
        }
        self.object_handle(index)
    }
```

and the same shape in `reify`, against `next_frame_collection`, after the frame is pushed. Beside them:

```rust
    /// An allocation never collects; it asks, and the run loop's next
    /// safepoint serves the request (decided 2026-09-13). Setting
    /// `safepoint_requested` is what makes the poll take its slow path.
    fn request_collection(&mut self) {
        self.collection_requested = true;
        self.safepoint_requested = true;
    }

    pub(crate) fn collection_requested(&self) -> bool {
        self.collection_requested
    }

    /// After a collection: no more is allowed to be allocated before the
    /// next one than is live, so the marking work per allocated slot is
    /// amortized constant.
    pub(crate) fn reset_collection_limits(&mut self) {
        self.collection_requested = false;
        self.next_object_collection = self.collection_floor.max(self.objects.len() * 2);
        self.next_frame_collection = self.collection_floor.max(self.frames.len() * 2);
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_collection_floor(&mut self, floor: usize) {
        self.collection_floor = floor;
        self.next_object_collection = floor;
        self.next_frame_collection = floor;
    }
```

- [ ] **Step 4: Serve the request at the safepoint**

`World::collect` ends with `self.mutator.heap.reset_collection_limits();`, and `World` grows the two pass-throughs `collection_requested` and `set_collection_floor`.

In `crates/ns-runtime/src/interpreter/safepoint.rs`, rewrite `Interpreter::safepoint`'s tail so that a pending collection is served after the stress modes have had their turn:

```rust
    #[cold]
    #[inline(never)]
    pub(crate) fn safepoint(&mut self) {
        #[cfg(debug_assertions)]
        let mut stressed = false;
        #[cfg(debug_assertions)]
        {
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
        }
        if self.world.collection_requested() {
            self.world.collect();
            return;
        }
        #[cfg(debug_assertions)]
        if stressed {
            return;
        }
        self.world.clear_safepoint_request();
    }
```

Keep the comment that explains why the slow path is never empty, and extend it: it now has real work in a release build, so the argument is about what remains when no request is pending.

Add `Interpreter::set_collection_floor` beside `set_stress_moves`, `#[cfg(debug_assertions)]`, delegating to `World`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p ns-runtime --test collector allocating_past_the_limit`
Expected: PASS.

- [ ] **Step 6: Run every gate**

The Global Constraints list, and pay attention to `run-corpus`: it runs each module from source *and* as a built binary, so this is where a decoded graph that the collector does not consider reachable would show up. Any difference there is a root the marking does not know about — find it, do not raise the floor.

- [ ] **Step 7: Commit**

Subject: `feat: request a collection when an arena reaches its limit`

---

### Task 7: The collect stress mode and the third gate

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (`StressCollections`, `stress_collect_from_env`, `set_stress_collect`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (the field, the bootstrap reading the environment)
- Modify: `CLAUDE.md` (the gates)

**Interfaces:**
- Consumes: `World::collect` (Task 3).
- Produces: `Interpreter::set_stress_collect(&mut self, enabled: bool)` (debug builds only).

- [ ] **Step 1: Write the failing test**

Add to `crates/ns-runtime/tests/collector.rs`:

```rust
/// The mode that catches premature reclamation: collecting at every
/// safepoint, so a missed root kills its object at the earliest possible
/// moment. The program's answer must not change.
#[test]
#[cfg(debug_assertions)]
fn the_collect_stress_mode_does_not_change_an_answer() {
    let source = indoc! {"
        class Sum = ()(
          public upTo: count = (
            | total ::= 0. |
            1 to: count do: [:each | total:: total + each].
            ^total
          )
        )
    "};
    let (mut interpreter, class) = ns_runtime::test_support::bootstrap_module(source, "Sum");
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash_small_integer(100);
    let selector = interpreter.intern("upTo:");

    let quiet = expect_ok(interpreter.send(&instance, selector, &[count]));
    let quiet_answer = interpreter.small_integer_value(&quiet);
    interpreter.set_stress_collect(true);
    let stressed = expect_ok(interpreter.send(&instance, selector, &[count]));

    assert_eq!(quiet_answer, interpreter.small_integer_value(&stressed));
    assert_eq!(quiet_answer, Some(5050));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test collector the_collect_stress_mode`
Expected: FAIL — `set_stress_collect` does not exist.

- [ ] **Step 3: Write the stress mode**

In `crates/ns-runtime/src/interpreter/safepoint.rs`, beside `StressMoves`, a `StressCollections` with the same shape and the same pacing argument — a collection rewrites work proportional to the live set, so collecting at literally every safepoint is quadratic in a deep recursion:

```rust
/// The collect stress mode's schedule: a collection at every safepoint
/// while the traced size is within `STRESS_MOVE_BUDGET`, and above it once
/// per size-over-budget safepoints -- the pacing `StressMoves` uses, for
/// the same reason.
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StressCollections {
    safepoints_since_collection: usize,
}

#[cfg(debug_assertions)]
impl StressCollections {
    fn due(&mut self, traced_size: usize) -> bool {
        self.safepoints_since_collection += 1;
        if self
            .safepoints_since_collection
            .saturating_mul(STRESS_MOVE_BUDGET)
            < traced_size
        {
            return false;
        }
        self.safepoints_since_collection = 0;
        true
    }

    /// Whether `NS_NEXT_STRESS_COLLECT` asks for the mode: set, and
    /// neither empty nor `0`.
    pub(crate) fn from_env() -> Option<StressCollections> {
        let requested = std::env::var_os("NS_NEXT_STRESS_COLLECT")
            .is_some_and(|value| !value.is_empty() && value != "0");
        requested.then(StressCollections::default)
    }
}
```

Add `stress_collect: Option<StressCollections>` to `Interpreter` (`#[cfg(debug_assertions)]`), read it in the bootstrap where `StressMoves::from_env` is read, and serve it in `safepoint` before the ordinary request:

```rust
        #[cfg(debug_assertions)]
        if let Some(stress) = &mut self.stress_collect {
            if stress.due(self.world.traced_size()) {
                self.world.collect();
            }
            return;
        }
```

`set_stress_collect` mirrors `set_stress_moves`, including keeping `safepoint_requested` set while any mode is on.

- [ ] **Step 4: Run the test, then the whole suite under the new mode**

```bash
cargo test -p ns-runtime --test collector the_collect_stress_mode
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: PASS; `ran 6, failed 0, skipped 205`. A failure here is the mode doing its job — a root the marking does not know about. Fix the root, never the test.

- [ ] **Step 5: Make it a gate in `CLAUDE.md`**

In "Working in this repository", after the cache stress block:

```bash
# the same suite with a collection at every safepoint
# (debug builds only; Git Bash syntax)
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
```

and, after the cache-stress corpus line:

```bash
# non-regression gate under the collect stress mode, same report
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Extend the "An image is untrusted input" and handle invariants only if this task changed what they say — it should not have.

- [ ] **Step 6: Run every gate, now including the new one, then commit**

Subject: `test: collect at every safepoint as a third stress mode`

---

### Task 8: `bench --collector-statistics`

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the counters)
- Modify: `crates/ns-runtime/src/heap/world.rs`, `crates/ns-runtime/src/interpreter/mod.rs` (`collector_statistics`)
- Modify: `crates/ns-cli/src/main.rs:118-131`, `crates/ns-cli/src/entry.rs`, `crates/ns-cli/src/commands/bench.rs`

**Interfaces:**
- Consumes: `World::collect` (Task 3).
- Produces:
  - `pub struct CollectorStatistics { pub collections: usize, pub total_pause: std::time::Duration, pub longest_pause: std::time::Duration, pub live_objects: usize, pub live_frames: usize }`
  - `Interpreter::collector_statistics(&self) -> CollectorStatistics`
  - `entry::print_collector_statistics(&CollectorStatistics)`

- [ ] **Step 1: Count in `World::collect`**

Wrap the body in `std::time::Instant::now()` / `elapsed()`, and keep `collections`, `total_pause` and `longest_pause` on `Heap` beside the limits. The measurement needs them; nothing else reads them, so they cost one `Instant` per collection and nothing per allocation.

- [ ] **Step 2: Write the failing CLI test**

Add to the CLI's test suite, beside the existing `--cache-statistics` coverage (`crates/ns-cli/tests/`), a test that runs `bench --iterations 1 --collector-statistics` on `goldens/bench/bench_block.ns` and asserts stderr contains `collector:`. Copy the scratch-directory and invocation helpers from the neighbouring test file rather than writing new ones.

- [ ] **Step 3: Run it to verify it fails**

Expected: FAIL — unknown argument `--collector-statistics`.

- [ ] **Step 4: Add the flag and the printer**

In `crates/ns-cli/src/main.rs`, in `Bench`, beside `cache_statistics`:

```rust
        /// Print the collector's counts and pauses to stderr after the
        /// timed runs.
        #[arg(long)]
        collector_statistics: bool,
```

Thread it through `commands::bench::execute` exactly as `cache_statistics` is threaded, and add to `crates/ns-cli/src/entry.rs`:

```rust
/// `--collector-statistics`'s one line, always to stderr so that a
/// golden's recorded stdout is untouched.
pub(crate) fn print_collector_statistics(statistics: &ns_runtime::CollectorStatistics) {
    eprintln!(
        "collector: {} collections, {:.3} ms total pause, {:.3} ms longest; {} live objects, \
         {} live frames",
        statistics.collections,
        statistics.total_pause.as_secs_f64() * 1000.0,
        statistics.longest_pause.as_secs_f64() * 1000.0,
        statistics.live_objects,
        statistics.live_frames,
    );
}
```

- [ ] **Step 5: Run the test to verify it passes, run every gate, commit**

Subject: `feat: report the collector's counts and pauses from bench`

---

### Task 9: The measurement and the documents

**Files:**
- Modify: `docs/design/execution.md` (a new section, "What the collector moved")
- Modify: `docs/design/memory.md` (the peak table's fourth column; the status lines)
- Modify: `docs/design/README.md` (the memory row's status)
- Modify: `docs/PROGRESS.md` (a new "M4 — The moving collector" section and the next-session paragraph)

- [ ] **Step 1: Build both binaries with the protocol's flags**

In PowerShell, for the baseline `baf524d` and for the head, each in its own checkout or in turn:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

- [ ] **Step 2: Check the inlining before timing anything**

`dumpbin /disasm` (Visual Studio Build Tools; not on `PATH`) on a release build with debug information: no `vm::run::closure` may be a function of its own. Record the instruction count of `vm::run` the way the previous sections do.

- [ ] **Step 3: Take the throughput measurement**

All eight sources, two sittings, head against `baf524d` and against `d804eaf`. Four of the eight are under `goldens/bench/`; the other four — `bench_new.ns`, `bench_new2.ns`, `bench_newarg.ns` and `bench_do.ns` — are **written out** in `docs/design/execution.md`, "What spec-conformant lookup moved", and must be copied from there, never reconstructed from prose.

- [ ] **Step 4: Take the peak working set**

Release build, `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize` via `psapi!GetProcessMemoryInfo` on the still-open handle — PowerShell's `Process.PeakWorkingSet64` answers 0 after exit on this platform. The rows that matter are the ones already in `memory.md`'s table: `run` and `bench --iterations 10` on `bench_loop.ns`, `bench_send.ns` and `bench_block.ns`.

- [ ] **Step 5: Take the collector's own numbers**

`bench --collector-statistics` on each of the eight, recording collections, total pause and longest pause.

- [ ] **Step 6: Write it up against the recorded expectation**

The expectation is in `memory.md`, "How the collector is measured", and it was written before the numbers: `bench_block.ns`'s peak falls substantially; `bench_loop.ns` and `bench_send.ns` stay under the floor, never collect and must not move measurably; the send benchmarks must not feel the comparison in `push_object`. Where a number contradicts the expectation, write down which, and why, rather than adjusting the expectation.

- [ ] **Step 7: Update the documents and commit**

`memory.md` gains the fourth column and loses the "not yet built" qualifier in "The shape: mark, compact, forward"; `README.md`'s memory row stops saying nothing reclaims a slot; `PROGRESS.md` gains its section and its next-session paragraph moves on to what follows step 5.

Subject: `docs: record what the moving collector moved`

---

## Self-Review

**Spec coverage.** Every section of `docs/design/memory.md`'s collector design maps to a task: "The shape" → Tasks 1–3; "Marking" → Task 2; "Compaction and forwarding" → Tasks 1 and 3; "The trigger and the growth policy" → Task 6; "Weak references" → Task 4 (the application cache needs no work — it stays as it is, which the spec explains); "Frames, the activation stack and the marriage" → Tasks 2 and 3, pinned by `marking_reaches_a_married_frame_through_its_entry`; "The gates the collector needs" → Tasks 5 and 7; "How the collector is measured" → Tasks 8 and 9.

**Placeholders.** One deliberate soft spot: Task 3's integration test names three host conveniences (`test_support::bootstrap_module`, `stash_small_integer`, `intern`, and later `is_true` and `small_integer_value`) whose exact spellings are not verified in this plan. The instruction there is to copy the spelling from the nearest existing integration test rather than invent a second one — which is the repo's own rule about duplicated helpers. Check this first when starting Task 3.

**Type consistency.** `Relocation::apply(&self, index: u32) -> u32` is implemented by `Rotation` (Task 1) and `ForwardingTable` (Task 3), and `Forwarder<M: Relocation>` is the one tracer both go through. `MarkBits::is_set` is used by `compact_arena` and `prune_canonical_symbols`; `Marker::objects`/`frames` are the fields both reach. `World::collect` is called by `Interpreter::collect_now` (Task 3), by the safepoint's request path (Task 6) and by the collect stress mode (Task 7) — one entry point, three callers.
