# Object and Frame Arena Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `Rc<RefCell<_>>` with an arena of typed 32-bit handles behind
`ObjRef` and `FrameRef`, so that objects and activations live in `Heap`-owned
vectors instead of individually reference-counted, interior-mutable cells.

**Architecture:** `Heap` gains `objects: Vec<Object>` and `frames: Vec<Frame>`;
a handle becomes an index into one of them. Every read and write already goes
through `Heap`'s accessors (the seam, merged 2026-09-09), and every accessor
already takes `&self`/`&mut self`, so the store changes inside `heap.rs` and
the ~193 call sites do not move. The change deletes three things outright:
`types.rs`'s 176-line iterative-teardown machinery (an arena drops as one
`Vec`, with no reference chain to unwind), `heap::bootstrap_self_referential_object`'s
unsafe `Rc::from_raw` construction (an arena can name a slot before filling it),
and the `.borrow()` grep gate (the handle's inner field becomes private to
`heap.rs`, which the compiler enforces everywhere).

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Three
crates: `ns-syntax`, `ns-runtime`, `ns-cli`.

**Spec:** [docs/design/memory.md](../../design/memory.md), "Planned: an arena,
handles, and a real collector", and [docs/design/object-model.md](../../design/object-model.md),
"Planned: arena allocation and handles". Read both before Task 1.

## Scope

This is Milestone 4's step 1 as [docs/PROGRESS.md](../../PROGRESS.md) numbers
it, and **only** step 1: the representation. Three things the design documents
discuss alongside the arena are deliberately **not** in this plan.

- **The `GcScope`/`NoGcScope` root discipline** is step 2. Nothing here needs
  it: without a collector, no handle can ever be invalidated by a safepoint.
- **Activations on a stack, reified on demand** is step 3. A frame stays one
  arena entry per activation here. In particular, **do not** merge `Frame.args`
  and `Frame.temps` into one register file as part of this plan: step 3
  replaces both with a contiguous register stack, and an intermediate merge is
  work thrown away, plus an image-format change bought twice.
- **A collector** is step 5.

**The consequence to accept and measure, not to design around:** without a
collector, neither arena ever reclaims a slot. Every object and every
activation a program creates is retained for the process's lifetime, where
`Rc` frees them today. Task 7 measures what that costs on the benchmark and
the corpus and records the number; it is an interim regression the milestone
ordering accepts, and step 3 removes the frame half of it by construction
while step 5 removes the object half.

## Global Constraints

- Rust 1.98.0, edition 2021. **No new external dependency** — the workspace is
  hand-rolled on purpose.
- All code, identifiers, comments, doc comments and commit messages in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`).
  Every commit ends with the trailer
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
  Never add a `Claude-Session` URL line.
- `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets`
  must be warning-free.
- Both non-regression gates must hold at every commit:
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`, and
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- The two engines must agree byte for byte on exit status, stdout **and**
  stderr; `crates/ns-cli/tests/differential.rs` enforces it with no exemptions.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvalResult`.
  Tests unwrap through a local `expect_ok` helper — copy the one in a
  neighbouring test file.
- **Object and frame access goes through `crates/ns-runtime/src/heap.rs`.** If
  an accessor you need is missing, add it there; never widen a field's
  visibility to reach past it.
- Never judge performance from a debug build. `cargo build --workspace --release`
  is the one to measure.
- Design documents in `docs/design/` are living: update the document for the
  aspect that changed. This plan file is an execution record — leave it as
  written once complete.

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/ns-runtime/src/heap.rs` | Owns `objects`/`frames`, defines `ObjRef`/`ClassRef`/`FrameRef` with a **private** inner index, and holds every accessor. The only file that can name a handle's representation. |
| `crates/ns-runtime/src/types.rs` | Pure data only. Loses the whole `// ---------- Iterative teardown ----------` section and the handle definitions; re-exports the handles from `heap.rs` so `ns_runtime::types::ObjRef` keeps resolving. |
| `crates/ns-runtime/src/lib.rs` | Re-exports the handles from `heap` alongside `Heap`. |
| `crates/ns-runtime/src/interpreter/mod.rs` | `build_self_referential_object_class` takes a `&mut Heap`. |
| `crates/ns-runtime/src/dispatch.rs` | Its `dummy_class` test helper takes a `&mut Heap`. |
| `crates/ns-runtime/tests/interpreter.rs`, `tests/image.rs` | Fixtures allocate through one live `Heap` instead of a throwaway one. |
| `docs/design/memory.md`, `object-model.md`, `execution.md`, `README.md`, `docs/PROGRESS.md`, `CLAUDE.md` | Updated in Task 7 to describe the built state. |

---

### Task 1: One live heap for every fixture

Four places build an object or a frame through a `Heap` that is not the one
the object will later be read through. Every one of them works today only
because `Heap` is a bare counter and all instances are interchangeable. Under
an arena each is a silent wrong answer — a fresh `Heap` is an *empty* arena,
and an index minted against one heap either misses or, worse, names a
different, perfectly valid object in another.

Fix them **first**, while `Rc` still works, so that Task 2's and Task 3's
failures are real failures and not fixture noise.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (add `alloc_self_referential_class`, make the free function private)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:1003`, `:1046-1057`
- Modify: `crates/ns-runtime/src/dispatch.rs:251-253`
- Modify: `crates/ns-runtime/src/types.rs:585-615`
- Test: `crates/ns-runtime/tests/interpreter.rs:24-27`, `:183-208`
- Test: `crates/ns-runtime/tests/image.rs:450-463`

**Interfaces:**
- Produces: `Heap::alloc_self_referential_class(&mut self, data: ObjectData) -> ObjRef`
  — the single entry point for the four bootstraps that need an object that is
  its own `.class`. Task 3 changes only its body.
- Produces: `dispatch::tests::dummy_class(heap: &mut Heap) -> ClassRef`,
  `types::tests::dummy_class(heap: &mut Heap) -> ClassRef`,
  `interpreter::build_self_referential_object_class(heap: &mut Heap, application: Rc<MixinApplication>) -> ClassRef`.
- Produces: in `crates/ns-runtime/tests/interpreter.rs`,
  `dummy_class(heap: &mut ns_runtime::Heap) -> ns_runtime::ClassRef` and
  `method_frame(heap: &mut ns_runtime::Heap) -> ns_runtime::FrameRef`.

- [ ] **Step 1: Write the failing test**

Add to `crates/ns-runtime/src/heap.rs`'s `mod tests`:

```rust
    #[test]
    fn a_self_referential_class_is_its_own_class() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        assert!(heap.class_of_obj(&class).same(&class));
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p ns-runtime --lib heap::tests::a_self_referential_class_is_its_own_class`
Expected: FAIL — `no method named 'alloc_self_referential_class' found for struct 'Heap'`.

- [ ] **Step 3: Add the method and hide the free function**

In `crates/ns-runtime/src/heap.rs`, add this method to `impl Heap`, directly
below `alloc_placeholder_object`:

```rust
    /// Allocates a class object that is genuinely its own `.class` — the
    /// shape every one of the crate's four bootstraps needs
    /// (`Interpreter::bootstrap`'s `build_self_referential_object_class` in
    /// production; the `dummy_class` helpers in `dispatch.rs`, `types.rs`
    /// and `tests/interpreter.rs`, each satisfying `Object.class`'s
    /// non-optional type without wiring up a whole hierarchy).
    ///
    /// Takes `&mut self` although today's body ignores it, for the same
    /// reason every accessor here already does: under the arena the object
    /// has to be allocated *into* a heap, and a caller that reached this
    /// through a throwaway `Heap` would silently read a different heap's
    /// slot afterwards.
    ///
    /// Identity hash fixed at 0, like `alloc_placeholder_object`: none of
    /// the four callers links this object into a real hierarchy a hash
    /// could distinguish it within.
    pub fn alloc_self_referential_class(&mut self, data: ObjectData) -> ObjRef {
        bootstrap_self_referential_object(data)
    }
