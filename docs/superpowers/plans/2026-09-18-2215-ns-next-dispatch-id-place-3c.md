# The Dispatch Id's Place (3c) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the two variants of step 3c — the dispatch id in the object
header (C2), and the id in the header with `ClassData` boxed out of it (C2+)
— measure both against `7928289`, let the user decide on the numbers, and
leave the tree holding the winner (or neither) with every document saying so.

**Architecture:** `Heap::dispatch_id` is the only function that knows where
the id lives (`heap.rs`), so both variants change its representation and
nothing on the send path around it: the guards, the entries and the global
cache stay as they are. C2 adds a `dispatch_id` to `Object`, set at
allocation and restamped where a class's application changes; a guard
becomes one load and one compare. C2+ additionally boxes `ClassData`, the one
variant that sets `ObjectData`'s size, so `Object` shrinks below today's 56
bytes at the price of one indirection on every read of a class's data. The
two land as consecutive commits on one branch so that each has a binary of
its own to measure; the decision then keeps both, reverts the second, or
reverts both.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency.

**Spec:** `docs/design/dispatch.md`, "3c — The dispatch id in the object
header" (the authority for this plan), with "The dispatch id" in the same
document, `docs/design/memory.md` (the amplification table under "An image
is untrusted input" and the paragraph on the caches' numbers) and
`docs/design/execution.md`'s measurement protocol ("What removing the
re-entries moved", "What the inline caches moved", "What the step before 3c
moved").

## Global Constraints

- Work on a branch `feature/dispatch-id-in-header` created from `master`, on
  the checked-out tree — no `git worktree` for the work itself (throwaway
  worktrees for the measurement's other binaries are fine and are removed
  afterwards). This plan is the branch's first commit.
- Before Task 1, record `git rev-parse HEAD` of `master` as **START**
  (`4a3cee9` when the plan was written). The baseline 3c is measured against
  is **`7928289`**, the last commit that changed a send's path
  (`docs/PROGRESS.md`); START is measured beside it to show that the
  commits between them (the goldens, resolver refusals) moved nothing.
  `d804eaf` is carried as in every measurement since step 2.
- **The user decides on the numbers.** Task 3 ends by presenting them and
  stops; Task 4 does not begin until the user has chosen. The proposed rule
  (`dispatch.md`): the faster wins; within noise, the smaller; if neither
  beats `7928289`, the id stays on the class object and the question is
  recorded as closed by that measurement. It is a proposal the user may
  overrule, not a rule the executor applies.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or
  two characters, no truncated word, in code, tests and commit messages alike.
  Clippy's `min_ident_chars` refuses the short ones.
- No dispatch logic in `vm.rs` or `compiler/`. Neither variant touches either.
- **Outcomes are pinned, not re-derived.** No golden, no `tests-fixtures`
  outcome and no `run-corpus` row changes in this step; one that does is a
  defect of the step, never a golden to update.
- **Never `{:?}` a `Value`**, never `.unwrap()`/`.expect()` an
  `EvaluationResult` or `HostResult`; tests use the local `expect_ok` the
  neighbouring test files have.
- An image is untrusted input: no byte sequence may panic, in a debug build
  either. Task 1 changes the decoder; it adds no read from the bytes, and the
  id it writes is never taken from them.
- The id is not a handle. The tracer does not trace it, and a move carries it
  with the object it sits in; neither variant adds anything to `trace.rs`
  beyond naming the new field as untraced.
- Comments explain the decision, not the mechanics.
- Every commit passes `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, and `cargo test --workspace`. Before the last
  commit of Tasks 1, 2 and 4 also run, in Git Bash:
  `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` (about three minutes),
  `NS_NEXT_STRESS_CACHES=1 cargo test --workspace` (about one minute),
  `cargo test --workspace --release` (the size test only runs there),
  `cargo run -p ns-cli -- parse-corpus corpus/modules` (must report
  `parsed 210, failed 0, expected-fail 1, stale 0`),
  `cargo run -p ns-cli -- run-corpus corpus/modules`,
  `NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules` and
  `NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules`
  (all three must report `ran 6, failed 0, skipped 205`).
- Conventional commits (`test:`, `refactor:`, `perf:`, `docs:`, `revert:`).
  **Every commit message ends with two trailer lines**, the first naming the
  model that actually wrote the commit's code (a subagent names its own
  model, not the controller's):

  ```
  Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01WstD18a59kQZpd1NwMpR96
  ```

- Never override the git author or committer identity.
- Not in scope: the specification gaps (decided 2026-09-18 to follow 3c in
  one plan of their own); `become:`'s exchange of the id, which is recorded,
  not built (there is no `become:` yet); closure conversion; the collector.

## File Structure

| File | Responsibility in this plan |
|---|---|
| `crates/ns-runtime/src/types.rs` | `Object` gains `dispatch_id` (Task 1); `ObjectData::Class` holds a `Box<ClassData>` (Task 2) |
| `crates/ns-runtime/src/heap.rs` | the id is computed at allocation, read by `dispatch_id`, restamped by `set_class_application`, stamped for a decoded graph (Task 1); the size test (Tasks 1, 2, 4) |
| `crates/ns-runtime/src/trace.rs` | names the new field as untraced (Task 1) |
| `crates/ns-runtime/src/image/decode.rs` | a filled record carries `NONE` until one stamping pass after `rebuild_applications` (Task 1); boxes a decoded class record (Task 2) |
| `crates/ns-runtime/src/dispatch.rs`, `interpreter/mod.rs` | box the class data they build (Task 2) |
| `crates/ns-runtime/tests/heap.rs`, `tests/image.rs`, `tests/vm_initializer.rs` | the restamping and decoding tests (Task 1); box the class data tests build (Task 2) |
| `docs/design/dispatch.md`, `memory.md`, `execution.md`, `README.md`, `liveness-and-reflection.md`, `docs/PROGRESS.md`, `CLAUDE.md` | the record of what was measured and decided (Task 4) |

---

### Task 1: C2 — the dispatch id in the object header

**Files:**
- Modify: `crates/ns-runtime/src/types.rs:32-42` (`Object`), `:98-110`
  (`ClassData::dispatch_id`'s doc comment)
- Modify: `crates/ns-runtime/src/heap.rs` — `allocate_object` (`:665`),
  `allocate_self_referential_class` (`:686`), `replace_object`'s doc comment
  (`:1210`), `dispatch_id` (`:1388-1408`), `set_class_application`
  (`:1476-1492`), the size test (`:2956-2966`)
- Modify: `crates/ns-runtime/src/trace.rs:45-55`
- Modify: `crates/ns-runtime/src/image/decode.rs` — `rebuild_applications`
  (`:2080-2087`), `decode_graph_section` (`:2289-2297`)
- Test: `crates/ns-runtime/tests/heap.rs`, `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `DispatchId`, `DispatchId::NONE`, `DispatchId::class_side`
  (`mixin.rs`); `ClassData.dispatch_id`, which stays and still holds the
  instances id (`class_dispatch_ids`, the key of a `super` and a private
  lexical send, and allocation read it).
- Produces: `Object.dispatch_id: DispatchId` (`pub(crate)`);
  `Heap::header_dispatch_id(&self, class: ClassReference<'gc>, data: &ObjectData<'gc>) -> DispatchId`
  (private); `Heap::install_class_application(&mut self, class: ClassReference<'gc>, application: Rc<MixinApplication>) -> bool`
  (`pub(crate)`, no restamping); `Heap::stamp_dispatch_ids(&mut self, objects: &[ObjectReference<'gc>])`
  (`pub(crate)`). `set_class_application` keeps its signature.

**Why restamping has two shapes.** Today an instance's id is derived through
its class on every read, so a class that is given a new application
(`set_class_application`) carries its live instances along for free. A copy
in each header does not: an instance keeping the old id would go on hitting
the old application's cache entries — silent wrong behaviour at exit status
0. So the public `set_class_application` restamps every object whose class
is the one it changed, which is a walk of the arena: acceptable for its
callers, an edit to a live class (`install_mixin` in `tests/image.rs`,
`tests/instantiate.rs`, `ns-cli`'s `disasm_snapshots.rs`). The decoder cannot
use it: it sets one application per class record, so walking the arena each
time would be quadratic in a count the image controls. It installs each
application without restamping and stamps its decoded objects once,
afterwards.

- [ ] **Step 1: Write the restamping test**

Append to `crates/ns-runtime/tests/heap.rs` (it already has `load`,
`expect_ok` and the imports this needs):

```rust
/// A class given a new application -- an edit to a live class, the shape
/// `install_mixin` in `tests/image.rs` takes -- moves its live instances to
/// the new application's id with it. With the id copied into every object
/// header (`docs/design/dispatch.md`, "3c"), an instance keeping the old copy
/// would go on hitting the old application's cache entries.
#[test]
fn a_class_given_a_new_application_moves_its_live_instances_with_it() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Subject = Object new (
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    interpreter.enter(|mutator| {
        let Value::Object(class_object) = mutator.fetch(&class) else {
            panic!("expected a class object")
        };
        let before = mutator.heap().class_dispatch_ids(class_object);
        let application = mutator
            .heap()
            .class_application(class_object)
            .unwrap_or_else(|| panic!("expected a class object"));
        // A copy of the mixin, not the mixin itself: `apply_mixin` answers
        // its cached application for a (mixin, superclass application) pair
        // it has seen, and a copy is another key -- an edited class.
        let edited = std::rc::Rc::new((*application.mixin).clone());
        let reapplied = mutator.apply_mixin(&edited, application.superclass.as_ref());
        assert!(mutator
            .heap_mut()
            .set_class_application(class_object, reapplied));
        let after = mutator.heap().class_dispatch_ids(class_object);
        assert_ne!(before, after, "a new application mints new ids");
        let instance = mutator.fetch(&instance);
        assert_eq!(mutator.heap().dispatch_id(&instance), after.instances);
        assert_eq!(
            mutator.heap().dispatch_id(&Value::Object(class_object)),
            after.class_side
        );
    });
}
```

- [ ] **Step 2: Write the decoding test**

Append to `crates/ns-runtime/tests/image.rs`, next to
`a_snapshot_loads_into_a_bytecode_interpreter_with_its_state_and_a_safe_hash_counter`
(whose helpers — `load`, `class_reference`, `object`, `expect_ok`, `Origin`,
`PathBuf` — it uses):

```rust
/// A decoded object answers the id of the application its class was given
/// while loading, never one read from the bytes (ids are per heap and not
/// serialized), and never the placeholder's: the decoder fills a record
/// before the class it names has its application, so the id is stamped only
/// once every class has one (`docs/design/dispatch.md`, "3c").
#[test]
fn a_loaded_image_answers_the_dispatch_ids_its_classes_were_given() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Counter = Object new (
          | count ::= 0. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let platform = interpreter.platform();
    let bytes = interpreter.enter(|mutator| {
        let instance = mutator.fetch(&instance);
        let class = class_reference(mutator, &class);
        let platform = mutator.fetch(&platform);
        image::snapshot(
            mutator,
            Some(source),
            &Origin::File(PathBuf::from("t.ns")),
            class,
            &platform,
            &[instance],
        )
        .expect("snapshot")
    });

    let mut loaded = image::load(&bytes).expect("load");
    loaded.interpreter.enter(|mutator| {
        let root_class = class_reference(mutator, &loaded.root_class);
        let ids = mutator.heap().class_dispatch_ids(root_class);
        assert_ne!(ids.instances, DispatchId::NONE);
        let instance = mutator.fetch(&loaded.roots[0]);
        assert_eq!(mutator.heap().dispatch_id(&instance), ids.instances);
        assert_eq!(
            mutator.heap().dispatch_id(&Value::Object(root_class)),
            ids.class_side
        );
    });
}
```

Add `DispatchId` to the file's `use ns_runtime::{...}` list if it is not
there.

- [ ] **Step 3: Run both tests; they pass today**

Run: `cargo test -p ns-runtime --test heap a_class_given_a_new_application -- --nocapture`
and `cargo test -p ns-runtime --test image a_loaded_image_answers -- --nocapture`
Expected: both PASS. They pin behaviour the header must keep; today the id is
derived on every read, so nothing can be stale yet. Their job is to fail if
Step 5 forgets a restamp — Step 6 checks that they would.

- [ ] **Step 4: Commit the tests**

```bash
git add crates/ns-runtime/tests/heap.rs crates/ns-runtime/tests/image.rs
git commit -m "test: pin the dispatch ids of a reapplied class and a decoded graph"
```

- [ ] **Step 5: Put the id in the header**

`types.rs`, `Object` gains the field after `identity_hash`:

```rust
    /// The id an inline cache guards this object's sends on
    /// (`docs/design/dispatch.md`, "The dispatch id"): an instance's class's
    /// instances id, a class object's own class-side id. A copy, so that a
    /// guard is one load and one compare rather than a walk to the class --
    /// which makes every place a class's application changes responsible for
    /// restamping it (`Heap::set_class_application`,
    /// `Heap::stamp_dispatch_ids`). Not a handle; a move carries it along.
    pub(crate) dispatch_id: crate::mixin::DispatchId,
```

`ClassData::dispatch_id`'s doc comment: it is no longer read by a guard; it
is where allocation, `class_dispatch_ids` and restamping read the instances
id from, and it still derives the class side rather than storing it — rewrite
the comment to say that, and drop the sentence about the object size (the
size test's comment carries that now).

`heap.rs`, a private helper beside `dispatch_id`:

```rust
    /// The id `dispatch_id` answers for an object of `class` holding `data`:
    /// a class object's own class-side id, an instance's class's instances
    /// id, `NONE` when the class is not a class object -- only a crafted
    /// image makes an instance of one.
    fn header_dispatch_id(&self, class: ClassReference<'gc>, data: &ObjectData<'gc>) -> DispatchId {
        if let ObjectData::Class(own) = data {
            return own.dispatch_id.class_side();
        }
        self.check_object_origin(class);
        match &self.objects[class.index as usize].data {
            ObjectData::Class(class_data) => class_data.dispatch_id,
            _ => DispatchId::NONE,
        }
    }
```

`allocate_object` computes it before pushing:

```rust
        let dispatch_id = self.header_dispatch_id(class, &data);
        self.push_object(Object {
            class,
            identity_hash: 0,
            dispatch_id,
            data,
        })
```

`allocate_self_referential_class` cannot read its class — it is the object
being built — so it derives the id from `data` alone: its own class-side id
when `data` is a class, `NONE` otherwise (its own class is then not a class
object):

```rust
        let dispatch_id = match &data {
            ObjectData::Class(own) => own.dispatch_id.class_side(),
            _ => DispatchId::NONE,
        };
```

`dispatch_id` reads the header, and in a debug build checks the copy against
the derivation it replaces, so every debug test run — the stress modes and
the goldens under both lowerings included — catches a copy some path forgot
to restamp:

```rust
    /// The dispatch id an inline cache guards `value`'s send on
    /// (`docs/design/dispatch.md`, "The dispatch id"): an object's is in its
    /// header -- one arena load -- and an immediate's in a table. Nothing
    /// outside this function knows where the id lives.
    #[inline(always)]
    pub fn dispatch_id(&self, value: &Value<'gc>) -> DispatchId {
        let Value::Object(object) = value else {
            return self.immediate_dispatch_ids[immediate_index(value)];
        };
        self.check_object_origin(*object);
        let entry = &self.objects[object.index as usize];
        debug_assert_eq!(
            entry.dispatch_id,
            self.header_dispatch_id(entry.class, &entry.data),
            "an object's dispatch id went stale: something changed its class's \
             application without restamping it"
        );
        entry.dispatch_id
    }
```

`set_class_application` becomes the restamping wrapper around a new
`install_class_application`, which holds today's body plus the class
object's own header:

```rust
    /// Gives `class` a new application and moves every live object of that
    /// class to its ids -- a walk of the arena, which is why the decoder,
    /// which installs one application per class record, calls
    /// `install_class_application` instead and stamps once afterwards
    /// (`stamp_dispatch_ids`). [keep the existing paragraph about callers
    /// and the `false` answer]
    pub fn set_class_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        if !self.install_class_application(class, application) {
            return false;
        }
        for index in 0..self.objects.len() {
            let entry = &self.objects[index];
            if entry.class.index == class.index {
                let dispatch_id = self.header_dispatch_id(entry.class, &entry.data);
                self.objects[index].dispatch_id = dispatch_id;
            }
        }
        true
    }

    /// `set_class_application` without the walk: `class`'s own header is
    /// restamped, its instances' are not.
    pub(crate) fn install_class_application(
        &mut self,
        class: ClassReference<'gc>,
        application: std::rc::Rc<crate::mixin::MixinApplication>,
    ) -> bool {
        self.check_object_origin(class);
        let entry = &mut self.objects[class.index as usize];
        if let ObjectData::Class(data) = &mut entry.data {
            data.dispatch_id = application.dispatch_ids.instances;
            data.application = application;
            entry.dispatch_id = data.dispatch_id.class_side();
            return true;
        }
        false
    }

    /// Stamps each of `objects` with the id its class now answers. The
    /// decoder's one pass, after `rebuild_applications` has given every
    /// decoded class its application.
    pub(crate) fn stamp_dispatch_ids(&mut self, objects: &[ObjectReference<'gc>]) {
        for object in objects {
            self.check_object_origin(*object);
            let entry = &self.objects[object.index as usize];
            let dispatch_id = self.header_dispatch_id(entry.class, &entry.data);
            self.objects[object.index as usize].dispatch_id = dispatch_id;
        }
    }
```

If the borrow checker refuses the walk's read-then-write, read `entry.class`
into a local first; the shape — compute with `&self`, then write — stays.

`trace.rs`, `impl Trace for Object`: add `dispatch_id: _,` to the
destructuring, beside `identity_hash: _`.

`decode.rs`:
- `rebuild_applications` calls `install_class_application` in place of
  `set_class_application` (keep the `unreachable!`).
- `decode_graph_section` fills each record with `dispatch_id:
  DispatchId::NONE` — the record's class may not have its application yet,
  and `NONE` matches no cache entry — and calls
  `context.mutator.heap.stamp_dispatch_ids(&objects);` immediately after
  `rebuild_applications(...)?`, before the frames are decoded. A comment
  there says why the stamp waits: a record can name a class whose record
  comes later in the table.
- `replace_object`'s doc comment gains one sentence: the object it installs
  answers `NONE` until `stamp_dispatch_ids` runs.

The size test, `heap.rs`: the header grows `Object` from 56 to 64 bytes in a
release build. Rename it to `an_object_is_the_size_the_amplification_figure_assumes`,
assert 64, and rewrite its comment: the figure it guards is `CLAUDE.md`'s and
`memory.md`'s amplification, which Task 4 settles for whichever variant is
chosen. Do not touch `CLAUDE.md` or `memory.md` here — Task 4 does, once the
choice is made.

- [ ] **Step 6: Check that the tests guard what they should**

Temporarily delete the walk from `set_class_application` (leave
`install_class_application`'s call). Run:
`cargo test -p ns-runtime --test heap a_class_given_a_new_application`
Expected: FAIL (the `debug_assert_eq!` in `dispatch_id` fires, or the
assertion on the instance's id). Restore the walk.

Temporarily delete the `stamp_dispatch_ids` call from `decode_graph_section`.
Run: `cargo test -p ns-runtime --test image a_loaded_image_answers`
Expected: FAIL. Restore the call.

- [ ] **Step 7: Run everything**

Run: `cargo test --workspace`, then every command in the Global Constraints'
list, including `cargo test --workspace --release` for the size test.
Expected: all PASS, the corpus reports unchanged.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add crates
git commit -m "perf: the dispatch id in the object header (3c, C2)"
```

Record this commit's hash as **C2** in the task report; Task 3 builds it.

---

### Task 2: C2+ — `ClassData` boxed out of `ObjectData`

**Files:**
- Modify: `crates/ns-runtime/src/types.rs:45-52` (`ObjectData::Class`)
- Modify: every construction of `ObjectData::Class(ClassData::new(...))`:
  `src/dispatch.rs:496`, `src/interpreter/mod.rs:1296`, `:1477`, `:1510`,
  `src/types.rs:483`, `src/heap.rs:2391` (a test), `tests/image.rs:4400`,
  `:5886`, `tests/vm_initializer.rs:204`; and `decode_class_record`'s
  result in `src/image/decode.rs` (`:2275`)
- Modify: `crates/ns-runtime/src/heap.rs`, the size test

**Interfaces:**
- Consumes: Task 1 (the header id is what lets the box cost a guard nothing:
  no guard reads `ClassData` any more).
- Produces: `ObjectData::Class(Box<ClassData<'gc>>)`. Every
  `if let ObjectData::Class(data) = &...` keeps compiling, since field access
  derefs through the box; only constructions and by-value moves change.

- [ ] **Step 1: Box the variant**

`types.rs`:

```rust
    /// Boxed: a class object's data is what would otherwise set every
    /// object's size, and class objects are few (`docs/design/dispatch.md`,
    /// "3c", C2+).
    Class(Box<ClassData<'gc>>),
```

Wrap every construction in `Box::new(...)`; the compiler lists each site.
Where a site destructures `ClassData` by value out of the variant, deref the
box (`*data`) rather than cloning it.

- [ ] **Step 2: Pin the size**

Run `cargo test -p ns-runtime --release an_object_is_the_size -- --nocapture`
with the assertion temporarily printing `std::mem::size_of::<Object>()`, and
set the assertion to the printed number. The design expects 48 bytes
(`dispatch.md`, "3c"); if it is not 48, stop and report the number — every
amplification figure Task 4 writes derives from it. Update the test's
comment to name C2+.

- [ ] **Step 3: Run everything**

Run: `cargo test --workspace`, then every command in the Global Constraints'
list, including `cargo test --workspace --release`.
Expected: all PASS, the corpus reports unchanged.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add crates
git commit -m "perf: box ClassData out of ObjectData (3c, C2+)"
```

Record this commit's hash as **C2+** in the task report.

---

### Task 3: Measure both variants, and present the numbers

**Files:**
- Create: nothing in the tree. Raw output goes to the session's scratchpad.

**Interfaces:**
- Consumes: C2 and C2+ (Tasks 1, 2), START, `7928289`, `d804eaf`.
- Produces: the numbers Task 4 records and the user decides on.

- [ ] **Step 1: Write out the four sources that are not in the tree**

`bench_do.ns` from the
[2c plan](2026-09-13-2348-ns-next-lifetime-brand-2c.md), "Measurement
protocol"; `bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` from
`docs/design/execution.md`, "What spec-conformant lookup moved" — **copied
from the text the documents give, never reconstructed from their prose**
(`docs/PROGRESS.md` says why). The other four are
`goldens/bench/bench_loop.ns`, `bench_send.ns`, `bench_block.ns` and
`bench_megamorphic.ns`. Put the four written-out files in the scratchpad.

- [ ] **Step 2: Build the five binaries**

In throwaway worktrees for `7928289`, START and `d804eaf`, and at the C2 and
C2+ commits (a worktree for C2, the branch head for C2+), each built the
same way, in PowerShell:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

Copy each `target/release/ns-next.exe` out under a name that says which it
is. Check that none imports `VCRUNTIME140.dll`, and that every binary prints
the expected answer for every source before timing anything.

- [ ] **Step 3: Check that the step is still inline**

Both variants change what every cached guard reads, on the path `vm::run`'s
step inlines. On a release build with debug information of `7928289`, C2
and C2+, run `dumpbin /disasm` (Visual Studio Build Tools, not on `PATH`)
and confirm that no `vm::run::closure` is a function of its own; record
`vm::run`'s instruction count in each (`docs/design/execution.md`, "What the
inline caches moved"). If one variant split the step, report that before
timing it — its numbers would measure the split, not the id's place.

- [ ] **Step 4: Time**

`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the five binaries rotating from round to round; two whole sittings,
nothing else running. Record every number, not a summary.

- [ ] **Step 5: Measure memory and sizes**

- The release and debug `size_of::<Object>()` for `7928289` (56 release),
  C2 and C2+ — the debug one from a throwaway test printing it, not
  committed.
- The amplification at the graph section each implies, as `memory.md`'s
  table computes it: `size / 6 + 136 / 18` in a release build,
  `size / 6 + 160 / 18` in a debug one (check the frame-arena entry sizes
  have not changed: 136 and 160).
- Peak working set of `ns-next run-corpus corpus/modules` and of
  `ns-next bench goldens/bench/bench_block.ns --iterations 30` for each of
  `7928289`, C2 and C2+, in PowerShell:

  ```powershell
  $process = Start-Process -FilePath .\ns-next-c2.exe -ArgumentList "run-corpus","corpus/modules" -NoNewWindow -PassThru -Wait
  $process.PeakWorkingSet64 / 1MB
  ```

  (`PeakWorkingSet64` must be read before the object is disposed; if it
  answers 0 after exit, poll it in a loop while the process runs.)

- [ ] **Step 6: Present the numbers and stop**

Report to the user, in tables: each source's means per sitting for all five
binaries, the change of C2 and of C2+ against `7928289` with the protocol's
gate (overlapping, entirely above, entirely below), START against `7928289`,
both against `d804eaf`, `vm::run`'s instruction counts, the three object
sizes, the amplifications and the peak working sets. Say what the proposed
rule would choose and why. **Then wait for the user's decision.** Do not
begin Task 4 without it.

---

### Task 4: Apply the decision, and record what was measured

**Files:**
- Modify (by outcome): `crates/ns-runtime/src/types.rs`, `heap.rs`,
  `trace.rs`, `image/decode.rs` and the files Task 2 touched
- Modify: `docs/design/dispatch.md` ("The dispatch id", "3c", "Decisions"),
  `docs/design/memory.md` (the caches' paragraph, the amplification table),
  `docs/design/execution.md` (a section "What 3c moved"),
  `docs/design/README.md` (dispatch's status line),
  `docs/design/liveness-and-reflection.md` (only if a header id stays),
  `docs/PROGRESS.md`, `CLAUDE.md` (only if `Object`'s size changed)

**Interfaces:**
- Consumes: the user's decision; Task 3's numbers.
- Produces: a tree holding the chosen variant, and documents a fresh session
  starts from.

- [ ] **Step 1: Make the tree hold the decision**

- **C2+ chosen:** nothing to revert.
- **C2 chosen:** `git revert --no-edit <C2+>`, then amend nothing — a
  `revert:` commit whose body says the measurement chose C2 and names the
  section of `execution.md` holding the numbers.
- **Neither:** `git revert --no-edit <C2+>` and then `git revert --no-edit
  <C2>`, each with that body. The two tests of Task 1 stay: they pin
  behaviour any representation must keep. The size test returns to 56 with
  its original name and comment through the revert.

Then run every command in the Global Constraints' list.

- [ ] **Step 2: Write the documents**

- `dispatch.md`: "3c" changes from **Planned** to what was decided, with the
  numbers (or a pointer to `execution.md`'s section) and the rule the user
  applied; "The dispatch id" describes where the id now lives and what a
  guard costs; the decision list's 3c item records the outcome.
- `memory.md`: the caches' paragraph states where the id lives and that it
  is not a handle; the amplification table's `Object` row, release and
  debug, and the combined figures, from Task 3's sizes. Unchanged if neither
  variant was chosen, except for one sentence recording that the question
  was closed by measurement.
- `execution.md`: "What 3c moved", in the form of "What the step before 3c
  moved": protocol, every number, the gate, the instruction counts, the
  sizes and the working sets.
- `liveness-and-reflection.md`: if a header id stays, that `become:` must
  exchange it along with the class.
- `README.md`: dispatch's status — 3 is complete.
- `PROGRESS.md`: a section "M4 — The dispatch id's place" in the
  chronology, and "Where the next session starts" moves on to the
  specification gaps — one plan, the member-set fix (a real class identity
  in place of a name) with the two cheap fixes riding along, and a fresh
  baseline for their measurement (`PROGRESS.md`, "The order after 3c").
- `CLAUDE.md`: if `Object`'s release size changed, the "An image is
  untrusted input" paragraph's figures — the size, the release and debug
  amplifications — and the size test's name.

- [ ] **Step 3: Check the documents against a fresh session**

Read `CLAUDE.md`, `docs/design/README.md` and `PROGRESS.md`'s "Where the
next session starts" as a session that knows nothing of this one would:
does each name the next piece of work, where its plan goes, and what it is
measured against? Fix what does not.

- [ ] **Step 4: Commit**

```bash
git add docs CLAUDE.md crates
git commit -m "docs: record 3c as decided, with its numbers"
```

---

## Self-Review

**Spec coverage.** `dispatch.md`'s "3c" asks for: C2 (Task 1) — the id set
at allocation, an instance's from its class, a class object's from its own
class side, a decoder placeholder that matches no entry and is set when the
record is filled, the id never from the bytes, the tracer unaffected; C2+
(Task 2); both measured against `7928289` in two sittings with `d804eaf`,
with the object sizes, the amplification and the memory after `run-corpus`
(Task 3); the user deciding (Task 3, Step 6; Task 4). `become:`'s exchange
is recorded, not built (Task 4). One refinement of the design's wording: a
decoded record answers `NONE` from the moment it is filled until the one
stamping pass after `rebuild_applications`, not "set when the record is
filled" — its class may not have an application yet at that point. Task 4
writes that into `dispatch.md`. The design did not mention that a class's
application changes after its instances exist (`set_class_application`);
Task 1 restamps there, and says why the decoder must not use that path.

**Placeholders.** Task 1's `set_class_application` doc comment says to keep
an existing paragraph rather than repeating it here; the paragraph is in the
file the implementer edits. Task 2's size is to be read off the build, since
the plan cannot know it for certain; the design's 48 is the expectation and a
different number stops the task.

**Type consistency.** `header_dispatch_id`, `install_class_application`
and `stamp_dispatch_ids` are introduced in Task 1 and used only there and by
the decoder. `Object.dispatch_id` is `pub(crate)`: nothing outside the crate
constructs an `Object`, and no host should be able to forge a guard.
`ObjectData::Class(Box<ClassData>)` is Task 2's only type change.

## Execution Handoff

Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task, review
   between tasks, fast iteration.
2. **Inline Execution** — the tasks run in this session with checkpoints.
