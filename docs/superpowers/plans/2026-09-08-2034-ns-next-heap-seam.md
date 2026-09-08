# The Heap Seam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every read and write of an `Object` or a `Frame` through one
`Heap` type, and make `ObjRef`, `ClassRef` and `FrameRef` opaque newtypes, so
that swapping what sits behind a handle changes one file instead of 193 call
sites.

**Architecture:** A new `crates/ns-runtime/src/heap.rs` holds `Heap` plus typed
copy-out accessors — `heap.field(obj, i)`, `heap.class_of_obj(obj)`,
`heap.receiver(frame)` — rather than a general `get() -> &Object`. Almost every
existing call site already borrows, clones one field and drops the borrow, so
copy-out accessors match what the code does; a general reference-returning
accessor would fight the borrow checker at every site that calls back into the
interpreter while holding a reference, which is most of `eval.rs`. Behind the
accessors the store stays `Rc<RefCell<_>>` for this whole plan. **Nothing about
observable behaviour or performance changes here.** The arena that replaces the
store is the next plan.

**Tech Stack:** Rust 1.98.0, edition 2021. No new dependencies.

**Spec:** `docs/design/memory.md` ("Planned: an arena, handles, and a real
collector"), `docs/design/object-model.md` ("Planned: arena allocation and
handles"), `docs/design/execution.md` ("Planned: activations on a stack, reified
on demand"). Ordering rationale and the measurements behind it are in
`docs/PROGRESS.md`, "Where the next session starts".

## Global Constraints

- **Rust 1.98.0, edition 2021.** No new external dependency without a reason that
  survives review.
- **All code, identifiers, comments and commit messages in English.** Classic
  terminology: `blacklist`, `whitelist`, `sane`.
- **Conventional commits** (`feat:`, `fix:`, `perf:`, `test:`, `docs:`,
  `refactor:`, `chore:`).
- **The two engines must agree byte for byte** — identical exit status, stdout
  and stderr for every program. `crates/ns-cli/tests/differential.rs` enforces it
  with no exemptions.
- **Never copy dispatch logic, lookup logic or error text out of `interpreter/`**
  into `vm.rs` or `compiler/`. Widen to `pub(crate)` instead.
- **Never `{:?}` a `Value`**, and never `.unwrap()` / `.expect()` / `{:?}` an
  `EvalResult`. `Object`'s bootstrap class is its own `.class`, so `Debug`
  recurses forever; `Signal` has no `Debug` on purpose. Tests unwrap through a
  local `expect_ok` helper — copy the one in a neighbouring test file.
- **After every task**, all four of these must hold:
  - `cargo test --workspace` — 0 failed
  - `cargo clippy --workspace --all-targets` — warning-free
  - `cargo run -p ns-cli -- parse-corpus corpus/modules` — `parsed 210, failed 0, expected-fail 1, stale 0`
  - `cargo run -p ns-cli -- run-corpus corpus/modules` — `ran 6, failed 0, skipped 205`
- **`cargo fmt --all` before every commit.**
- Never judge performance from a debug build; `cargo build --workspace --release`
  is the one to measure.

## How this plan is gated

Tasks 2 through 10 are per-file migrations. A migration that silently skips a
call site still leaves the suite green, so **each of those tasks carries a
mechanical completeness check**: after the task, the file must contain zero
`.borrow()` / `.borrow_mut()` occurrences. Task 13 is the real proof — flipping
the aliases to newtypes makes any missed site a compile error — but the per-task
grep is what keeps that final task from becoming a pile of surprises.

Current counts, for reference:

| File | `.borrow()` sites | Task |
|---|---|---|
| `interpreter/eval.rs` | 21 | 5 |
| `interpreter/mod.rs` | 12 | 6 |
| `image/decode.rs` | 12 | 9 |
| `vm.rs` | 9 | 8 |
| `image/encode.rs` | 9 | 9 |
| `interpreter/instantiate.rs` | 8 | 7 |
| `builtins/collections.rs` | 7 | 3 |
| `dispatch.rs` | 6 | 4 |
| `builtins/text.rs` | 5 | 3 |
| `ns-cli/src/main.rs` | 4 | 10 |
| `interpreter/nested.rs` | 3 | 6 |
| `builtins/object_class.rs` | 3 | 3 |
| `symbol.rs`, `platform.rs`, `compiler/mod.rs` | 2 each | 10 |
| `types.rs`, `image/mixins.rs`, `builtins/mod.rs`, `builtins/block.rs` | 1 each | 3, 9, 10 |
| **`crates/*/src` total** | **193** | 1–10 |
| `tests/image.rs` | 50 | 12 |
| the other thirteen files under `tests/` | 33 | 11 |
| **`crates/*/tests` total** | **83** | 11–12 |

Beyond the borrows there are 20 `Rc::ptr_eq` uses (10 under `src`, 10 under
`tests`) and 11 direct `Rc::new(RefCell::new(Object { … }))` constructions
(2 under `src` plus the two unsafe self-referential bootstraps, 9 under
`tests`). All of those are Task 13's, replaced in one pass so that identity and
allocation never have two spellings in the tree at once.

---

### Task 1: `Heap`, the identity counter, and the first accessors

**Files:**
- Create: `crates/ns-runtime/src/heap.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (add `pub mod heap;` and re-export `Heap`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:81` (replace the
  `next_identity_hash: u32` field with `pub(crate) heap: Heap`), and
  `interpreter/mod.rs:200-220` (`alloc_identity_hash`,
  `advance_identity_hash_past` delegate to it)
- Test: `crates/ns-runtime/tests/heap.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ns_runtime::heap::Heap` with `Heap::new() -> Heap`,
  `alloc_identity_hash(&mut self) -> u32`,
  `advance_identity_hash_past(&mut self, max: u32)`,
  `identity_hash(&self, obj: &ObjRef) -> u32`,
  `class_of_obj(&self, obj: &ObjRef) -> ClassRef`,
  `is_class(&self, obj: &ObjRef) -> bool`,
  `array_len(&self, obj: &ObjRef) -> Option<usize>`,
  `array_elements(&self, obj: &ObjRef) -> Option<Vec<Value>>`,
  `array_at(&self, obj: &ObjRef, index: usize) -> Option<Value>`,
  `array_set(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool`,
  `array_push(&mut self, obj: &ObjRef, value: Value) -> bool`,
  `str_value(&self, obj: &ObjRef) -> Option<String>`,
  `sym_value(&self, obj: &ObjRef) -> Option<Symbol>`,
  `closure_data(&self, obj: &ObjRef) -> Option<ClosureData>`.
  `Interpreter::heap` is `pub(crate)`; `Interpreter::alloc_identity_hash` keeps
  its present public signature.

- [ ] **Step 1: Write the failing test**

Create `crates/ns-runtime/tests/heap.rs`:

```rust
//! The heap seam's own tests. Everything here is about the accessor
//! contract, not about what sits behind it -- which is the point of the
//! seam.

use ns_runtime::heap::Heap;
use ns_runtime::Interpreter;
use ns_runtime::types::{ObjectData, Value};

#[test]
fn identity_hashes_are_monotonic_and_start_at_one() {
    let mut heap = Heap::new();
    assert_eq!(heap.alloc_identity_hash(), 1);
    assert_eq!(heap.alloc_identity_hash(), 2);
    assert_eq!(heap.alloc_identity_hash(), 3);
}

#[test]
fn advancing_past_a_maximum_never_moves_the_counter_backwards() {
    let mut heap = Heap::new();
    heap.advance_identity_hash_past(10);
    assert_eq!(heap.alloc_identity_hash(), 11);
    // A lower maximum must not rewind: an image's ids and freshly
    // allocated ones would then collide.
    heap.advance_identity_hash_past(3);
    assert_eq!(heap.alloc_identity_hash(), 12);
}

#[test]
fn array_accessors_read_and_write_through_the_seam() {
    let mut interp = Interpreter::bootstrap();
    let obj = interp.new_array(vec![Value::SmallInt(1), Value::SmallInt(2)]);

    assert_eq!(interp.heap_ref().array_len(&obj), Some(2));
    assert!(matches!(
        interp.heap_ref().array_at(&obj, 0),
        Some(Value::SmallInt(1))
    ));

    assert!(interp.heap_mut().array_set(&obj, 0, Value::SmallInt(9)));
    assert!(matches!(
        interp.heap_ref().array_at(&obj, 0),
        Some(Value::SmallInt(9))
    ));

    assert!(interp.heap_mut().array_push(&obj, Value::SmallInt(3)));
    assert_eq!(interp.heap_ref().array_len(&obj), Some(3));
}

#[test]
fn an_array_accessor_on_a_non_array_answers_none_rather_than_panicking() {
    let mut interp = Interpreter::bootstrap();
    let obj = interp.new_string("not an array".to_string());
    assert_eq!(interp.heap_ref().array_len(&obj), None);
    assert!(interp.heap_ref().array_at(&obj, 0).is_none());
    assert!(!interp.heap_mut().array_push(&obj, Value::Nil));
}

#[test]
fn string_and_symbol_accessors_answer_only_for_their_own_shape() {
    let mut interp = Interpreter::bootstrap();
    let s = interp.new_string("hello".to_string());
    assert_eq!(interp.heap_ref().str_value(&s), Some("hello".to_string()));
    assert_eq!(interp.heap_ref().sym_value(&s), None);
}

#[test]
fn is_class_distinguishes_a_class_object_from_an_instance() {
    let mut interp = Interpreter::bootstrap();
    let s = interp.new_string("x".to_string());
    let class = interp.class_of(&Value::Obj(s.clone()));
    assert!(!interp.heap_ref().is_class(&s));
    assert!(interp.heap_ref().is_class(&class));
}
```

Note the assertion shapes. `Value` derives `Clone` and `Debug` only — it has no
`PartialEq` and must not gain one, since that would invite structural equality
where Newspeak wants identity. So `assert_eq!` never takes a `Value` or an
`Option<Value>`: use `matches!` for a `Value`, `.is_none()` for an absent one,
and `assert_eq!` only for `Option<usize>`, `Option<String>` and `u32`, exactly
as written above. And `Value` is never `{:?}`-formatted, because `Object`'s
bootstrap class is its own `.class` and `Debug` would recurse until the stack
gives out.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test heap`
Expected: FAIL to compile — `unresolved import ns_runtime::heap`.

- [ ] **Step 3: Write `heap.rs`**

Create `crates/ns-runtime/src/heap.rs`:

```rust
//! The heap seam. Every read and write of an `Object` -- and, from Task 5,
//! of a `Frame` -- goes through `Heap`, so that replacing what sits behind
//! a handle changes this file rather than its call sites. The store behind
//! the accessors is `Rc<RefCell<_>>` today and an arena of typed handles
//! next; see `docs/design/memory.md`, "Planned: an arena, handles, and a
//! real collector".
//!
//! The accessors copy out rather than lend out. That is not a stylistic
//! choice: almost every call site borrows, clones one field and drops the
//! borrow, and a reference-returning accessor would be unusable at the
//! sites that call back into the interpreter while holding one -- which is
//! most of `interpreter/eval.rs`.

use crate::types::*;
use crate::Symbol;

#[derive(Debug)]
pub struct Heap {
    /// Monotonic, assigned at allocation, never derived from an address, so
    /// identity survives both a moving collector and a snapshot
    /// (`docs/design/memory.md`).
    next_identity_hash: u32,
}

impl Default for Heap {
    fn default() -> Self {
        Heap::new()
    }
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            next_identity_hash: 1,
        }
    }

    /// Saturating rather than wrapping: an exhausted counter must not hand
    /// out an id a live object already holds.
    pub fn alloc_identity_hash(&mut self) -> u32 {
        let h = self.next_identity_hash;
        self.next_identity_hash = self.next_identity_hash.saturating_add(1);
        h
    }

    /// Moves the counter past everything an image brought with it. Never
    /// moves it backwards -- a lower maximum from a second load would
    /// otherwise let a fresh allocation collide with a decoded object.
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        if self.next_identity_hash <= max {
            self.next_identity_hash = max.saturating_add(1);
        }
    }

    // ---------- Object accessors ----------
    //
    // `&self` is unused while a handle is an `Rc`, because the data hangs
    // off the handle itself. It is in these signatures from the start
    // because the arena will need it, and adding it afterwards would touch
    // every call site this module exists to protect. The mutating
    // accessors take `&mut self` for the same reason, even though the
    // `RefCell` behind the handle would make `&self` sufficient today.

    pub fn identity_hash(&self, obj: &ObjRef) -> u32 {
        obj.borrow().identity_hash
    }

    pub fn class_of_obj(&self, obj: &ObjRef) -> ClassRef {
        obj.borrow().class.clone()
    }

    pub fn is_class(&self, obj: &ObjRef) -> bool {
        matches!(obj.borrow().data, ObjectData::Class(_))
    }

    pub fn array_len(&self, obj: &ObjRef) -> Option<usize> {
        match &obj.borrow().data {
            ObjectData::Array(items) => Some(items.len()),
            _ => None,
        }
    }

    pub fn array_elements(&self, obj: &ObjRef) -> Option<Vec<Value>> {
        match &obj.borrow().data {
            ObjectData::Array(items) => Some(items.clone()),
            _ => None,
        }
    }

    pub fn array_at(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        match &obj.borrow().data {
            ObjectData::Array(items) => items.get(index).cloned(),
            _ => None,
        }
    }

    /// Answers whether the write happened. A wrong-shape receiver or an
    /// out-of-range index is a caller's concern -- the natives that reach
    /// here have already produced their own Newspeak error for it -- so
    /// this reports rather than panics.
    pub fn array_set(&mut self, obj: &ObjRef, index: usize, value: Value) -> bool {
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            if index < items.len() {
                items[index] = value;
                return true;
            }
        }
        false
    }

    pub fn array_push(&mut self, obj: &ObjRef, value: Value) -> bool {
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            items.push(value);
            return true;
        }
        false
    }

    pub fn str_value(&self, obj: &ObjRef) -> Option<String> {
        match &obj.borrow().data {
            ObjectData::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn sym_value(&self, obj: &ObjRef) -> Option<Symbol> {
        match &obj.borrow().data {
            ObjectData::Sym(s) => Some(*s),
            _ => None,
        }
    }

    pub fn closure_data(&self, obj: &ObjRef) -> Option<ClosureData> {
        match &obj.borrow().data {
            ObjectData::Closure(c) => Some(c.clone()),
            _ => None,
        }
    }
}
```

- [ ] **Step 4: Wire it into the crate and the interpreter**

In `crates/ns-runtime/src/lib.rs`, add alongside the existing module
declarations and re-exports:

```rust
pub mod heap;
pub use heap::Heap;
```

In `crates/ns-runtime/src/interpreter/mod.rs`, replace the
`next_identity_hash: u32` field on `Interpreter` with:

```rust
    pub(crate) heap: Heap,
```

initialize it in both `Interpreter::new` and `Interpreter::bootstrap` with
`heap: Heap::new()` in place of `next_identity_hash: 1`, and replace the two
methods' bodies with delegations, keeping their signatures and doc comments:

```rust
    pub fn alloc_identity_hash(&mut self) -> u32 {
        self.heap.alloc_identity_hash()
    }

    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        self.heap.advance_identity_hash_past(max);
    }