```

Change the free function's signature line from

```rust
pub fn bootstrap_self_referential_object(data: ObjectData) -> ObjRef {
```

to

```rust
fn bootstrap_self_referential_object(data: ObjectData) -> ObjRef {
```

and, in its doc comment, replace the paragraph beginning "The four call sites
above" with:

```rust
/// Private to this module: `Heap::alloc_self_referential_class` is the only
/// caller, so a fifth call site cannot mint a second live self-referential
/// object outside a heap. That mattered even before the arena — such an
/// object shares identity hash 0 with every placeholder
/// `alloc_placeholder_object` has ever built, and code elsewhere reasons
/// about hash-0 objects being throwaway and never observed outside their
/// own construction.
```

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cargo test -p ns-runtime --lib heap::tests::a_self_referential_class_is_its_own_class`
Expected: PASS.

- [ ] **Step 5: Thread a heap through the production bootstrap**

In `crates/ns-runtime/src/interpreter/mod.rs`, change the function at line 1046 to:

```rust
fn build_self_referential_object_class(
    heap: &mut Heap,
    application: Rc<crate::mixin::MixinApplication>,
) -> ClassRef {
    heap.alloc_self_referential_class(ObjectData::Class(ClassData {
        application,
        enclosing: None,
        declaring_class: None,
        superclass: None,
        builtin: Some(BuiltinClass::Object),
        metaclass: None,
    }))
}
```

In its doc comment, replace "The unsafe construction itself lives in
`heap::bootstrap_self_referential_object`" with "The construction itself lives
in `Heap::alloc_self_referential_class`".

At line 1003, change the call to:

```rust
    let class_of_class =
        build_self_referential_object_class(&mut interp.heap, object_application.clone());
```

- [ ] **Step 6: Thread a heap through the three test fixtures**

In `crates/ns-runtime/src/dispatch.rs`, change `dummy_class` to:

```rust
    fn dummy_class(heap: &mut Heap) -> ClassRef {
        heap.alloc_self_referential_class(ObjectData::Instance(vec![]))
    }
```

and update its doc comment's reference from `heap::bootstrap_self_referential_object`
to `Heap::alloc_self_referential_class`. Every caller in that module's `mod tests`
already has a `let mut heap = Heap::new();` in scope — pass `&mut heap`.

In `crates/ns-runtime/src/types.rs`, change `dummy_class` to take
`heap: &mut crate::Heap` and call `heap.alloc_self_referential_class(...)`,
updating its doc comment the same way. Rewrite the two tests that use it:

```rust
    #[test]
    fn class_data_is_reachable_through_object_data() {
        let mut heap = crate::Heap::new();
        let class = dummy_class(&mut heap);
        assert!(heap.is_class(&class));
    }

    #[test]
    fn value_clone_shares_object_identity() {
        let mut heap = crate::Heap::new();
        let class = dummy_class(&mut heap);
        let instance = heap.alloc_object(class, ObjectData::Instance(vec![]));
        let a = Value::Obj(instance.clone());
        let b = a.clone();
        match (a, b) {
            (Value::Obj(x), Value::Obj(y)) => assert!(x.same(&y)),
            _ => panic!("expected Value::Obj"),
        }
    }
```

Delete the now-stale "`heap` is inert here" comment from the first of the two.

In `crates/ns-runtime/tests/interpreter.rs`, change `dummy_class` to
`fn dummy_class(heap: &mut ns_runtime::Heap) -> ns_runtime::ClassRef` calling
`heap.alloc_self_referential_class(ObjectData::Instance(vec![]))`, and change
`method_frame` to take `heap: &mut ns_runtime::Heap` and call
`heap.alloc_frame(Frame { .. })` on it, deleting the "No live interpreter here"
comment. Every caller of the two is inside a test that already builds an
`Interpreter`; pass `interp.heap_mut()`. Where a caller does not yet have one,
add `let mut interp = ns_runtime::Interpreter::bootstrap();` above it and pass
`interp.heap_mut()`.

- [ ] **Step 7: Give the image fixture a real heap**

In `crates/ns-runtime/tests/image.rs` at line 450, replace the
`let heap = Heap::default();` line and its preceding "`heap` is inert here"
comment with a heap borrowed from the interpreter that owns `class`. The
fixture's callers each hold one; thread it in as a parameter:

```rust
fn encode_and_decode_tables(
    heap: &Heap,
    module: &CompiledModule,
    class: &ClassRef,
) -> (decode::DecodedCodeTable, mixins::Decoded, encode::CodeIds) {
```

and delete the local `heap` binding. Update every call site to pass the
interpreter's heap (`interp.heap_ref()`, or the `heap` already in scope).

- [ ] **Step 8: Confirm no throwaway heap is left**

Run: `grep -rn "Heap::new()\|Heap::default()" crates --include=*.rs`
Expected: matches only in `crates/ns-runtime/src/heap.rs` (its own `mod tests`),
`crates/ns-runtime/src/interpreter/mod.rs` (`Interpreter::new`/`bootstrap`),
`crates/ns-runtime/src/dispatch.rs` and `crates/ns-runtime/src/types.rs`
(unit tests that build one heap and use only it). No match may be in a
position where the handle outlives the heap that made it.

- [ ] **Step 9: Run the full suite and both gates**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: clippy warning-free, all tests pass,
`parsed 210, failed 0, expected-fail 1, stale 0`, `ran 6, failed 0, skipped 205`.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor: allocate every self-referential class through a live heap

Four bootstraps built an object through a Heap that is not the one it is
later read through, which works only while Heap is a bare counter. Route
them through Heap::alloc_self_referential_class and make the unsafe
construction private, so the arena can change one body instead of four
call sites.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Frames into an arena

`FrameRef` becomes a 32-bit index into `Heap.frames`. Frames go first because
their teardown is the half that unwinds *chains* — `sender`, `home`,
`captured_frame` — so removing it leaves `types.rs`'s remaining teardown
self-contained on objects, which Task 3 then deletes whole.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the `Heap` struct, `alloc_frame`, all 18 frame accessors)
- Modify: `crates/ns-runtime/src/types.rs` (`FrameRef`'s inner type, `same`, `identity_key`, `Debug`; the teardown section narrows)
- Test: `crates/ns-runtime/src/heap.rs`'s `mod tests`

**Interfaces:**
- Consumes: `Heap::alloc_self_referential_class` from Task 1.
- Produces: `FrameRef { pub(crate) index: u32 }` — a named field rather than a
  tuple field, because Task 4 adds a second one — and `Heap.frames: Vec<Frame>`.
  Every public `Heap` frame-accessor signature is **unchanged**.

- [ ] **Step 1: Write the characterization tests**

Add to `crates/ns-runtime/src/heap.rs`'s `mod tests`:

```rust
    fn top_level_frame() -> Frame {
        Frame {
            receiver: Value::Nil,
            args: vec![],
            temps: vec![],
            pc: 0,
            pc_kind: PcKind::TreeWalk,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
            slot_base: 0,
        }
    }

    /// A frame is addressed by its position in the arena, so a handle must
    /// stay valid across the reallocation that growing the arena performs.
    /// Under `Rc` this was free; under an arena it is the property the
    /// representation rests on.
    #[test]
    fn a_frame_handle_survives_the_arena_growing_under_it() {
        let mut heap = Heap::new();
        let first = heap.alloc_frame(top_level_frame());
        for _ in 0..1000 {
            heap.alloc_frame(top_level_frame());
        }
        assert!(matches!(heap.receiver(&first), Value::Nil));
        assert_eq!(heap.frame_register_shape(&first), (0, 0, 0));
    }

    /// Activation identity is what `Signal::Return`'s target check and
    /// `invoke_method`'s catch both rest on, so two live activations may
    /// never compare equal however alike their contents are.
    #[test]
    fn two_frames_are_never_the_same_activation() {
        let mut heap = Heap::new();
        let a = heap.alloc_frame(top_level_frame());
        let b = heap.alloc_frame(top_level_frame());
        assert!(!a.same(&b));
        assert!(a.same(&a.clone()));
    }
```

- [ ] **Step 2: Run them to confirm they pass on `Rc` first**

Run: `cargo test -p ns-runtime --lib heap::tests::`
Expected: PASS. These are characterization tests — they must hold both before
and after the representation changes, and their value is that they fail loudly
if the arena breaks handle stability or identity. Confirming they pass now is
what proves they test the property rather than the implementation.

- [ ] **Step 3: Commit the characterization tests**

```bash
git add crates/ns-runtime/src/heap.rs
git commit -m "test: pin frame handle stability and activation identity

Both properties are free under Rc and are exactly what an index-based
handle could silently break, so they are pinned before the store changes
under them.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Change the frame handle's representation**

In `crates/ns-runtime/src/types.rs`, replace `FrameRef`'s definition and its
three impls with:

```rust
/// An opaque handle to an activation: an index into `Heap.frames`. The
/// inner index is private to the crate, and only `types.rs` and `heap.rs`
/// reach it — that privacy is what made replacing `Rc<RefCell<Frame>>`
/// with this a change to those two files rather than to every caller.
/// See `docs/design/memory.md`, "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot — there is no collector yet — so two live
/// activations never share an index, which is what `Signal::Return`'s
/// target check depends on.
#[derive(Clone)]
pub struct FrameRef {
    pub(crate) index: u32,
}

impl FrameRef {
    /// Activation identity. Replaces `Rc::ptr_eq` at every call site, so
    /// that identity has one spelling that survives the store changing
    /// under it -- `invoke_method`'s `Signal::Return` catch and
    /// `raise_return`'s target check both depend on this rather than on
    /// the representation directly.
    pub fn same(&self, other: &FrameRef) -> bool {
        self.index == other.index
    }

    /// An opaque, hashable key unique to this handle, for code that needs
    /// `FrameRef` as a `HashMap` key rather than a comparison --
    /// `image::encode::GraphTable`'s frame-id map. Crate-internal: its
    /// only current caller lives inside `ns-runtime`.
    pub(crate) fn identity_key(&self) -> usize {
        self.index as usize
    }
}

/// Prints the arena index and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` walking the sender chain (a closure's
/// `home` can itself be captured, so that walk had no acyclic guarantee
/// to lean on); an index has nothing to descend into at all, so the
/// hazard is gone by construction rather than by discipline. It also can
/// no longer panic, which is why the `try_borrow` dance is gone.
impl std::fmt::Debug for FrameRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameRef(#{})", self.index)
    }
}
```

- [ ] **Step 5: Give `Heap` the frame arena**

In `crates/ns-runtime/src/heap.rs`, change the struct to:

```rust
#[derive(Debug)]
pub struct Heap {
    /// Monotonic, assigned at allocation, never derived from an address, so
    /// identity survives both a moving collector and a snapshot
    /// (`docs/design/memory.md`).
    next_identity_hash: u32,
    /// Every activation, addressed by a `FrameRef`'s index. Nothing is ever
    /// removed: reclaiming a slot needs a collector, which is Milestone 4's
    /// step 5. Until then an activation is retained for the process's
    /// lifetime — the interim cost this milestone's ordering accepts, and
    /// the one step 3 removes by construction when activations move onto a
    /// stack.
    frames: Vec<Frame>,
}
```

and `Heap::new` to `Heap { next_identity_hash: 1, frames: Vec::new() }`.

Rewrite `alloc_frame`'s body, keeping its whole existing doc comment and the
`debug_assert_eq!` untouched:

```rust
    pub fn alloc_frame(&mut self, frame: Frame) -> FrameRef {
        debug_assert_eq!(
            frame.home.is_some(),
            matches!(frame.kind, FrameKind::Closure(_)),
            "Frame.home must be populated exactly for a closure activation"
        );
        let index = u32::try_from(self.frames.len())
            .expect("more than u32::MAX activations allocated in one heap");
        self.frames.push(frame);
        FrameRef { index }
    }
```

- [ ] **Step 6: Convert the 18 frame accessors**

The rewrite is mechanical: `frame.0.borrow()` becomes `self.frames[frame.index as usize]`,
and `frame.0.borrow_mut()` becomes `self.frames[frame.index as usize]` behind a
`&mut self`. Indexing rather than `.get()` on purpose: a handle is in range by
construction, and a cross-heap handle — the only way to violate that — should
fail loudly here rather than read a different heap's activation. Convert every
one of: `receiver`, `receiver_and_current_class`, `local`, `set_local`,
`frame_current_class`, `frame_slot_base`, `grow_temps`, `set_frame_pc`,
`frame_home`, `frame_sender`, `frame_captured`, `frame_is_uncontinuable`,
`mark_uncontinuable`, `frame_is_method`, `frame_kind`, `frame_register_shape`,
`frame_snapshot`, `replace_frame`. Keep every doc comment.

The four whose shape changes by more than the substitution:

```rust
    pub fn receiver_and_current_class(&self, frame: &FrameRef) -> (Value, Option<ClassRef>) {
        let f = &self.frames[frame.index as usize];
        (f.receiver.clone(), f.current_class.clone())
    }

    pub fn local(&self, frame: &FrameRef, index: u16) -> Value {
        let f = &self.frames[frame.index as usize];
        let i = index as usize;
        if i < f.args.len() {
            f.args[i].clone()
        } else {
            f.temps[i - f.args.len()].clone()
        }
    }

    pub fn set_local(&mut self, frame: &FrameRef, index: u16, value: Value) {
        let f = &mut self.frames[frame.index as usize];
        let i = index as usize;
        if i < f.args.len() {
            f.args[i] = value;
        } else {
            let temp_index = i - f.args.len();
            f.temps[temp_index] = value;
        }
    }

    pub fn replace_frame(&mut self, frame: &FrameRef, filled: Frame) {
        self.frames[frame.index as usize] = filled;
    }
```

`receiver_and_current_class`'s doc comment says it exists to take one borrow
where two were measured as a 13% regression. Append to it:

```rust
    /// Under the arena the two reads are two slice indexes rather than two
    /// `RefCell` borrows, so the cost this batching removed is smaller than
    /// it was. It is kept because the site is the hottest in the workspace —
    /// five times per `bench_loop.ns` iteration — and because the batched
    /// shape is what step 3's register-stack accessors want anyway; whether
    /// it still pays is a measurement, not an assumption.
```

- [ ] **Step 7: Narrow the teardown machinery to objects**

In `crates/ns-runtime/src/types.rs`'s `// ---------- Iterative teardown ----------`
section, a `FrameRef` no longer owns anything: it is an index, and the arena
outlives every handle into it. Delete `push_frame`, `drain_frame`,
`impl Drop for Frame`, and `Dying`'s `Frame` variant, together with
`run_teardown`'s arm for it. `push_closure` keeps its object work and loses its
frame work. Update the section's opening doc comment to say it now covers
objects only, and why: frames moved into the arena, which has no reference
chain to unwind.

- [ ] **Step 8: Build, test, and run both gates**

```bash
cargo build --workspace
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: no call site outside `heap.rs` and `types.rs` needed a change —
every accessor signature was already `&self`/`&mut self` and already copies
out, so borrow checking is unaffected. If a call site *does* fail to compile,
stop and report it rather than working around it: it means the seam had a hole
worth recording in `docs/design/memory.md`.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "perf: move activations into an arena

FrameRef becomes a 32-bit index into Heap.frames: no Rc box per
activation, no RefCell borrow on a register access, and no reference
chain to unwind, so Frame's iterative Drop and the frame half of
types.rs's teardown go with it.

Nothing reclaims a slot yet -- that needs the collector, Milestone 4's
step 5 -- so an activation is retained for the process's lifetime. See
docs/design/memory.md.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Objects into an arena

`ObjRef` becomes a 32-bit index into `Heap.objects`, and the unsafe
self-referential bootstrap and the rest of the teardown machinery go with it.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the `Heap` struct, `alloc_object`, `alloc_placeholder_object`, `alloc_self_referential_class`, all 29 object and class accessors; delete `new_object_handle` and `bootstrap_self_referential_object`)
- Modify: `crates/ns-runtime/src/types.rs` (`ObjRef`'s inner type and its three impls; delete the teardown section entirely)
- Test: `crates/ns-runtime/src/heap.rs`'s `mod tests`

**Interfaces:**
- Consumes: `Heap.frames` and `FrameRef`'s named-index shape from Task 2.
- Produces: `ObjRef { pub(crate) index: u32 }` and `Heap.objects: Vec<Object>`,
  mirroring Task 2's `FrameRef`. Every public `Heap` accessor signature is
  **unchanged**, `alloc_self_referential_class` included.

- [ ] **Step 1: Write the characterization test**

Add to `crates/ns-runtime/src/heap.rs`'s `mod tests`:

```rust
    /// The object counterpart of the frame test above: a handle must stay
    /// valid across the reallocation that growing the arena performs, and
    /// two objects with identical contents must never be the same object.
    #[test]
    fn an_object_handle_survives_the_arena_growing_under_it() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let first = heap.alloc_object(class.clone(), ObjectData::Str("first".to_string()));
        for _ in 0..1000 {
            heap.alloc_object(class.clone(), ObjectData::Str("filler".to_string()));
        }
        assert_eq!(heap.str_value(&first).as_deref(), Some("first"));
        assert!(heap.class_of_obj(&first).same(&class));

        let twin = heap.alloc_object(class, ObjectData::Str("first".to_string()));
        assert!(!first.same(&twin));
    }
```

- [ ] **Step 2: Run it to confirm it passes on `Rc` first**

Run: `cargo test -p ns-runtime --lib heap::tests::an_object_handle_survives_the_arena_growing_under_it`
Expected: PASS, for the same reason as Task 2 Step 2 — it pins a property the
arena could silently break, so it must hold on both representations.

- [ ] **Step 3: Commit the characterization test**

```bash
git add crates/ns-runtime/src/heap.rs
git commit -m "test: pin object handle stability and object identity

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Change the object handle's representation**

In `crates/ns-runtime/src/types.rs`, replace `ObjRef`'s definition and its
three impls with:

```rust
/// An opaque handle to an object: an index into `Heap.objects`. The inner
/// index is private to the crate, and only `types.rs` and `heap.rs` reach
/// it — that privacy is what made replacing `Rc<RefCell<Object>>` with
/// this a change to those two files rather than to every caller. See
/// `docs/design/memory.md`, "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot — there is no collector yet — so two live
/// objects never share an index.
#[derive(Clone)]
pub struct ObjRef {
    pub(crate) index: u32,
}

impl ObjRef {
    /// Object identity. Replaces `Rc::ptr_eq` at every call site, so that
    /// identity has one spelling that survives the store changing under it.
    pub fn same(&self, other: &ObjRef) -> bool {
        self.index == other.index
    }

    /// An opaque, hashable key unique to this handle, for code that needs
    /// `ObjRef` as a `HashMap`/`HashSet` key rather than a comparison --
    /// `identity_hash` cannot serve that role, because two placeholder
    /// objects may legitimately share hash 0
    /// (`Heap::alloc_placeholder_object`,
    /// `Heap::alloc_self_referential_class`), which would fold two distinct
    /// objects into one table entry. Crate-internal: every current caller
    /// (`image::encode::GraphTable`'s object-id map,
    /// `image::decode::rebuild_applications`'s superclass-position map,
    /// `interpreter::nested::in_progress_key`) lives inside `ns-runtime`.
    pub(crate) fn identity_key(&self) -> usize {
        self.index as usize
    }
}

/// Prints the arena index and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` recursing through the bootstrap `Object`
/// class, which is its own `.class`; an index has nothing to descend into,
/// so the recursion hazard is gone by construction. It still renders Rust
/// internals, which is why `CLAUDE.md`'s "Never `{:?}` a `Value`" rule
/// stands: `Heap::describe` is what a Newspeak-facing message needs.
impl std::fmt::Debug for ObjRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjRef(#{})", self.index)
    }
}
```

- [ ] **Step 5: Give `Heap` the object arena and drop the unsafe bootstrap**

In `crates/ns-runtime/src/heap.rs`, add the field:

```rust
    /// Every object, addressed by an `ObjRef`'s index. Nothing is ever
    /// removed, for the same reason as `frames`.
    objects: Vec<Object>,