```

Add the two test-facing accessors, next to them:

```rust
    /// The heap, for reading. Public so integration tests can exercise the
    /// seam directly; the field itself stays `pub(crate)` so no caller
    /// outside the crate can hold one across a call back into the
    /// interpreter.
    pub fn heap_ref(&self) -> &Heap {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test heap`
Expected: PASS, 6 passed.

- [ ] **Step 6: Run the full gates**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: 0 failed; no warnings; `parsed 210, failed 0, expected-fail 1, stale 0`;
`ran 6, failed 0, skipped 205`.

- [ ] **Step 7: Commit**

```bash
git add crates/ns-runtime/src/heap.rs crates/ns-runtime/src/lib.rs \
        crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/tests/heap.rs
git commit -m "refactor: add the heap seam and move the identity counter onto it"
```

---

### Task 2: Record the seam in the design documents

Doing this now rather than at the end means the rule the following nine tasks
follow is written down before they are executed, which is what makes a reviewer
able to reject a task for breaking it.

**Files:**
- Modify: `docs/design/memory.md` (a new "The seam" subsection under
  "Planned: an arena, handles, and a real collector")
- Modify: `CLAUDE.md` (one new entry under "Invariants that are easy to break")

- [ ] **Step 1: Add the subsection to `docs/design/memory.md`**

Insert immediately before the existing `### The representation: an arena of typed
handles`:

```markdown
### The seam

`crates/ns-runtime/src/heap.rs` is the single place that knows what a handle
is. Every read and write of an `Object` or a `Frame` goes through a `Heap`
accessor, so replacing the store behind them touches that file rather than the
193 call sites the runtime had before the seam existed.

The accessors copy out rather than lend out. That follows what the call sites
already did — borrow, clone one field, drop the borrow — and it is what keeps
the arena from needing a rooting discipline at every site that calls back into
the interpreter while holding a reference. Mutating accessors take `&mut self`
even where the present store would not require it, so that the arena is a
change of implementation rather than a second pass over every caller.
```

- [ ] **Step 2: Add the invariant to `CLAUDE.md`**

Append to the "Invariants that are easy to break" section, in the same voice as
its neighbours:

```markdown
**Object and frame access goes through `heap.rs`, never through a handle
directly.** `ObjRef`, `ClassRef` and `FrameRef` are opaque newtypes whose
contents only `heap.rs` and `types.rs` can reach. A `.borrow()` anywhere else
is the thing the seam exists to prevent: it is what made a representation
change a 193-site edit, and re-introducing one puts the next change back where
this one started. Accessors copy out on purpose — if you want to lend out a
reference instead, you are writing the rooting problem the arena's scope
discipline exists to answer.
```

- [ ] **Step 3: Verify the documents still cross-reference correctly**

Run: `grep -n "heap.rs" docs/design/memory.md CLAUDE.md`
Expected: at least one hit in each.

- [ ] **Step 4: Commit**

```bash
git add docs/design/memory.md CLAUDE.md
git commit -m "docs: record the heap seam and the rule that protects it"
```

---

### Task 3: Migrate `builtins/`

The smallest coherent group, and the one that exercises the accessor set Task 1
defined before nine harder files depend on it. 17 sites across five files.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/collections.rs` (7 sites),
  `builtins/text.rs` (5), `builtins/object_class.rs` (3), `builtins/mod.rs` (1),
  `builtins/block.rs` (1)
- Modify: `crates/ns-runtime/src/heap.rs` (add `ordered_collection_len`, below)
- Test: existing `crates/ns-runtime/tests/builtins_collections_block.rs` and
  `builtin_object_inheritance.rs` cover this; no new test file.

**Interfaces:**
- Consumes: every accessor Task 1 produced.
- Produces: no new public API beyond one accessor added below.

- [ ] **Step 1: Confirm the existing tests pass before touching anything**

Run: `cargo test -p ns-runtime --test builtins_collections_block --test builtin_object_inheritance`
Expected: PASS. This is the baseline the migration must not move.

- [ ] **Step 2: Migrate `builtins/collections.rs`**

Each of the seven sites becomes an accessor call. The two mutating ones are the
interesting pair — `at:put:` at line 135 and `add:` at line 160. Before:

```rust
        let len = match &obj.borrow().data {
            ObjectData::Array(items) => items.len(),
            _ => return None,
        };
        // ...
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            items[(index as usize) - 1] = value;
        }
        return Some(Ok(receiver.clone()));
```

After:

```rust
        let Some(len) = interp.heap.array_len(obj) else {
            return None;
        };
        // ...
        interp.heap.array_set(obj, (index as usize) - 1, value);
        return Some(Ok(receiver.clone()));
```

The `add:` site at line 154-162, before:

```rust
        if !matches!(&obj.borrow().data, ObjectData::Array(_)) {
            return None;
        }
        let value = args.first().cloned().unwrap_or(Value::Nil);
        if let ObjectData::Array(items) = &mut obj.borrow_mut().data {
            items.push(value.clone());
        }
        return Some(Ok(value));
```

After — note that `array_push` reports its own shape check, so the separate
`matches!` guard would now be a second read of the same fact. Keep the guard,
because it decides `None` (fall through to `dispatch_common`) versus a write,
and `array_push`'s `false` cannot be distinguished from a successful write of a
different shape later:

```rust
        if interp.heap.array_len(obj).is_none() {
            return None;
        }
        let value = args.first().cloned().unwrap_or(Value::Nil);
        interp.heap.array_push(obj, value.clone());
        return Some(Ok(value));
```

The `do:` site at line 84 becomes `interp.heap.array_elements(obj)`, and the
`size` and `at:` sites at 52, 57 and 117 become `array_len` and `array_at`.

- [ ] **Step 3: Migrate `builtins/text.rs`, `object_class.rs`, `mod.rs`, `block.rs`**

`text.rs` sites 55, 89, 116 and 20 become `heap.str_value(obj)` and
`heap.sym_value(obj)`; site 96 compares two symbols, which becomes
`heap.sym_value(arg_obj) == Some(sym)`.

`object_class.rs:53` becomes `interp.heap.identity_hash(obj)`. Sites 67 and 69
walk an array of symbols and become `array_elements` plus `sym_value`.

`builtins/mod.rs:148` in `call_block` becomes:

```rust
            let closure_data = interp.heap.closure_data(obj);
```

`block.rs:22` reads `numArgs` off a closure and becomes
`interp.heap.closure_data(obj).map(|c| c.params.len())`.

- [ ] **Step 4: Verify the migration is complete**

Run: `grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/builtins/*.rs`
Expected: `0` for every one of the five files.

- [ ] **Step 5: Run the full gates**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: 0 failed; no warnings; `210/0/1/0`; `ran 6, failed 0, skipped 205`.

- [ ] **Step 6: Commit**

```bash
git add crates/ns-runtime/src/builtins crates/ns-runtime/src/heap.rs
git commit -m "refactor: route builtins through the heap seam"
```

---

### Task 4: Migrate `dispatch.rs`

**Files:**
- Modify: `crates/ns-runtime/src/dispatch.rs` (6 sites, plus the unsafe
  self-referential bootstrap at 283-300 and the object construction at 348)
- Modify: `crates/ns-runtime/src/heap.rs` (add the `ClassData` accessors below)
- Modify: every caller of the `dispatch::` free functions whose signature gains
  a `&Heap`: `interpreter/mod.rs`, `interpreter/nested.rs`, `image/encode.rs`,
  `image/decode.rs`

**Interfaces:**
- Consumes: Task 1's accessors.
- Produces: on `Heap` —
  `class_application(&self, class: &ClassRef) -> Option<Rc<MixinApplication>>`,
  `class_enclosing(&self, class: &ClassRef) -> Option<ObjRef>`,
  `class_declaring(&self, class: &ClassRef) -> Option<ClassRef>`,
  `class_superclass(&self, class: &ClassRef) -> Option<ClassRef>`,
  `class_builtin(&self, class: &ClassRef) -> Option<BuiltinClass>`,
  `class_metaclass(&self, class: &ClassRef) -> Option<ClassRef>`,
  `set_class_metaclass(&mut self, class: &ClassRef, metaclass: ClassRef)`.
  The free functions in `dispatch` that read a class take `heap: &Heap` as their
  new first parameter.

- [ ] **Step 1: Add the `ClassData` accessors to `heap.rs`**

```rust
    // ---------- Class accessors ----------
    //
    // Each answers `None` for a handle whose data is not a class, rather
    // than panicking: a class reference decoded from an image is untrusted
    // until verified, and the verifier is a caller of these.

    pub fn class_application(
        &self,
        class: &ClassRef,
    ) -> Option<std::rc::Rc<crate::mixin::MixinApplication>> {
        match &class.borrow().data {
            ObjectData::Class(data) => Some(data.application.clone()),
            _ => None,
        }
    }

    pub fn class_enclosing(&self, class: &ClassRef) -> Option<ObjRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.enclosing.clone(),
            _ => None,
        }
    }

    pub fn class_declaring(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.declaring_class.clone(),
            _ => None,
        }
    }

    pub fn class_superclass(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.superclass.clone(),
            _ => None,
        }
    }

    pub fn class_builtin(&self, class: &ClassRef) -> Option<BuiltinClass> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.builtin,
            _ => None,
        }
    }

    pub fn class_metaclass(&self, class: &ClassRef) -> Option<ClassRef> {
        match &class.borrow().data {
            ObjectData::Class(data) => data.metaclass.clone(),
            _ => None,
        }
    }

    pub fn set_class_metaclass(&mut self, class: &ClassRef, metaclass: ClassRef) {
        if let ObjectData::Class(data) = &mut class.borrow_mut().data {
            data.metaclass = Some(metaclass);
        }
    }
```

- [ ] **Step 2: Write the failing test for the class accessors**

Append to `crates/ns-runtime/tests/heap.rs`:

```rust
#[test]
fn class_accessors_answer_none_for_an_instance() {
    let mut interp = Interpreter::bootstrap();
    let s = interp.new_string("x".to_string());
    assert!(interp.heap_ref().class_application(&s).is_none());
    assert!(interp.heap_ref().class_superclass(&s).is_none());
    assert!(interp.heap_ref().class_builtin(&s).is_none());
}

#[test]
fn a_builtin_class_reports_its_kind_through_the_seam() {
    let mut interp = Interpreter::bootstrap();
    let s = interp.new_string("x".to_string());
    let class = interp.class_of(&Value::Obj(s));
    assert!(interp.heap_ref().class_builtin(&class).is_some());
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test heap`
Expected: FAIL to compile — `no method named class_application`.

- [ ] **Step 4: Migrate `dispatch.rs`'s six read sites**

Each of them reads `ObjectData::Class(data)` off a `ClassRef`; replace with the
matching accessor. `dispatch::class_application` already exists as a free
function — it becomes a thin caller of `heap.class_application`, or is deleted in
favour of it if every caller has a `&Heap` in scope. Prefer deleting it: two
spellings of one lookup is what this seam exists to avoid.

The unsafe self-referential bootstrap at 283-300 and the plain construction at
348 stay as they are for now. They *build* objects rather than read them, and
Task 13 is where construction moves behind the seam.

- [ ] **Step 5: Thread `&Heap` through the callers**

Every `dispatch::` free function that now reads a class takes `heap: &Heap` as
its first parameter. Where a caller holds `&mut Interpreter`, pass
`&interp.heap` — the immutable borrow ends when the function returns an owned
`Lookup`, so the following `self.invoke_method(...)` still type-checks. Where the
borrow checker objects, copy the value out first; that is the API's design, not
a workaround.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test heap`
Expected: PASS, 8 passed.

- [ ] **Step 7: Verify the migration is complete**

Run: `grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/dispatch.rs`
Expected: `0`.

- [ ] **Step 8: Run the full gates and commit**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime crates/ns-cli
git commit -m "refactor: route dispatch through the heap seam"
```

---

### Task 5: Migrate `interpreter/eval.rs`

The largest single file, 21 sites, and the one where the copy-out decision earns
itself: `eval_send` holds a receiver across argument evaluation, which
allocates.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/eval.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (add the `Frame` accessors below)

**Interfaces:**
- Consumes: Tasks 1 and 4's accessors.
- Produces: on `Heap` — `receiver(&self, frame: &FrameRef) -> Value`,
  `frame_current_class(&self, frame: &FrameRef) -> Option<ClassRef>`,
  `frame_arg_count(&self, frame: &FrameRef) -> usize`,
  `local(&self, frame: &FrameRef, index: u16) -> Value`,
  `set_local(&mut self, frame: &FrameRef, index: u16, value: Value)`,
  `frame_pc(&self, frame: &FrameRef) -> usize`,
  `set_frame_pc(&mut self, frame: &FrameRef, pc: usize)`,
  `frame_home(&self, frame: &FrameRef) -> Option<FrameRef>`,
  `frame_sender(&self, frame: &FrameRef) -> Option<FrameRef>`,
  `frame_is_uncontinuable(&self, frame: &FrameRef) -> bool`,
  `mark_uncontinuable(&mut self, frame: &FrameRef)`,
  `frame_slot_base(&self, frame: &FrameRef) -> usize`,
  `frame_captured(&self, frame: &FrameRef) -> Option<FrameRef>`.
  `eval::read_local` and `eval::write_local` keep their names and gain a
  `&Heap` / `&mut Heap` first parameter.

- [ ] **Step 1: Add the frame accessors to `heap.rs`**

```rust
    // ---------- Activation accessors ----------
    //
    // `local`/`set_local` carry the register-file addressing that
    // `eval::read_local` used to own: arguments occupy the first
    // `args.len()` indices and declared temporaries follow, and the
    // boundary comes from how many arguments the activation was *given*.
    // A wrong-arity activation is refused in `invoke_method` before it can
    // reach here, so this addressing is total.

    pub fn receiver(&self, frame: &FrameRef) -> Value {
        frame.borrow().receiver.clone()
    }

    pub fn frame_arg_count(&self, frame: &FrameRef) -> usize {
        frame.borrow().args.len()
    }

    pub fn local(&self, frame: &FrameRef, index: u16) -> Value {
        let borrowed = frame.borrow();
        let i = index as usize;
        if i < borrowed.args.len() {
            borrowed.args[i].clone()
        } else {
            borrowed.temps[i - borrowed.args.len()].clone()
        }
    }

    pub fn set_local(&mut self, frame: &FrameRef, index: u16, value: Value) {
        let mut borrowed = frame.borrow_mut();
        let i = index as usize;
        if i < borrowed.args.len() {
            borrowed.args[i] = value;
        } else {
            let temp_index = i - borrowed.args.len();
            borrowed.temps[temp_index] = value;
        }
    }

    pub fn frame_current_class(&self, frame: &FrameRef) -> Option<ClassRef> {
        frame.borrow().current_class.clone()
    }

    pub fn frame_pc(&self, frame: &FrameRef) -> usize {
        frame.borrow().pc
    }

    pub fn set_frame_pc(&mut self, frame: &FrameRef, pc: usize) {
        frame.borrow_mut().pc = pc;
    }

    pub fn frame_home(&self, frame: &FrameRef) -> Option<FrameRef> {
        frame.borrow().home.clone()
    }

    pub fn frame_sender(&self, frame: &FrameRef) -> Option<FrameRef> {
        frame.borrow().sender.clone()
    }

    pub fn frame_is_uncontinuable(&self, frame: &FrameRef) -> bool {
        frame.borrow().uncontinuable
    }

    pub fn mark_uncontinuable(&mut self, frame: &FrameRef) {
        frame.borrow_mut().uncontinuable = true;
    }

    pub fn frame_slot_base(&self, frame: &FrameRef) -> usize {
        frame.borrow().slot_base
    }

    /// The frame a closure activation captured, or `None` for any other
    /// kind. `eval::frame_at_depth` walks this.
    pub fn frame_captured(&self, frame: &FrameRef) -> Option<FrameRef> {
        match &frame.borrow().kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame.clone()),
            _ => None,
        }
    }
```

- [ ] **Step 2: Write the failing test**

Append to `crates/ns-runtime/tests/heap.rs`:

There is no shared test-support crate — every integration test carries its own
`load` and `expect_ok`. Copy that shape, exactly as
`crates/ns-runtime/tests/builtins_collections_block.rs:16-34` has it:

```rust
use ns_runtime::{Interpreter, ResolvedModule, Signal, Symbol, Value};

fn load(src: &str) -> (Interpreter, ns_runtime::ClassRef, ResolvedModule) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, resolved)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

#[test]
fn register_addressing_puts_arguments_before_temporaries() {
    // Exercised through a real activation rather than a hand-built frame:
    // the argument/temporary boundary is exactly what the accessor
    // encodes, and a synthetic frame could encode it wrongly and then
    // agree with itself.
    let src = "Newspeak3\n'T'\nclass T usingPlatform: p = Object new () (\n\
               \tpublic run: a = (| t | t := a. ^t)\n) : ()\n";
    let (mut interp, class, resolved) = load(src);
    let instance = expect_ok(
        interp.send(
            Value::Obj(class),
            Symbol::intern("usingPlatform:"),
            vec![interp.platform()],
            &resolved.bindings,
            None,
        ),
        "instantiate",
    );
    let result = expect_ok(
        interp.send(
            instance,
            Symbol::intern("run:"),
            vec![Value::SmallInt(41)],
            &resolved.bindings,
            None,
        ),
        "run:",
    );
    assert!(matches!(result, Value::SmallInt(41)));
}
```

Check the factory selector against a neighbouring test before running this: the
module header above declares `usingPlatform:`, and `tests/instantiate.rs` shows
how that send is made. If the shape differs there, follow that file — it is the
one the suite already proves.

- [ ] **Step 3: Run the test to verify it fails, then migrate**

Run: `cargo test -p ns-runtime --test heap`
Expected: FAIL.

Then rewrite `read_local` and `write_local` as delegations, so that the 21 call
sites change only by gaining a heap argument:

```rust
pub(crate) fn read_local(heap: &Heap, frame: &FrameRef, index: u16) -> Value {
    heap.local(frame, index)
}

pub(crate) fn write_local(heap: &mut Heap, frame: &FrameRef, index: u16, value: Value) {
    heap.set_local(frame, index, value);
}
```

and migrate the remaining sites in `eval.rs` — `frame.borrow().receiver.clone()`
becomes `heap.receiver(frame)`, `frame_at_depth`'s walk becomes
`heap.frame_captured(...)`, and `invoke_closure`'s reads of
`closure.captured_frame` become `heap.receiver` and `heap.frame_current_class`
on that frame.

- [ ] **Step 4: Run the tests, verify completeness, commit**

```bash
cargo test -p ns-runtime --test heap
grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/interpreter/eval.rs
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime
git commit -m "refactor: route the tree-walker's evaluation through the heap seam"
```

Expected: tests PASS; grep answers `0`; gates as in Task 1 Step 6.

---

### Task 6: Migrate `interpreter/mod.rs` and `interpreter/nested.rs`

15 sites. This is the shared orchestration, so it is the task where an
accidental behaviour change would show up as the two engines disagreeing rather
than as a failing unit test — the differential harness is the gate that matters
here.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (12 sites),
  `crates/ns-runtime/src/interpreter/nested.rs` (3 sites)

**Interfaces:**
- Consumes: Tasks 1, 4 and 5's accessors. No new API.

- [ ] **Step 1: Migrate the reads**

`Interpreter::send`'s class-object test at 253-255 becomes
`self.heap.is_class(obj)`; `metaclass_of`'s cache read and write become
`class_metaclass` and `set_class_metaclass`; `class_of`'s reads become
`class_of_obj`; `invoke_method`'s `frame.borrow_mut().uncontinuable = true`
becomes `self.heap.mark_uncontinuable(&frame)`; `raise_return`'s walk becomes
`frame_home` plus `frame_is_uncontinuable`.

`nested.rs`'s three sites read the hidden nested-class slot off an instance;
they become `heap.field` and `heap.set_field`. Add those two accessors to
`heap.rs` if Task 3 did not already need them:

```rust
    pub fn field(&self, obj: &ObjRef, index: usize) -> Option<Value> {
        match &obj.borrow().data {
            ObjectData::Instance(fields) => fields.get(index).cloned(),
            _ => None,
        }
    }

    /// Silently skipped for a non-instance receiver, matching what
    /// `instantiate::run_own_slots` and the VM's `InitSlot` already do: a
    /// direct field write on a receiver that has no fields is not an error
    /// at this level.
    pub fn set_field(&mut self, obj: &ObjRef, index: usize, value: Value) {
        if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
            if index < fields.len() {
                fields[index] = value;
            }
        }
    }
```

- [ ] **Step 2: Leave `Rc::ptr_eq` alone**

`interpreter/mod.rs:568` and `nested.rs:359` compare a return signal's target
against the current frame. They stay `Rc::ptr_eq` until Task 13, which replaces
every one of them at once with the newtype's own identity comparison. Splitting
that across two tasks would leave two spellings of identity in the tree.

- [ ] **Step 3: Verify and commit**

```bash
grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/interpreter/mod.rs crates/ns-runtime/src/interpreter/nested.rs
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime
git commit -m "refactor: route the shared orchestration through the heap seam"
```

Expected: grep answers `0` for both files; `cargo test --workspace` includes
`differential` at 0 failed; gates as before.

---

### Task 7: Migrate `interpreter/instantiate.rs`

8 sites, and the one file where `slot_base` addressing is load-bearing: an
`InitSlot`'s `own_index` is mixin-relative and only the executing frame's
`slot_base` turns it into an absolute field index.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`

**Interfaces:**
- Consumes: Tasks 1, 4, 5 and 6's accessors. No new API.

- [ ] **Step 1: Migrate, preserving the `slot_base` assertion**

`run_own_initializer`'s `debug_assert_eq!(f.slot_base, own_slot_base, ...)`
becomes `debug_assert_eq!(self.heap.frame_slot_base(frame), own_slot_base, ...)`
with its message unchanged — it states the invariant that a frame is built for
the level it runs, and losing it would make a wrong `slot_base` silent.

The `temps` resize in the same function needs a new accessor, because growing an
activation is exactly what the arena will have to reason about:

```rust
    /// Grows an activation's register file, filling with nil. Needed by
    /// the initializer path, where a level's compiled body may declare
    /// more registers than the frame was built with, and by
    /// edit-and-continue later (`docs/design/liveness-and-reflection.md`).
    pub fn grow_temps(&mut self, frame: &FrameRef, needed: usize) {
        let mut f = frame.borrow_mut();
        if f.temps.len() < needed {
            f.temps.resize(needed, Value::Nil);
        }
    }
```

- [ ] **Step 2: Verify and commit**

```bash
grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/interpreter/instantiate.rs
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime
git commit -m "refactor: route instantiation through the heap seam"
```

Expected: grep answers `0`; gates as before.

---

### Task 8: Migrate `vm.rs`

9 sites, all of them either register access or the `pc` store. The hot path, so
this is the task whose result the benchmark should be checked against.

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs`

**Interfaces:**
- Consumes: Tasks 1 and 5's accessors. No new API.

- [ ] **Step 1: Record the before-number**

```bash
cargo build --workspace --release
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns
```

Write the bytecode median down; the after-number in Step 3 must be within noise
of it. This plan changes no representation, so a real slowdown here means an
accessor is doing more work than the code it replaced.

- [ ] **Step 2: Migrate**

`read_local(frame, *src)` becomes `read_local(&interp.heap, frame, *src)` and
`write_local(frame, *dst, v)` becomes
`write_local(&mut interp.heap, frame, *dst, v)`. The three `frame.borrow_mut().pc = pc`
stores become `interp.heap.set_frame_pc(frame, pc)`, keeping them in exactly the
same three arms — every `Send*`, `MakeClosure` and `NonLocalReturn` — because
which instructions store `pc` is a documented decision, not an implementation
detail.

`Instr::InitSlot`'s two reads become `interp.heap.receiver(frame)` and
`interp.heap.frame_slot_base(frame)`.

`Instr::LoadSelf` becomes `interp.heap.receiver(frame)`.

Watch for the borrow conflict in the `Send*` arms: `collect(frame, arg_regs)`
now needs `&interp.heap` while `interp.send(...)` needs `&mut interp`. Collect
the arguments into an owned `Vec<Value>` *before* the send, which is what the
code already does — keep the two statements in that order.

- [ ] **Step 3: Verify no regression**

```bash
grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/vm.rs
cargo build --workspace --release
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns
```

Expected: grep answers `0`; bytecode median within noise of Step 1's figure.

- [ ] **Step 4: Run the full gates and commit**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime
git commit -m "refactor: route the VM loop through the heap seam"
```

---

### Task 9: Migrate `image/`

21 sites across three files. The encoder and decoder walk the object graph
directly, so this is where the accessor set gets its widest exercise — and where
an accessor that answers `None` instead of panicking on a wrong shape matters,
because a decoded graph is untrusted until verified.

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs` (9 sites),
  `crates/ns-runtime/src/image/decode.rs` (12 sites),
  `crates/ns-runtime/src/image/mixins.rs` (1 site)

**Interfaces:**
- Consumes: every accessor so far. No new API expected; if the encoder needs a
  shape the accessors cannot express, add the accessor rather than reaching past
  the seam.

- [ ] **Step 1: Migrate the encoder**

The encoder's graph walk is already iterative over an explicit stack, because an
image controls its own depth. Keep it iterative: replace each borrow with the
matching accessor and change nothing about the traversal.

- [ ] **Step 2: Migrate the decoder**

`decode.rs:1989` and `1997` *construct* objects and placeholder frames; leave
them, as Task 13 owns construction. The twelve reads become accessors.

- [ ] **Step 3: Leave the decoder's `Rc::ptr_eq` sites alone**

Lines 1891 and 2266, like Task 6's, are replaced wholesale in Task 13.

- [ ] **Step 4: Verify and commit**

```bash
grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/src/image/*.rs
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime
git commit -m "refactor: route image encoding and decoding through the heap seam"
```

Expected: grep answers `0` for `encode.rs`, `decode.rs` and `mixins.rs`; the
image round-trip and mutation tests in `crates/ns-runtime/tests/image.rs` all
pass.

---

### Task 10: Migrate the remaining files

11 sites in five files, none of them related to each other, which is why they
are one task rather than five: no reviewer could meaningfully accept one and
reject another.

**Files:**
- Modify: `crates/ns-runtime/src/symbol.rs` (2), `platform.rs` (2),
  `compiler/mod.rs` (2), `types.rs` (1), `crates/ns-cli/src/main.rs` (4)

**Interfaces:**
- Consumes: every accessor so far. `ns-cli` reaches the heap through
  `Interpreter::heap_ref()` / `heap_mut()`, since `Interpreter::heap` is
  `pub(crate)`.

- [ ] **Step 1: Migrate all five files**

`types.rs`'s single site is inside the module that owns the types, so it may
reach the data directly — but route it through `Heap` anyway, so that `types.rs`
does not become the one file with a second spelling.

- [ ] **Step 2: Verify the production sources are clean**

Run: `grep -rn "\.borrow()\|\.borrow_mut()" crates/*/src --include=*.rs | grep -v "src/heap.rs"`
Expected: no output. Every remaining borrow under `src/` lives in `heap.rs`.

The test files are **not** clean yet — they hold 83 further sites and are Tasks
11 and 12. Do not widen this grep to `crates/` here; it will fail, and the
failure would be correct.

- [ ] **Step 3: Run the full gates and commit**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates
git commit -m "refactor: route the remaining call sites through the heap seam"
```

---

### Task 11: Migrate the integration tests, except `image.rs`

The tests reach into objects and frames as directly as the runtime did — 33
sites across thirteen files, plus nine direct `Rc::new(RefCell::new(Object { … }))`
constructions. They have to go through the seam too, or Task 13 turns into a
compile-error hunt across the whole suite at the moment the suite is the only
thing telling us the migration was behaviour-neutral.

**Files:**
- Modify: `crates/ns-runtime/tests/` — `builtin_object_inheritance.rs` (1 site),
  `builtins_boolean_numbers.rs` (1), `builtins_collections_block.rs` (3 sites +
  a construction), `builtins_control_flow.rs` (1), `builtins_text.rs` (2),
  `compiler.rs` (3), `factory_send.rs` (1), `instantiate.rs` (4),
  `interpreter.rs` (3 sites + a construction), `mixin.rs` (3),
  `nested_classes.rs` (4), `vm.rs` (4), `vm_initializer.rs` (3 sites + a
  construction)

**Interfaces:**
- Consumes: every accessor so far, through `Interpreter::heap_ref()` and
  `heap_mut()` — the `heap` field is `pub(crate)`, so an integration test in
  `tests/` reaches it only through those two.
- Produces: `Heap::alloc_object` is not available until Task 13, so the nine
  direct constructions stay as they are in this task and move in that one.

- [ ] **Step 1: Migrate the reads**

Each site is the same shape as its production counterpart:
`obj.borrow().identity_hash` becomes `interp.heap_ref().identity_hash(&obj)`,
`&obj.borrow().data` matches become the typed accessor, and frame reads become
`heap_ref().receiver(&frame)` and friends.

- [ ] **Step 2: Fix the latent `Debug`-on-`Value` defect in the same files**

Ten panic arms across `builtin_object_inheritance.rs:160,185`,
`builtins_boolean_numbers.rs:90,255,280,349`,
`builtins_collections_block.rs:40`, `builtins_text.rs:59` and
`factory_send.rs:96,185` format a `Value` with `{v:?}`:

```rust
        Ok(v) => panic!("{what}: expected a NewspeakError, got Ok({v:?})"),
```

That violates the never-`{:?}`-a-`Value` rule and is not theoretical: these arms
fire exactly when a test fails, and a `Value::Obj` there would recurse through
`Object`'s self-referential bootstrap class until the stack gives out — turning
a readable assertion failure into a stack overflow with no message. Replace each
with a shape that names the value without formatting it:

```rust
        Ok(v) => panic!(
            "{what}: expected a NewspeakError, got Ok({})",
            value_shape(&v)
        ),
```

with one local helper per file, beside the existing `expect_ok`:

```rust
/// Names a `Value`'s shape without formatting it. `Debug` on a
/// `Value::Obj` recurses forever, because the bootstrap `Object` class is
/// its own `.class` -- so a panic arm that formats one replaces a readable
/// failure with a stack overflow.
fn value_shape(v: &Value) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Bool(_) => "a Boolean",
        Value::SmallInt(_) => "a SmallInteger",
        Value::Float(_) => "a Float",
        Value::Obj(_) => "an object",
    }
}
```

This is a defect fix riding along with the migration rather than scope creep:
these are the exact lines the migration touches, and leaving them would mean
re-opening the same files to fix a known crash.

- [ ] **Step 3: Verify and commit**

```bash
grep -rn "\.borrow()\|\.borrow_mut()" crates/ns-runtime/tests --include=*.rs | grep -v "tests/image.rs"
grep -rn "{v:?}" crates/*/tests --include=*.rs
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
git add crates/ns-runtime/tests
git commit -m "test: route the integration tests through the heap seam"
```

Expected: both greps produce no output; 0 failed; no warnings.

---

### Task 12: Migrate `tests/image.rs`

50 sites in one file — its own task because no reviewer could hold it and Task 11
in mind at once, and because these are the tests that would catch an image
regression, so they deserve their own gate.

**Files:**
- Modify: `crates/ns-runtime/tests/image.rs` (50 sites, one construction, and
  seven `Rc::ptr_eq` uses at 516, 548, 1181, 1185, 1219, 1223, 1230, 1243)

**Interfaces:**
- Consumes: every accessor so far. The `Rc::ptr_eq` sites stay until Task 13,
  which replaces all of them at once.

- [ ] **Step 1: Migrate the reads**

Work top to bottom. Where a test asserts on a decoded graph's shape, prefer the
accessor that answers `Option` and assert on that, so a decode regression that
produces the wrong shape fails with a clear `None` rather than a panic inside a
`match` arm.

- [ ] **Step 2: Verify and commit**

```bash
grep -c "\.borrow()\|\.borrow_mut()" crates/ns-runtime/tests/image.rs
cargo fmt --all
cargo test -p ns-runtime --test image
cargo test --workspace
cargo clippy --workspace --all-targets
git add crates/ns-runtime/tests/image.rs
git commit -m "test: route the image tests through the heap seam"
```

Expected: grep answers `0`; the image test file passes on its own and in the
workspace run; no warnings.

---

### Task 13: Make the handles opaque

The task the previous eleven exist to make survivable. Flipping the aliases to
newtypes turns any missed call site into a compile error, so this is where the
migration is actually proven rather than asserted.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (the three aliases at 18, 20 and 187;
  the three `Drop` implementations at 437 and 446 and `mixin.rs:234`; the
  `push_obj` / `push_frame` strong-count helpers at 324 and 330; the unsafe
  self-referential bootstrap at 488-520)
- Modify: `crates/ns-runtime/src/heap.rs` (allocation moves here)
- Modify: `crates/ns-runtime/src/dispatch.rs:283-300, 348`,
  `crates/ns-runtime/src/interpreter/mod.rs:777, 875, 1027, 1064-1090`,
  `crates/ns-runtime/src/image/decode.rs:1989, 1997`,
  `crates/ns-runtime/src/interpreter/eval.rs:676`,
  `crates/ns-runtime/src/interpreter/instantiate.rs:385`,
  `crates/ns-runtime/src/interpreter/mod.rs:535` — every construction site
- Modify: the nine test-side constructions in
  `crates/ns-runtime/tests/builtins_collections_block.rs`, `image.rs`,
  `interpreter.rs` and `vm_initializer.rs`
- Modify: all 20 `Rc::ptr_eq` sites, 10 under `crates/*/src` and 10 under
  `crates/ns-runtime/tests/` (seven of them in `image.rs`)

**Interfaces:**
- Produces: `ObjRef`, `ClassRef` and `FrameRef` as newtypes with a private
  field. `ObjRef::same(&self, other: &ObjRef) -> bool` and
  `FrameRef::same(&self, other: &FrameRef) -> bool` replace `Rc::ptr_eq`.
  `Heap::alloc_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef`
  and `Heap::alloc_frame(&mut self, frame: Frame) -> FrameRef` replace the bare
  constructions.

- [ ] **Step 1: Write the failing test**

Append to `crates/ns-runtime/tests/heap.rs`:

```rust
#[test]
fn identity_is_compared_through_the_handle_not_through_a_pointer() {
    let mut interp = Interpreter::bootstrap();
    let a = interp.new_string("x".to_string());
    let b = interp.new_string("x".to_string());
    assert!(a.same(&a));
    assert!(!a.same(&b));
}