```

and initialize it in `Heap::new`. Replace the three allocators and delete both
free functions (`new_object_handle` and `bootstrap_self_referential_object`,
whose `unsafe` block the arena makes unnecessary), keeping each allocator's
existing doc comment:

```rust
    pub fn alloc_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        let identity_hash = self.alloc_identity_hash();
        self.push_object(Object {
            class,
            identity_hash,
            data,
        })
    }

    pub fn alloc_placeholder_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        self.push_object(Object {
            class,
            identity_hash: 0,
            data,
        })
    }

    /// An object that is genuinely its own `.class`. Under `Rc` this needed
    /// `Rc::new_uninit`, a manual strong-count bump and `Rc::from_raw`,
    /// because `Object.class` is non-optional and `Rc::new_cyclic` only ever
    /// offers a `Weak` that fails to upgrade until construction finishes.
    /// An arena has no such problem: the slot's index is known before the
    /// slot is filled, so the object can name itself in safe Rust.
    ///
    /// Identity hash fixed at 0, like `alloc_placeholder_object`: none of
    /// the four callers links this object into a real hierarchy a hash
    /// could distinguish it within.
    pub fn alloc_self_referential_class(&mut self, data: ObjectData) -> ObjRef {
        let index = self.next_object_index();
        let handle = ObjRef { index };
        self.objects.push(Object {
            class: handle.clone(),
            identity_hash: 0,
            data,
        });
        handle
    }

    /// The one place an object enters the arena. The ceiling is the
    /// representation's, not an implementation limit: a handle is 32 bits
    /// by design (`docs/design/memory.md`), so a heap holding more than
    /// `u32::MAX` objects cannot be addressed at all, and failing loudly at
    /// the allocation beats handing out an index that wrapped.
    fn push_object(&mut self, object: Object) -> ObjRef {
        let index = self.next_object_index();
        self.objects.push(object);
        ObjRef { index }
    }

    fn next_object_index(&self) -> u32 {
        u32::try_from(self.objects.len()).expect("more than u32::MAX objects allocated in one heap")
    }
```

- [ ] **Step 6: Convert the 29 object and class accessors**

Same mechanical substitution as Task 2 Step 6: `obj.0.borrow()` becomes
`self.objects[obj.index as usize]`, `class.0.borrow()` becomes
`self.objects[class.index as usize]`, and the `borrow_mut()` forms become the same
behind `&mut self`. Convert every one of: `identity_hash`, `set_identity_hash`,
`class_of_obj`, `is_class`, `array_len`, `instance_field_count`,
`array_elements`, `array_at`, `array_set`, `array_push`, `instance_field`,
`set_instance_field`, `str_value`, `sym_value`, `closure_data`,
`object_snapshot`, `replace_object`, `class_application`, `class_enclosing`,
`class_declaring`, `class_superclass`, `class_builtin`, `class_lookup_step`,
`class_metaclass`, `set_class_metaclass`, `set_class_superclass`,
`set_class_application`. Keep every doc comment. The class accessors keep
answering `None` for a handle whose data is not a class — a class reference
decoded from an image is untrusted until verified, and the verifier is a
caller of these.

`describe` is the one with a nested read; it becomes:

```rust
            Value::Obj(obj) => match &self.objects[obj.index as usize].data {
                ObjectData::Str(s) => format!("the String {s:?}"),
                ObjectData::Sym(s) => format!("the Symbol #{s}"),
                ObjectData::Array(items) => format!("an Array of {}", items.len()),
                ObjectData::Closure(_) => "a Block".to_string(),
                ObjectData::Class(data) => {
                    format!("the class {}", data.application.mixin.name)
                }
                ObjectData::Instance(_) => {
                    let class = &self.objects[obj.index as usize].class;
                    match &self.objects[class.index as usize].data {
                        ObjectData::Class(data) => {
                            format!("an instance of {}", data.application.mixin.name)
                        }
                        // `types.rs`'s `ClassRef` doc: every `Object::class`
                        // is an `ObjRef` whose own `data` is
                        // `ObjectData::Class`, by convention rather than by
                        // the type system. This arm exists only so the match
                        // is total, not because a conforming object can
                        // reach it.
                        _ => "an object".to_string(),
                    }
                }
            },
```

Also update `Heap::new`'s doc comment: the paragraph warning that a fresh
`Heap` "is an *empty* arena" under the *planned* arena now describes the built
one, and the three entry points it names were fixed before this landed. Say
what is true: a `Heap` built on the spot is an empty arena, every handle from
another heap either indexes out of range and panics or names a different
object entirely, and Task 4's debug guard is what turns the second case into
the first.

- [ ] **Step 7: Delete the teardown section**

Delete `crates/ns-runtime/src/types.rs`'s entire
`// ---------- Iterative teardown ----------` section: `Dying`, `push_obj`,
`push_value`, `push_closure`, `drain_object_data`, `take_object_data`,
`run_teardown` and `impl Drop for Object`. Nothing an `Object` owns is a
reference-counted chain any more — a `Vec<Object>` drops entry by entry with
no recursion — so the machinery that existed to avoid a stack overflow on a
long chain has nothing left to unwind.

`mixin.rs`'s `impl Drop for MixinApplication` **stays**: an application chain
is still `Rc`-linked, and this plan does not touch mixins.

- [ ] **Step 8: Build, test, and run both gates**