#[test]
fn allocation_assigns_a_fresh_identity_hash_per_object() {
    let mut interp = Interpreter::bootstrap();
    let a = interp.new_string("x".to_string());
    let b = interp.new_string("x".to_string());
    assert_ne!(
        interp.heap_ref().identity_hash(&a),
        interp.heap_ref().identity_hash(&b)
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime --test heap`
Expected: FAIL to compile — `no method named same`.

- [ ] **Step 3: Flip the aliases**

In `types.rs`, replace the aliases with newtypes:

```rust
/// An opaque handle to an object. The inner `Rc` is private, and only
/// `types.rs` and `heap.rs` can reach it: that privacy is what makes
/// replacing the store behind it a change to those two files rather than
/// to every caller. See `docs/design/memory.md`, "The seam".
#[derive(Debug, Clone)]
pub struct ObjRef(pub(crate) std::rc::Rc<RefCell<Object>>);

impl ObjRef {
    /// Object identity. Replaces `Rc::ptr_eq` at every call site, so that
    /// identity has one spelling that survives the store changing under it.
    pub fn same(&self, other: &ObjRef) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

/// A class is an `ObjRef` whose `data` is `ObjectData::Class(_)`, which the
/// type system does not enforce.
pub type ClassRef = ObjRef;

#[derive(Debug, Clone)]
pub struct FrameRef(pub(crate) std::rc::Rc<RefCell<Frame>>);

impl FrameRef {
    pub fn same(&self, other: &FrameRef) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}
```

- [ ] **Step 4: Move allocation onto the heap**

```rust
    pub fn alloc_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        let identity_hash = self.alloc_identity_hash();
        ObjRef(std::rc::Rc::new(std::cell::RefCell::new(Object {
            class,
            identity_hash,
            data,
        })))
    }

    pub fn alloc_frame(&mut self, frame: Frame) -> FrameRef {
        FrameRef(std::rc::Rc::new(std::cell::RefCell::new(frame)))
    }
```

Every construction site listed under **Files** becomes a call to one of these.
The two unsafe self-referential bootstraps stay unsafe and stay where they are —
they build the `Object` class that is its own `.class`, which safe Rust cannot
express with `Rc` — but they wrap their result in `ObjRef` and move into
`heap.rs` beside `alloc_object`, so that every construction of a handle is in one
file. Keep their existing comments verbatim: they explain the strong-count
balancing, which is exactly the part a later reader cannot reconstruct.

- [ ] **Step 5: Fix the fallout**

`cargo build --workspace` now reports every site that reached past the seam. Fix
them by adding the accessor they need, never by widening the field's visibility.
The `Drop` implementations and the `push_obj` / `push_frame` helpers live in
`types.rs` and may use `self.0` directly.

Replace all 20 `Rc::ptr_eq` sites with `.same(...)`, including the ten in the
test files.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test heap`
Expected: PASS, 12 passed.

- [ ] **Step 7: Run the full gates**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: 0 failed; no warnings; `210/0/1/0`; `ran 6, failed 0, skipped 205`.

- [ ] **Step 8: Commit**

```bash
git add crates
git commit -m "refactor: make object and frame handles opaque"
```

---

### Task 14: Prove the seam cost nothing

**Files:**
- Modify: `docs/PROGRESS.md` (a short entry recording the seam)
- Modify: `docs/design/memory.md` (change "The seam" from a plan to a
  description, since it now exists)

**Interfaces:**
- Consumes: everything.

- [ ] **Step 1: Measure**

```bash
cargo build --workspace --release
cargo run -p ns-cli --release -- bench tests-fixtures/bench_loop.ns
```

Expected: bytecode median within noise of the ~19.6 ms recorded in
`docs/design/execution.md`. The seam adds a function call per access that
`--release` should inline away entirely; a measurable regression means an
accessor clones something the old code borrowed, and is a defect to fix rather
than a cost to record.

- [ ] **Step 2: Confirm the seam holds**

```bash
grep -rn "\.borrow()\|\.borrow_mut()" crates/ --include=*.rs | grep -v "src/heap.rs\|src/types.rs"
grep -rn "Rc::ptr_eq" crates/ --include=*.rs | grep -v "src/types.rs"
```

Expected: no output from either.

- [ ] **Step 3: Update the documents to the present tense**

In `docs/design/memory.md`, "The seam" is now built: change it from a `Planned:`
subsection into a description under a `## What exists` heading, per the
documentation convention that present tense means it exists.

In `docs/PROGRESS.md`, add an entry under the M4 heading recording what was done
and the benchmark result.

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "docs: record the heap seam as built"
```

---

## What this plan deliberately does not do

Named here so a reviewer does not read them as gaps:

- **No arena.** The store behind the handles is still `Rc<RefCell<_>>`. That is
  the next plan, and it changes `heap.rs` and nothing else.
- **No `GcScope`/`NoGcScope`.** The scope discipline arrives with the arena,
  because there is nothing to root until objects can move.
- **No activation stack.** Activations are still individually allocated. That is
  the third plan, and it depends on the arena.
- **No performance improvement.** This plan is measured on whether it costs
  nothing, not on whether it gains anything.