```bash
cargo build --workspace
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: all green; both gates report their pinned numbers.

- [ ] **Step 9: Confirm the seam has no `RefCell` left**

Run: `grep -rn "borrow()\|borrow_mut()\|RefCell" crates --include=*.rs | grep -v with_borrow`
Expected: matches only in `crates/ns-runtime/src/symbol.rs` (the thread-local
interner, which is not part of the object model). No match in `heap.rs` or
`types.rs`.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "perf: move objects into an arena

ObjRef becomes a 32-bit index into Heap.objects. Rc and RefCell leave the
object model entirely: types.rs's iterative teardown has no reference
chain left to unwind and goes, and the self-referential bootstrap drops
its Rc::from_raw construction, since an arena slot can be named before it
is filled.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: A debug-only heap-identity guard

Reading a handle through the wrong `Heap` is the one failure mode this
representation introduces, and it is silent: index 5 in another heap is a
different, perfectly valid object, so the read succeeds and answers the wrong
thing. The seam migration hit this shape three separate times
(`compile_initializer`, `compile_module`, `cmd_disasm`) while `Heap` was still
a bare counter and the mistake cost nothing; it will recur.

Catch it where it happens, at zero release cost.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs`
- Modify: `crates/ns-runtime/src/types.rs`
- Test: `crates/ns-runtime/src/heap.rs`'s `mod tests`

**Interfaces:**
- Consumes: `ObjRef(u32)`/`FrameRef(u32)` and `Heap.objects`/`Heap.frames`.
- Produces: `Heap::object_handle(&self, index: u32) -> ObjRef` and
  `Heap::frame_handle(&self, index: u32) -> FrameRef` (both private), and,
  under `debug_assertions` only, a `heap_id` field on `Heap` and an `origin`
  field on each handle. Public accessor signatures are unchanged. There is
  deliberately **no** `heap_id()` accessor: the field does not exist in a
  release build, so a method returning it could not compile there.

- [ ] **Step 1: Write the failing test**

Add to `crates/ns-runtime/src/heap.rs`'s `mod tests`:

```rust
    /// Reading a handle through a heap that did not allocate it is the one
    /// failure the arena introduces and the one it cannot answer correctly:
    /// the index names a different, perfectly valid object. Debug builds
    /// must refuse it rather than answer.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn reading_a_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let class = origin.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let obj = origin.alloc_object(class, ObjectData::Str("mine".to_string()));

        let mut other = Heap::new();
        let other_class = other.alloc_self_referential_class(ObjectData::Instance(vec![]));
        other.alloc_object(other_class.clone(), ObjectData::Str("theirs".to_string()));
        other.alloc_object(other_class, ObjectData::Str("theirs".to_string()));

        // In range for `other` — which is exactly why this is silent
        // without the guard.
        let _ = other.str_value(&obj);
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p ns-runtime --lib heap::tests::reading_a_handle_through_a_foreign_heap_panics`
Expected: FAIL — the call answers `Some("theirs")` instead of panicking, so the
test fails with "note: test did not panic as expected".

- [ ] **Step 3: Give each heap an id and each handle its origin**

In `crates/ns-runtime/src/heap.rs`, add above `impl Heap`:

```rust
/// Hands every `Heap` a distinct id, so a handle can name the heap that
/// allocated it. Debug builds only — the id and the field on each handle
/// both compile out of a release build, where a handle is exactly its
/// 32-bit index.
#[cfg(debug_assertions)]
fn next_heap_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
```

Add a `#[cfg(debug_assertions)] heap_id: u32` field to `Heap`, initialized from
`next_heap_id()` in `Heap::new`. Do **not** add a `heap_id()` accessor: the
field does not exist in a release build, so nothing outside a `cfg`-guarded
position may name it.

Because the field on the handle is `cfg`-guarded too, a struct expression that
names it has to be guarded as well. Write that once, in the two private
constructors every allocation goes through, rather than at each allocation
site:

```rust
    /// The one place an `ObjRef` is built, so the `cfg` on `origin` appears
    /// once rather than at every allocation. A struct-expression field may
    /// carry a `cfg` attribute, which is what makes the release build see a
    /// plain 4-byte handle.
    fn object_handle(&self, index: u32) -> ObjRef {
        ObjRef {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
        }
    }

    fn frame_handle(&self, index: u32) -> FrameRef {
        FrameRef {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
        }
    }
```

In `crates/ns-runtime/src/types.rs`, add the matching field to both handles:

```rust
#[derive(Clone)]
pub struct ObjRef {
    pub(crate) index: u32,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap — which answers a
    /// different, valid object rather than failing — into a panic at the
    /// read. See `Heap::check_object_origin`.
    #[cfg(debug_assertions)]
    pub(crate) origin: u32,
}
```

and the same for `FrameRef`. `same` and `identity_key` keep reading
`self.index`, and `Debug` keeps printing it — neither may read `origin`, which
does not exist in a release build. Route every construction in `heap.rs`
through the two constructors above: `push_object`,
`alloc_self_referential_class` and `alloc_frame` are the only three, and each
replaces its `ObjRef { index }` / `FrameRef { index }` literal with
`self.object_handle(index)` / `self.frame_handle(index)`.

Note that `same` deliberately compares **only** the index: two handles from
different heaps comparing equal is the bug the guard catches at the read, and
making `same` a three-way answer would push that decision onto every call site.

- [ ] **Step 4: Check the origin on every access**

Add to `impl Heap`:

```rust
    /// Refuses a handle that another heap allocated. Compiles to nothing in
    /// a release build: the origin field does not exist there, so this is a
    /// no-op the optimizer removes entirely. The message names the two ids
    /// because the useful question when this fires is *which* extra heap
    /// got built — usually a `Heap::new()` standing in for one that is out
    /// of scope.
    #[inline]
    fn check_object_origin(&self, obj: &ObjRef) {
        #[cfg(debug_assertions)]
        assert_eq!(
            obj.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            obj.origin, self.heap_id
        );
        let _ = obj;
    }

    #[inline]
    fn check_frame_origin(&self, frame: &FrameRef) {
        #[cfg(debug_assertions)]
        assert_eq!(
            frame.origin, self.heap_id,
            "handle belongs to a different heap (handle from heap {}, read through heap {})",
            frame.origin, self.heap_id
        );
        let _ = frame;
    }
```

Call `self.check_object_origin(obj)` as the first line of every object and
class accessor, and `self.check_frame_origin(frame)` as the first line of every
frame accessor — the same 47 methods Tasks 2 and 3 converted. `describe` checks
the object it was handed; it need not re-check the class it reaches through,
which by construction came out of the same heap.

- [ ] **Step 5: Run the test to confirm it passes**

Run: `cargo test -p ns-runtime --lib heap::tests::reading_a_handle_through_a_foreign_heap_panics`
Expected: PASS.

- [ ] **Step 6: Confirm the guard costs nothing in release**

```bash
cargo build --workspace --release
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns run 50000
```

Expected: the bytecode median is unchanged against Task 3's, within noise.
Record both numbers in the commit message. If it is not within noise, stop and
report — a guard that costs measurable time in release is not doing what its
`cfg` claims.

- [ ] **Step 7: Run the full suite and both gates**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: all green. Both gates report their pinned numbers. A failure here is
a real find, not guard noise: it means production code reads a handle through
the wrong heap.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: refuse a handle read through a foreign heap in debug builds

An index from another heap is in range and names a different, valid
object, so the read answers the wrong thing and nothing reports it. The
seam migration hit that shape three times while Heap was still a bare
counter and it cost nothing; under the arena it is a silent defect.
Debug-only: the origin field and the check both compile out of release.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The handle types move into `heap.rs`

`CLAUDE.md` documents the seam as compiler-enforced only across the crate
boundary: inside `ns-runtime`'s own `src/`, `pub(crate)` is visible from every
module, so containment is a grep gate rather than something the compiler
refuses. `docs/design/memory.md` records that as a known rough edge and names
the fix — a shared parent module. Now that a handle is a bare index with no
`Drop` machinery around it, the fix is a move.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (gains the three type definitions)
- Modify: `crates/ns-runtime/src/types.rs` (loses them, re-exports them)
- Modify: `crates/ns-runtime/src/lib.rs`
- Modify: `CLAUDE.md`

**Interfaces:**
- Produces: `ObjRef`, `ClassRef` and `FrameRef` defined in `heap.rs` with
  **private** fields (no `pub(crate)`). `types.rs` keeps
  `pub use crate::heap::{ClassRef, FrameRef, ObjRef};` so both
  `ns_runtime::ObjRef` and `ns_runtime::types::ObjRef` keep resolving.

- [ ] **Step 1: Move the definitions**

Cut `ObjRef`, its `impl`s, its `Debug`, the `pub type ClassRef = ObjRef;` alias,
`FrameRef`, its `impl`s and its `Debug` out of `crates/ns-runtime/src/types.rs`
and paste them into `crates/ns-runtime/src/heap.rs`, directly above
`pub struct Heap`. Change every `pub(crate)` field to a private one — drop the
qualifier entirely, so `index` and `origin` are readable only from inside
`heap.rs`. `identity_key` stays `pub(crate)`: its callers are in `image/` and
`interpreter/`, not here.

- [ ] **Step 2: Re-export so no import path breaks**

In `crates/ns-runtime/src/types.rs`, at the top of the
`// ---------- Values and objects ----------` section:

```rust
/// The handle types live in `heap.rs`, whose `Heap` is the only thing that
/// can interpret one. They are re-exported here because this module is
/// where the object model reads as a whole, and because `lib.rs`'s
/// `pub use types::*` is how the rest of the workspace names them.
pub use crate::heap::{ClassRef, FrameRef, ObjRef};
```

In `crates/ns-runtime/src/lib.rs`, change `pub use heap::Heap;` to
`pub use heap::{ClassRef, FrameRef, Heap, ObjRef};`. Resolve the resulting
ambiguity with `pub use types::*` by removing the re-export from whichever of
the two produces a duplicate — keep `lib.rs`'s explicit one, and confirm with
`cargo build` that `ns_runtime::ObjRef` and `ns_runtime::types::ObjRef` both
still resolve.

- [ ] **Step 3: Build and confirm the compiler now enforces the seam**

```bash
cargo build --workspace
cargo test --workspace
```

Expected: green. To prove the enforcement is real rather than assumed, add a
temporary line in `crates/ns-runtime/src/dispatch.rs` reading
`let _ = some_obj_ref.index;`, run `cargo build -p ns-runtime`, confirm it
fails with `field 'index' of struct 'ObjRef' is private`, then delete the line.

- [ ] **Step 4: Retire the grep gate from `CLAUDE.md`**

In `CLAUDE.md`'s "Conventions", rewrite the bullet on object and frame access.
The seam is no longer a discipline checked by grep; state what it is now:

```markdown
- **Object and frame access goes through `crates/ns-runtime/src/heap.rs`, not
  directly through a handle.** `ObjRef`, `ClassRef` and `FrameRef` are defined
  in `heap.rs` and their inner arena index is private to that module, so the
  compiler refuses every read of one from anywhere else — there is no grep gate
  to remember and no way to widen a field to reach past an accessor. If an
  accessor you need is missing, add it there. The accessors copy out rather
  than lend out on purpose; see `docs/design/memory.md`, "The seam".
```

Also update the file's opening description of `ns-runtime` if it still calls
the object model `Rc`-based.

- [ ] **Step 5: Run the full suite and both gates**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: all green; both gates report their pinned numbers.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: make the heap seam compiler-enforced everywhere

The handle types move into heap.rs with private fields, so reading a
handle's index outside that module is a compile error rather than a grep
gate anyone has to remember. Retires the gate from CLAUDE.md.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Handles and `Value` become `Copy`

A handle is 4 bytes with no destructor, and `Value` is now a plain 16-byte
enum. Every `.clone()` on one is a memcpy the compiler already emits; the calls
are leftovers from a representation that no longer exists, and the next two
steps of Milestone 4 would carry them forward.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the derives)
- Modify: `crates/ns-runtime/src/types.rs` (`Value`'s derive)
- Modify: workspace-wide, mechanically, via `cargo clippy --fix`

**Interfaces:**
- Produces: `ObjRef: Copy`, `FrameRef: Copy` (hence `ClassRef: Copy`),
  `Value: Copy`. `Object`, `Frame`, `ClassData` and `ClosureData` stay
  `Clone`-only — each owns a `Vec`, a `String` or an `Rc`.

- [ ] **Step 1: Add the derives**

In `crates/ns-runtime/src/heap.rs`, change both handle derives from
`#[derive(Clone)]` to `#[derive(Clone, Copy)]`. In
`crates/ns-runtime/src/types.rs`, change `Value`'s from `#[derive(Debug, Clone)]`
to `#[derive(Debug, Clone, Copy)]`.

- [ ] **Step 2: Run clippy to see the redundant clones**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -c clone_on_copy`
Expected: a non-zero count (roughly 200–300 sites across the workspace).

- [ ] **Step 3: Apply the mechanical fix**

```bash
cargo clippy --fix --workspace --all-targets --allow-dirty
cargo fmt --all
```

- [ ] **Step 4: Clean up what the fix could not**

Run: `cargo clippy --workspace --all-targets`
Expected: warning-free. Where it is not, the remaining sites are ones
`clippy --fix` will not touch on its own — typically `std::mem::take` or
`Option::take` on a now-`Copy` field, and `iter().cloned()` on a `Copy`
element. Fix each by hand; do not silence one with an `#[allow]`.

- [ ] **Step 5: Run the full suite and both gates**

```bash
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: all green; both gates report their pinned numbers. This task changes
no behaviour — a failure means the mechanical fix rewrote something it should
not have, so read the diff at the failing site rather than adjusting the test.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: make handles and Value Copy

A handle is a 4-byte index with no destructor and Value is a plain
16-byte enum, so every .clone() on one was a memcpy the compiler already
emitted. Mechanical, no behaviour change.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Measure, and write down what was built

The milestone's whole argument for this change is a measurement, so it ends
with one. Two numbers matter: what a send costs now, and what never reclaiming
a slot costs in memory.

**Files:**
- Modify: `docs/design/memory.md`
- Modify: `docs/design/object-model.md`
- Modify: `docs/design/execution.md`
- Modify: `docs/design/README.md`
- Modify: `docs/PROGRESS.md`
- Modify: `crates/ns-cli/src/main.rs` (the `cmd_disasm` comment about a fresh `Heap`)

- [ ] **Step 1: Measure the benchmark on a release build**

```bash
cargo build --workspace --release
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns run 50000
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns run 50000
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns run 50000
```

Record all three bytecode medians and all three tree-walk medians. The baseline
to compare against is the post-seam figure in `docs/PROGRESS.md`: bytecode
medians of 19.71, 19.59 and 19.72 ms.

- [ ] **Step 2: Measure peak memory**

The arena reclaims nothing, so this is the number that says what the interim
costs. On Windows, in PowerShell:

```powershell
$p = Start-Process -FilePath ".\target\release\ns-next.exe" `
  -ArgumentList "bench","tests-fixtures/bench_loop.ns","run","50000" `
  -PassThru -NoNewWindow
$p.WaitForExit()
"{0:N1} MB" -f ($p.PeakWorkingSet64 / 1MB)
```

Run the same against the pre-arena binary for a comparison: build it from the
merge-base commit into a separate `git worktree add` checkout. If that is more
trouble than the number is worth, record the arena figure alone and say so.
Record the figure either way; it is the number step 5 will be judged against.

- [ ] **Step 3: Update `docs/design/memory.md`**

Rewrite "Planned: an arena, handles, and a real collector" as two parts: what
is built (the arena and the handle representation, with the measured figures),
and what remains planned (the root discipline and the collector), keeping the
existing argument for each. Specifically:

- The "The representation: an arena of typed handles" section describes a
  built thing now, not a planned one.
- The "Known rough edges in the seam" list: strike the compiler-enforcement
  gap, which Task 5 closed. Keep the accessor naming grown by accretion and
  the `heap` parameter's inconsistent position — the arena changed neither.
- **Correct that list's copy-out entry rather than striking it.** It claims
  `builtins/text.rs`'s payload clone and `object_snapshot`/`frame_snapshot`'s
  double clone "disappear once a handle indexes an arena instead of owning an
  `Rc`". They do not. Copy-out is a property of the accessor's return type,
  not of the store: `str_value` still returns an owned `String`. What the
  arena changed is that a *borrowing* accessor became expressible at all —
  the `Ref` guard that made one unusable is gone, so an accessor could now
  lend out `&str` tied to `&Heap`. Whether to add one is a measurement, and
  the sites named are still not on a measured path. Say that instead.
- **Answer the open question "whether a handle is globally unique or
  arena-relative"** — this plan settled it: arena-relative, an index
  meaningful only against the `Heap` that allocated it, with the debug origin
  guard making that explicit rather than conventional. Record that the room
  the actor milestone needs is still there: a handle is 32 bits and nothing
  reads its bits outside `heap.rs`, so widening it to carry a heap tag is a
  change to one module.
- In "Decisions", retire **"Iterative `Drop` rather than a depth ceiling"** for
  objects and frames: the machinery it describes is gone, because an arena has
  no reference chain to unwind. `MixinApplication`'s chain still has one, so
  the decision survives for mixins — say which.
- Add a decision recording the debug-only origin guard and why it exists.
- Add, under what the collector owns, that neither arena reclaims a slot today
  and what that measured (Step 2's figure).

- [ ] **Step 4: Update the other design documents**

- `docs/design/object-model.md`: "Planned: arena allocation and handles"
  becomes a description of the built representation. Its "**An activation is an
  object too**" paragraph must now distinguish what landed (a frame is an arena
  entry) from what did not (a frame is not yet a stack entry reified on
  demand) and point at `execution.md` for the rest. Its "**`become:` becomes
  expressible**" paragraph is still forward-looking — it waits on the collector
  — so say that.
- `docs/design/execution.md`: in the measurements section, add the post-arena
  figures beside the post-seam ones, and say what moved and what did not. Be
  precise about why the activation cost only partly moved: an arena entry
  removes the `Rc` box and the `RefCell` borrow, but `Frame.args` and
  `Frame.temps` are still two `Vec` allocations per activation, and those are
  step 3's to remove.
- `docs/design/README.md`: update the status line for `memory.md` and
  `object-model.md`.

- [ ] **Step 5: Update `docs/PROGRESS.md`**

Add an "M4 — The object and frame arena" section in the same shape as
"M4 — The heap seam": what it replaced, what it deleted, the measured
before/after, and the interim cost that has no collector behind it yet. Update
"Where the next session starts": step 1 is done, so the numbered list now
starts at the root discipline, and the paragraph naming the heap seam as
groundwork gains the arena beside it. Link this plan file.

- [ ] **Step 6: Correct the stale comment in `ns-cli`**

`crates/ns-cli/src/main.rs` around line 953 explains why each arm reads
`root_mixin` before its interpreter drops, in terms of a future arena ("a
moving arena's fresh `Heap` starts out holding nothing, and the read would
silently miss"). The arena exists now, and what happens is worse than a miss:
in a debug build the origin guard panics, and in a release build the index
names a different object. Say that.

- [ ] **Step 7: Verify the documentation supports a fresh session**

Read `docs/design/README.md`, `docs/PROGRESS.md` and `CLAUDE.md` as though you
had never seen this repository. Confirm that a reader arriving at them can tell
what the object representation is today, what step of Milestone 4 comes next,
and where its design lives — without reading this plan or any commit message.
Fix whatever fails that reading.

- [ ] **Step 8: Run the full suite and both gates one last time**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: all green; both gates report their pinned numbers.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "docs: record the object and frame arena as built

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Verification checklist for the whole branch

Before finishing the branch, confirm each of these by running it, not by
recalling it:

- [ ] `cargo clippy --workspace --all-targets` is warning-free.
- [ ] `cargo test --workspace` passes.
- [ ] `parse-corpus` reports `parsed 210, failed 0, expected-fail 1, stale 0`.
- [ ] `run-corpus` reports `ran 6, failed 0, skipped 205`.
- [ ] `grep -rn "borrow()\|borrow_mut()\|RefCell" crates --include=*.rs | grep -v with_borrow`
      matches only `crates/ns-runtime/src/symbol.rs`.
- [ ] `grep -rn "unsafe" crates --include=*.rs` matches nothing under
      `crates/ns-runtime/src/heap.rs`.
- [ ] Reading an `ObjRef`'s field from outside `heap.rs` is a compile error
      (re-run Task 5 Step 3's temporary check).
- [ ] The release-build bench medians are recorded in `docs/PROGRESS.md`
      against the 19.59–19.72 ms post-seam baseline.
- [ ] Peak memory on `bench_loop.ns` is recorded, with the note that nothing
      reclaims a slot until step 5.
