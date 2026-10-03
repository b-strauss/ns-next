# The Object Representation Implementation Plan (Road Step 3a)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every object in today's arena one block of words in road step 3's final layout -- a header word, a class word, then its fields -- with class objects, closures and reified frames as such objects, the frame arena gone, `Symbol` under `String`, and the image's frames object records. This is road step 3's first plan; 3b moves the blocks into `ns-memory`'s space.

**Architecture:** Thirteen stages, each landing green. The ids come first (Tasks 1-3): the reserved guard values at the low end, per-heap tables that give mixins, applications and compiled code an index, and the applied clause's packing with its limit. Then the seam closes (Task 4): nothing outside the heap module names `ObjectData`, `Object`, `ClassData`, `ClosureData` or `Frame`. `Symbol` moves under `String` (Task 5) while the payloads are still enum variants. Then the representation changes behind the closed seam: the header word (Task 6), the data-only kinds as words (Task 7), strings and symbols as byte- and 32-bit-indexed words (Task 8), frames as objects of the frame format (Task 9), closures (Task 10) and class objects (Task 11) as words, and finally the slot collapses into one `Box<[u64]>` traced by format (Task 12). The image's wire format follows (Task 13), then the measurement (Task 14), the documentation (Task 15) and the end-of-batch gates (Task 16).

**Tech Stack:** Rust 1.98.0, edition 2021, `#![forbid(unsafe_code)]` in `ns-runtime`. No new dependency. Tests under `cargo-nextest`; goldens under `goldens/`.

**Spec:** `docs/design/object-model.md`, "Planned: the object layout" (its 3a/3b split, the header table, "The reserved guard values sit at the low end", "An application has two ids", the class-object, refusal and applied-clause paragraphs, the frame paragraph, "Strings have two widths" with "Planned with road step 3a", "Handles", and "What a slot access checks" with its last paragraph, which defers the recorded guard to road step 5), and its "Decisions" entry dated 2026-10-03; `docs/design/memory.md`, "Planned: the object space" (3a's part: the frame arena goes, trace and compaction by format over the one arena); `docs/design/image-format.md`, "Planned: the image under the new representation"; `docs/design/dispatch.md`, "Planned: the guard in the header word"; `docs/design/core-library.md`, "Planned, with road step 3a". All decided with the user on 2026-09-29 and 2026-10-03. Background: `docs/design/memory.md`, "The seam", "The brand", "The move stress mode", "The interim cost, and the one question still open" (the amplification figures); `docs/design/execution.md`, "Activations", "Converted representation", "The Are We Fast Yet benchmarks" (the measurement protocol). Prior art: Spur's formats (`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c`, `formatOf:`, the puns at `:2815-2817`), Primordial Soup's reserved class ids (`prior-art/primordialsoup/vm/object.h:64-68`) and its `kNegativeBit` (`:48`).

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - `vm.rs` holds no dispatch logic and raises no error of its own.
  - Compiling a resolved module never raises a `NewspeakError`. The clause limit of Task 3 is a *resolve* error, raised before compilation.
  - An image is untrusted input: no byte sequence may panic, every id, tag, count and length is validated before it indexes, allocates or slices, and every pre-allocation is bounded by `bounded_count`.
  - A wire tag is assigned once and never renumbered or reused. This plan adds exactly one: `OBJECT_FRAME = 10` (Task 13), and retires `OBJECT_ACTIVATION` (9).
  - A handle means nothing against a `Heap` that did not mint it; the brand needs no cast; `Heap::load`/`Heap::store` stay `pub(crate)`.
  - A running activation is an `ActivationReference`; only a `FrameReference` is ever stored. `bytecode::outer_reach_of` and `compact::narrow_outer_reach` are not touched.
  - Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvaluationResult`. Tests use a local `expect_ok` helper.
  - The VM mirror's six locks stay as they are.
  - A cache records what the lookup decided: `ClassData.builtin`, once a class-object field, is still read afresh in `act` on every send.
- **Object, frame and activation access goes through `crates/ns-runtime/src/heap.rs`** and its submodules. From Task 4 on, no file outside `crates/ns-runtime/src/heap.rs` and `crates/ns-runtime/src/heap/` names `ObjectData`, `Object`, `ClassData`, `ClosureData`, `Frame` or `FrameSlot`.
- **The header word's layout** is exactly `object-model.md`'s table: bits 0-21 the guard, 22-26 the format, 27-30 the age, 31 the remembered bit, 32 a `LargeInteger`'s negative bit, 33 the mark bit, 34-55 the identity hash, 56-63 the word count after the header (255 = an overflow word, used from 3b on). Age, remembered and mark stay 0 in 3a.
- **Reserved guards:** `DispatchId::NONE = 0`, `VACANT = 1`, `FORWARDED = 2`; the counter mints from 3 and below `2^22`; an exhausted counter answers `NONE`.
- **The identity hash is 22 bits wide**, drawn from a counter that wraps within them and skips 0; the decoder refuses a hash of `2^22` or more.
- **The applied clause** is one `SmallInteger`: the mixin id in bits 0-31, the clause index in bits 32-54, the named builtin's tag plus one in bits 55-60 (0 = none), the 61-bit pattern sign-extended so it lies in `SMALL_INTEGER_MIN ..= SMALL_INTEGER_MAX`. A superclass clause of more than `2^23` `<:` clauses is a resolve error; an image clause index of `2^23` or more a `DecodeError`.
- **Reified frames and the stack:** the activation stack (`StackEntry`, `ActivationHeader`, `RegisterBase`) is unchanged. No recorded guard is added anywhere (road step 5).
- **The ids are minted and never reclaimed; the application cache stays strong** (decided with the user on 2026-10-03).
- **A golden that changes is a behaviour change**, named in its task with the reason it is right. Only Task 5 changes or adds golden output.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's `min_ident_chars` refuses one- and two-character names. Comments explain the decision, not the mechanics. Public members of `Kernel.ns` are documented.
- A Newspeak module inline in a test is an `indoc! {"…"}` or `formatdoc!`, indented with the Rust around it.
- Conventional commits. Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_014iiGvJnqAeLJtzhwKA6Poa`.
  A subagent's commit names the subagent's own model.
- Work happens on the branch `feature/object-representation` in the main working directory (no worktree), created from `master` before Task 1.
- Per-commit gate (`CLAUDE.md`, "Working in this repository"), every task, however small:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules` → "parsed 211, failed 0"

  Tasks 6, 9, 12 and 13 also run `cargo nextest run --workspace --release` (the size tests run only there). The stress modes, `run-corpus`, the benchmarks profile and the `ns-lsp` suite run once, in Task 16 (the `finishing-a-batch` skill). A commit that never saw them is work in progress; a subagent implementing a task is told so. Nothing under `crates/ns-syntax` or `editors/jetbrains` changes.
- Run every command in the foreground, except Task 14's measurement, which the controller session runs itself.

## Review Focus

- **A married frame across a move and a collection.** A frame object reified for a running activation, then moved (`NS_NEXT_STRESS_MOVES`) or compacted, still reads and writes the live registers through `frame_local`/`frame_set_local`, and its divorce on return copies them into the moved object exactly when `retains_registers` was set. Pinned in Task 9 (heap unit tests across `move_bare_heap` and `collect_for_test`).
- **The identity hash at the edge of 22 bits.** The counter hands out `2^22 - 1`, then 1 (never 0); an image hash of `2^22 - 1` loads and the next fresh hash is 1 after the wrap; `2^22` is refused, in an object record and in the tag-named table. Pinned in Task 6.
- **The packed applied clause at its edges.** Clause index `2^23 - 1` with mixin id `u32::MAX` and the largest builtin tag packs into a negative `SmallInteger` and unpacks to the same triple; the resolver refuses `2^23 + 1` clauses and accepts `2^23`; the decoder refuses index `2^23`. Pinned in Task 3.
- **A Symbol used as a String.** `#abc size`, `#abc at: 1`, `#abc copyFrom: 1 to: 2`, `'a' , #b` and `#a , 'b'` answer as for the String `'abc'`/`'ab'` (a String, not a Symbol, from `,` and `copyFrom:to:`); `#a = 'a'` and `'a' = #a` stay `false`; `#a hash` is unchanged; `Symbol new` is refused while a program's `MySymbol new` still answers an ordinary instance. Pinned in Task 5 (goldens) and Task 8 (the same goldens on the new representation).
- **A crafted image's frames.** A frame record whose register count differs from its body's, a closure-kind operand that is not a closure, a record under the retired tag 9, a frame record under a header other than `Activation`'s, and a count that would allocate past the bytes remaining are each a `DecodeError`, never a panic. Pinned in Task 13.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-runtime/src/mixin.rs` | `DispatchId`'s reserved values (Task 1); `MixinApplication.index`, `AppliedClause` packing (Tasks 2, 3) |
| `crates/ns-runtime/src/heap/tables.rs` (new, Task 2) | `MixinId`, `ApplicationIndex`, `CodeId`, `CodeEntry`, `Tables` |
| `crates/ns-runtime/src/heap/header.rs` (new, Task 6) | `Header`, `Format`, the bit layout, the hash counter's wrap |
| `crates/ns-runtime/src/heap/object.rs` (new, Task 4) | `Object`, `ObjectData`, `ClassData`, `ClosureData`, `Frame`, `FrameSlot`, moved out of `types.rs`/`heap.rs`, private to the heap module; from Task 12 the slot is `Box<[u64]>` and these types are gone |
| `crates/ns-runtime/src/heap/records.rs` (new, Task 4) | the seam's public value types: `ObjectShape`, `NewClass`, `NewClosure`, `Record`, `FrameRecord` |
| `crates/ns-runtime/src/heap.rs` | the accessors; the frame arena's removal (Task 9); trace by format and `forward_everything` (Task 12) |
| `crates/ns-runtime/src/trace.rs`, `heap/mark.rs` | `Tracer::frame` maps to objects (Task 9); one mark bit set (Task 9); object tracing by format (Task 12) |
| `crates/ns-runtime/src/types.rs` | loses the object-data types (Task 4); `ValueClassRefusal` holds the offender (Task 11) |
| `crates/ns-runtime/src/interpreter/*.rs`, `builtins/*.rs`, `vm.rs`, `dispatch.rs` | through the seam (Task 4); `apply_mixin`'s id keys (Task 2); `decide_factory` (Task 5) |
| `crates/ns-runtime/src/resolver/mod.rs`, `resolver/classes.rs` | the clause-count resolve error (Task 3) |
| `crates/ns-runtime/core-library/Kernel.ns` | `Symbol = String new`, `Symbol>>=`/`hash` (Task 5) |
| `crates/ns-runtime/src/image/*.rs` | through the seam (Task 4); the 22-bit hash (Task 6); the clause limit (Task 3); `OBJECT_FRAME`, the two-pass decoder, `inspect` (Task 13) |
| `crates/ns-runtime/tests/*.rs`, `crates/ns-cli/**` | through the seam (Task 4); new tests per task |
| `goldens/library/string/*`, `goldens/library/builtin-subclass/*` | Task 5 |
| `docs/**`, `CLAUDE.md` | Task 15 |

**The interfaces every later task relies on** (exact signatures):

```rust
// mixin.rs (Task 1)
impl DispatchId {
    pub const NONE: DispatchId;                 // 0
    pub(crate) const VACANT: DispatchId;        // 1
    pub(crate) const FORWARDED: DispatchId;     // 2
    pub(crate) const FIRST_MINTED: u32;         // 3
    pub(crate) const LIMIT: u32;                // 1 << 22, exclusive
    pub(crate) fn from_counter(value: u32) -> DispatchId;
    pub(crate) const fn bits(self) -> u32;
}

// heap/tables.rs (Task 2), re-exported from heap.rs and lib.rs
pub struct MixinId(u32);          // Copy, Eq, Hash, Debug
pub struct ApplicationIndex(u32); // Copy, Eq, Hash, Debug
pub struct CodeId(u32);           // Copy, Eq, Hash, Debug
impl MixinId { pub fn index(self) -> usize; pub(crate) fn from_index(index: usize) -> Option<MixinId>; }
// ApplicationIndex and CodeId: the same two methods.
impl<'gc> Heap<'gc> {
    pub(crate) fn mixin_id(&mut self, mixin: &Rc<Mixin>) -> MixinId;          // registers on first sight
    pub fn mixin(&self, id: MixinId) -> Option<Rc<Mixin>>;
    pub(crate) fn register_application(&mut self, application: MixinApplication) -> Rc<MixinApplication>;
    pub fn application(&self, index: ApplicationIndex) -> Option<Rc<MixinApplication>>;
    pub(crate) fn code_id(&mut self, code: &Rc<CompiledCode>) -> CodeId;     // registers on first sight
    pub(crate) fn code_id_of_method(&mut self, method: &Rc<MixinMethod>) -> CodeId;
    pub fn code(&self, id: CodeId) -> Option<Rc<CompiledCode>>;
    pub fn code_method(&self, id: CodeId) -> Option<Rc<MixinMethod>>;
}
impl MixinApplication { pub fn index(&self) -> ApplicationIndex; }

// mixin.rs (Task 3)
pub(crate) const CLAUSE_INDEX_LIMIT: usize = 1 << 23;
impl AppliedClause {
    pub(crate) fn pack(mixin: MixinId, index: usize, named_builtin: Option<BuiltinClass>) -> i64;
    pub(crate) fn unpack(packed: i64) -> Option<(MixinId, usize, Option<BuiltinClass>)>;
}

// heap/records.rs (Task 4), re-exported from heap.rs and lib.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectShape { Instance, String, Symbol, Array, Closure, Class, Activation, LargeInteger, Float }
pub struct NewClass<'gc> {
    pub application: Rc<MixinApplication>,
    pub enclosing: Option<ObjectReference<'gc>>,
    pub declaring_class: Option<ClassReference<'gc>>,
    pub superclass: Option<ClassReference<'gc>>,
    pub builtin: Option<BuiltinClass>,
}
pub struct NewClosure<'gc> {
    pub code: Rc<CompiledCode>,
    pub captured_frame: FrameReference<'gc>,
    pub copied: Vec<Value<'gc>>,
    pub feedback_base: FeedbackBase,
}
pub enum Record<'gc> {
    Instance(Vec<Value<'gc>>),
    String(CodePoints),
    Symbol(Symbol),
    Array(Vec<Value<'gc>>),
    Closure(NewClosure<'gc>),
    Class(NewClass<'gc>),
    Activation(FrameReference<'gc>),
}
pub struct FrameRecord<'gc> {
    pub receiver: Value<'gc>,
    pub arguments: Vec<Value<'gc>>,
    pub temporaries: Vec<Value<'gc>>,
    pub program_counter: usize,
    pub sender: Option<FrameReference<'gc>>,
    pub home: Option<FrameReference<'gc>>,
    pub current_class: Option<ClassReference<'gc>>,
    pub kind: FrameKind<'gc>,
    pub uncontinuable: bool,
}
impl<'gc> Heap<'gc> {
    pub fn object_shape(&self, object: ObjectReference<'gc>) -> ObjectShape;
    pub fn allocate_instance(&mut self, class: ClassReference<'gc>, field_count: usize) -> ObjectReference<'gc>;
    pub fn allocate_array(&mut self, class: ClassReference<'gc>, elements: &[Value<'gc>]) -> ObjectReference<'gc>;
    pub fn allocate_string(&mut self, class: ClassReference<'gc>, text: CodePoints) -> ObjectReference<'gc>;
    pub(crate) fn allocate_symbol(&mut self, class: ClassReference<'gc>, symbol: Symbol) -> ObjectReference<'gc>;
    pub(crate) fn allocate_large_integer(&mut self, class: ClassReference<'gc>, integer: ArbitraryInteger) -> ObjectReference<'gc>;
    pub(crate) fn allocate_float(&mut self, class: ClassReference<'gc>, value: f64) -> ObjectReference<'gc>;
    pub(crate) fn allocate_closure(&mut self, class: ClassReference<'gc>, closure: NewClosure<'gc>) -> ObjectReference<'gc>;
    pub fn allocate_class(&mut self, metaclass: ClassReference<'gc>, class: NewClass<'gc>) -> ClassReference<'gc>;
    pub fn allocate_self_referential_class(&mut self, class: NewClass<'gc>) -> ClassReference<'gc>;
    pub(crate) fn allocate_activation(&mut self, class: ClassReference<'gc>, frame: FrameReference<'gc>) -> ObjectReference<'gc>;
    pub(crate) fn allocate_placeholder(&mut self, class: ClassReference<'gc>) -> ObjectReference<'gc>;
    pub(crate) fn fill_placeholder(&mut self, object: ObjectReference<'gc>, class: ClassReference<'gc>, identity_hash: u32, record: Record<'gc>);
    pub fn instance_fields(&self, object: ObjectReference<'gc>) -> Option<Vec<Value<'gc>>>;
    pub fn string_code_points(&self, object: ObjectReference<'gc>) -> Option<CodePoints>;
    pub(crate) fn allocate_placeholder_frame(&mut self) -> FrameReference<'gc>;
    pub fn frame_record(&self, frame: FrameReference<'gc>) -> FrameRecord<'gc>;
    pub fn fill_frame(&mut self, frame: FrameReference<'gc>, record: FrameRecord<'gc>);
}
// From Task 9: FrameReference names an object; these replace activation_object_frame:
impl<'gc> Heap<'gc> {
    pub fn frame_object(&self, frame: FrameReference<'gc>) -> ObjectReference<'gc>;
    pub fn object_frame(&self, object: ObjectReference<'gc>) -> Option<FrameReference<'gc>>;
}
```

---

### Task 1: The reserved guard values sit at the low end

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs:40-57`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:345-348` (the counter's doc), `:794-816` (`mint_dispatch_ids`), `:2416-2451` (its two tests)
- Modify: `crates/ns-runtime/src/heap/world.rs:43` (the counter's start)
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` tests at `:1351`, `:1418-1420`, `:1459-1461`, `:1507` (`from_counter(0..2)` → `from_counter(3..5)`)

**Interfaces:**
- Produces: `DispatchId::{NONE, VACANT, FORWARDED, FIRST_MINTED, LIMIT, from_counter, bits}` as listed above.

- [ ] **Step 1: Write the failing tests.** Replace the two tests at `interpreter/mod.rs:2416-2451` (keep their harness -- how they reach a `Mutator`) with:

```rust
#[test]
fn the_first_mint_follows_the_reserved_guards() {
    // (the existing harness that yields `mutator`)
    assert_eq!(
        mutator.mint_dispatch_ids().instances,
        DispatchId::from_counter(DispatchId::FIRST_MINTED)
    );
    assert_eq!(
        mutator.mint_dispatch_ids().instances,
        DispatchId::from_counter(DispatchId::FIRST_MINTED + 1)
    );
}

#[test]
fn minting_stops_below_twenty_two_bits_and_answers_none_once_exhausted() {
    // (the existing harness)
    mutator.next_dispatch_id = DispatchId::LIMIT - 1;
    assert_eq!(
        mutator.mint_dispatch_ids().instances,
        DispatchId::from_counter(DispatchId::LIMIT - 1)
    );
    assert_eq!(mutator.mint_dispatch_ids(), DispatchIds::NONE);
    assert_eq!(mutator.mint_dispatch_ids(), DispatchIds::NONE);
}
```

And in `mixin.rs`'s test module (create `#[cfg(test)] mod dispatch_id_tests` if none fits):

```rust
#[test]
fn the_reserved_guards_are_the_three_lowest_ids() {
    assert_eq!(DispatchId::NONE.bits(), 0);
    assert_eq!(DispatchId::VACANT.bits(), 1);
    assert_eq!(DispatchId::FORWARDED.bits(), 2);
    assert_eq!(DispatchId::FIRST_MINTED, 3);
    assert_eq!(DispatchId::LIMIT, 1 << 22);
}
```

- [ ] **Step 2: Run them to verify they fail.** Run: `cargo nextest run -p ns-runtime the_first_mint the_reserved_guards minting_stops`. Expected: compile errors (`FIRST_MINTED`, `LIMIT`, `FORWARDED`, `bits` not found).

- [ ] **Step 3: Implement.** In `mixin.rs`:

```rust
impl DispatchId {
    /// What `Heap::dispatch_id` answers for a value whose class has no
    /// application -- only a crafted image makes one -- and what an
    /// application built outside `apply_mixin` carries. Never cached. 0, so
    /// that a header word left zero by a missed initialization carries a
    /// guard no application was minted (`docs/design/object-model.md`, "The
    /// reserved guard values sit at the low end").
    pub const NONE: DispatchId = DispatchId(0);
    /// The guard of a cache entry that guards nothing: a vacant, polymorphic
    /// or megamorphic site. `Heap::dispatch_id` never answers it, so no
    /// receiver matches one.
    pub(crate) const VACANT: DispatchId = DispatchId(1);
    /// A forwarder's guard (road step 5): never minted, so that every send
    /// guarded on a forwarded receiver's own header misses.
    pub(crate) const FORWARDED: DispatchId = DispatchId(2);
    /// The first id the counter mints.
    pub(crate) const FIRST_MINTED: u32 = 3;
    /// One past the last id: the header word's guard field is 22 bits wide.
    pub(crate) const LIMIT: u32 = 1 << 22;

    pub(crate) fn from_counter(value: u32) -> DispatchId {
        debug_assert!((DispatchId::FIRST_MINTED..DispatchId::LIMIT).contains(&value));
        DispatchId(value)
    }

    /// The id as the header word's low 22 bits hold it.
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }
}

// The forwarder's guard is reserved now and used from road step 5 on; the
// assertion keeps it below every minted id.
const _: () = assert!(DispatchId::FORWARDED.0 < DispatchId::FIRST_MINTED);
```

In `interpreter/mod.rs`:

```rust
    pub(crate) fn mint_dispatch_ids(&mut self) -> DispatchIds {
        let first = self.next_dispatch_id;
        if first >= DispatchId::LIMIT {
            return DispatchIds::NONE;
        }
        self.next_dispatch_id = first + 1;
        DispatchIds {
            instances: DispatchId::from_counter(first),
        }
    }
```

Rewrite the counter's doc (`:345-347`) and `mint_dispatch_ids`'s doc (`:794-801`) to say: the counter starts at `DispatchId::FIRST_MINTED`, mints below `DispatchId::LIMIT` (22 bits, the header word's guard field), and answers `NONE` once exhausted, so that application runs uncached. In `world.rs:43` start the counter at `DispatchId::FIRST_MINTED`. In the inline-cache unit tests, replace `from_counter(0)`, `(1)`, `(2)` by `(3)`, `(4)`, `(5)`.

- [ ] **Step 4: Run the tests to verify they pass**, then the per-commit gate. Expected: all green; no golden changes (no output depends on an id's number).
- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime
git commit -m "feat: the reserved guard values are the three lowest ids, and minting stops below 22 bits"
```

---

### Task 2: Mixins, applications and compiled code have per-heap indices; the application cache is keyed by them

**Files:**
- Create: `crates/ns-runtime/src/heap/tables.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (`mod tables;`, a `tables: Tables` field on `Heap`, the accessors, `Heap::new`)
- Modify: `crates/ns-runtime/src/lib.rs:69` (re-export `MixinId`, `ApplicationIndex`, `CodeId`)
- Modify: `crates/ns-runtime/src/mixin.rs:726-822` (`MixinApplication.index`), `:884-890` (`mixin_key`/`application_key` stay for the image's mixin plan and `nested_slot`, see Step 3)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:305` (`applications` key type), `:1682-1825` (`apply_mixin`)
- Modify: the two test helpers that build an application outside `apply_mixin`: `crates/ns-runtime/src/types.rs:751` (`empty_application`) and `crates/ns-runtime/src/dispatch.rs:478` (`class_with_method`)
- Test: `crates/ns-runtime/src/heap/tables.rs` (unit tests), `crates/ns-runtime/tests/heap.rs`

**Interfaces:**
- Produces: `MixinId`, `ApplicationIndex`, `CodeId`, `Heap::{mixin_id, mixin, register_application, application, code_id, code_id_of_method, code, code_method}`, `MixinApplication::index`, as listed above.

- [ ] **Step 1: Write the failing tests.** In `crates/ns-runtime/tests/heap.rs` (copy the file's interpreter setup):

```rust
#[test]
fn a_mixin_keeps_one_id_and_two_mixins_never_share_one() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        let first = std::rc::Rc::new(ns_runtime::Mixin::empty(ns_runtime::Symbol::intern("First")));
        let second = std::rc::Rc::new(ns_runtime::Mixin::empty(ns_runtime::Symbol::intern("Second")));
        let first_id = mutator.heap_mut().mixin_id(&first);
        assert_eq!(mutator.heap_mut().mixin_id(&first), first_id);
        assert_ne!(mutator.heap_mut().mixin_id(&second), first_id);
        assert!(std::rc::Rc::ptr_eq(&mutator.heap().mixin(first_id).expect("registered"), &first));
    });
}

#[test]
fn an_application_answers_its_own_index_and_the_heap_answers_it_back() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        let object = mutator.builtin_class(ns_runtime::BuiltinClass::Object);
        let application = mutator.heap().class_application(object).expect("a class");
        let back = mutator.heap().application(application.index()).expect("registered");
        assert!(std::rc::Rc::ptr_eq(&back, &application));
    });
}

#[test]
fn compiled_code_keeps_one_id_and_records_its_method() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        let object = mutator.builtin_class(ns_runtime::BuiltinClass::Object);
        let application = mutator.heap().class_application(object).expect("a class");
        let method = application.mixin.compiled_methods().next().expect("Object has a method").clone();
        let id = mutator.heap_mut().code_id_of_method(&method);
        assert_eq!(mutator.heap_mut().code_id(&method.code), id);
        assert!(std::rc::Rc::ptr_eq(&mutator.heap().code(id).expect("registered"), &method.code));
        assert!(std::rc::Rc::ptr_eq(&mutator.heap().code_method(id).expect("a method"), &method));
    });
}
```

(Use the names the test file and `Mutator` already expose for a mutable heap and a builtin class; if `heap_mut` is not public to integration tests, put these three tests in `heap.rs`'s unit-test module instead, where `Mutator`'s crate-private API is reachable. `compiled_methods()` is `Mixin::compiled_methods` at `mixin.rs:620`; adapt the iterator shape to its real return type.)

- [ ] **Step 2: Run them to verify they fail.** Run: `cargo nextest run -p ns-runtime a_mixin_keeps an_application_answers compiled_code_keeps`. Expected: compile errors (`mixin_id`, `index`, `code_id` not found).

- [ ] **Step 3: Implement.** Create `heap/tables.rs`:

```rust
//! What a heap object names by a `SmallInteger` because it cannot live in
//! the words: a mixin, a mixin application, a compiled body
//! (`docs/design/object-model.md`, "Planned: the object layout"). Per heap,
//! because the objects naming them are, and append-only: road step 3a mints
//! these ids and reclaims none, since nothing drops a mixin before live
//! editing (`docs/OPEN-WORK.md`, "Live editing of mixins").

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::mixin::{Mixin, MixinApplication, MixinMethod};

macro_rules! table_index {
    ($name:ident, $what:literal) => {
        #[doc = concat!("The index of ", $what, " in its heap's table.")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub struct $name(u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }

            /// `None` past `u32::MAX`, which a table cannot reach before
            /// memory runs out.
            pub(crate) fn from_index(index: usize) -> Option<$name> {
                u32::try_from(index).ok().map($name)
            }
        }
    };
}

table_index!(MixinId, "a mixin");
table_index!(ApplicationIndex, "a mixin application");
table_index!(CodeId, "a compiled body");

/// A compiled body and, for a method's, the method -- so that a method
/// frame's code id names its method too, which the encoder writes and
/// `image::decode::frame_body` reads.
pub(crate) struct CodeEntry {
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) method: Option<Rc<MixinMethod>>,
}

/// The three tables. The address maps are keyed by `Rc::as_ptr`, which is
/// sound only because the table holds the strong `Rc` that keeps each
/// address valid and unique for as long as the entry lives -- forever, in
/// 3a.
#[derive(Default)]
pub(crate) struct Tables {
    pub(crate) mixins: Vec<Rc<Mixin>>,
    pub(crate) mixin_ids: IdentityMap<usize, MixinId>,
    pub(crate) applications: Vec<Rc<MixinApplication>>,
    pub(crate) codes: Vec<CodeEntry>,
    pub(crate) code_ids: IdentityMap<usize, CodeId>,
}
```

(Check `IdentityMap` implements `Default` through its `BuildHasher`; if it does not, write `Tables::new()` instead of deriving.)

In `heap.rs` add `mod tables; pub use tables::{ApplicationIndex, CodeId, MixinId}; pub(crate) use tables::{CodeEntry, Tables};`, a field `tables: Tables` on `Heap` (initialised in `Heap::new`, printed nowhere by `Debug`), and:

```rust
    // ---------- The Rust-side tables ----------

    pub(crate) fn mixin_id(&mut self, mixin: &Rc<Mixin>) -> MixinId {
        let key = crate::mixin::mixin_key(mixin);
        if let Some(id) = self.tables.mixin_ids.get(&key) {
            return *id;
        }
        let id = MixinId::from_index(self.tables.mixins.len())
            .expect("more than u32::MAX mixins in one heap");
        self.tables.mixins.push(mixin.clone());
        self.tables.mixin_ids.insert(key, id);
        id
    }

    pub fn mixin(&self, id: MixinId) -> Option<Rc<Mixin>> {
        self.tables.mixins.get(id.index()).cloned()
    }

    /// Enters `application` into the table, stamped with its index, and
    /// answers the shared handle every class object of it will name.
    pub(crate) fn register_application(
        &mut self,
        mut application: MixinApplication,
    ) -> Rc<MixinApplication> {
        let index = ApplicationIndex::from_index(self.tables.applications.len())
            .expect("more than u32::MAX applications in one heap");
        application.index = index;
        let application = Rc::new(application);
        self.tables.applications.push(application.clone());
        application
    }

    pub fn application(&self, index: ApplicationIndex) -> Option<Rc<MixinApplication>> {
        self.tables.applications.get(index.index()).cloned()
    }

    pub(crate) fn code_id(&mut self, code: &Rc<CompiledCode>) -> CodeId {
        self.register_code(code, None)
    }

    pub(crate) fn code_id_of_method(&mut self, method: &Rc<MixinMethod>) -> CodeId {
        self.register_code(&method.code, Some(method))
    }

    fn register_code(
        &mut self,
        code: &Rc<CompiledCode>,
        method: Option<&Rc<MixinMethod>>,
    ) -> CodeId {
        let key = Rc::as_ptr(code) as usize;
        if let Some(id) = self.tables.code_ids.get(&key).copied() {
            let entry = &mut self.tables.codes[id.index()];
            if entry.method.is_none() {
                entry.method = method.cloned();
            }
            return id;
        }
        let id = CodeId::from_index(self.tables.codes.len())
            .expect("more than u32::MAX compiled bodies in one heap");
        self.tables.codes.push(CodeEntry {
            code: code.clone(),
            method: method.cloned(),
        });
        self.tables.code_ids.insert(key, id);
        id
    }

    pub fn code(&self, id: CodeId) -> Option<Rc<CompiledCode>> {
        self.tables.codes.get(id.index()).map(|entry| entry.code.clone())
    }

    pub fn code_method(&self, id: CodeId) -> Option<Rc<MixinMethod>> {
        self.tables.codes.get(id.index()).and_then(|entry| entry.method.clone())
    }
```

In `mixin.rs`, give `MixinApplication` a field `pub(crate) index: ApplicationIndex` (documented: "Its index in its heap's table, the full-width identity every class object of it names and every identity comparison of applications uses (`docs/design/object-model.md`, \"An application has two ids\"); set by `Heap::register_application`.") with `pub fn index(&self) -> ApplicationIndex`. Every constructor of a `MixinApplication` writes `index: ApplicationIndex::UNREGISTERED`, a `pub(crate) const UNREGISTERED: ApplicationIndex = ApplicationIndex(u32::MAX)` added to `tables.rs`, which `register_application` overwrites.

In `apply_mixin` (`interpreter/mod.rs:1682-1825`):
- Key the cache by ids: `let key = (self.heap.mixin_id(mixin), superclass.map(|application| application.index()));` and change `Mutator.applications` to `IdentityMap<(MixinId, Option<ApplicationIndex>), Rc<MixinApplication>>` -- `IdentityBuildHasher` hashes a tuple of two small ids like it hashed two addresses (check `hash.rs`; if the hasher only accepts one `write_usize`, pack the key into one `u64`: `u64::from(mixin.index() as u32) << 32 | superclass.map_or(u64::from(u32::MAX), |index| index.index() as u64)` and key by that).
- Build the application as today, then `let application = self.heap.register_application(application);` instead of `Rc::new(...)`, and insert that.
- Rewrite `Mutator.applications`' doc (`:300-304`) to say it is keyed by the mixin's id and the superclass application's full-width index, strong until live editing.

`mixin_key` stays: the image's mixin plan (`image/mixins.rs`), `nested_slot` and `metaclass_mixins` key by it, and they hold the `Rc` that keeps the address valid, as before. `application_key` loses its last caller in `apply_mixin`; delete it if nothing else calls it (`rg -n "application_key" crates`).

In the two test helpers that build a `MixinApplication` directly (`types.rs:751`, `dispatch.rs:478`), keep `ApplicationIndex::UNREGISTERED`; they never reach the cache.

- [ ] **Step 4: Run the tests to verify they pass**, then the per-commit gate. Expected: all green, no golden changes.
- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime
git commit -m "feat: mixins, applications and compiled code have per-heap indices, and the application cache is keyed by them"
```

---

### Task 3: The applied clause packs into one SmallInteger, and its limit is a resolve and a decode error

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs:492-507` (`AppliedClause`, `CLAUSE_INDEX_LIMIT`, `pack`, `unpack`)
- Modify: `crates/ns-runtime/src/resolver/classes.rs` (a `clause_count_error` beside `duplicate_member_names`), `crates/ns-runtime/src/resolver/mod.rs:115-121` (call it)
- Modify: `crates/ns-runtime/src/image/decode.rs:28-448` (`DecodeError::ClauseIndexTooLarge`), `:450-724` (its `Display`), `:3672-3692` (`decode_applied_clause`), `:2846-2853` (`decode_root_section`)
- Test: `mixin.rs` unit tests, `resolver/classes.rs` unit tests, `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `MixinId` (Task 2).
- Produces: `CLAUSE_INDEX_LIMIT`, `AppliedClause::{pack, unpack}`, `DecodeError::ClauseIndexTooLarge { row: u32, index: u32 }`.

- [ ] **Step 1: Write the failing tests.** In `mixin.rs`'s test module:

```rust
#[test]
fn an_applied_clause_round_trips_through_its_small_integer_at_every_edge() {
    let largest_tag = *BuiltinClass::ALL.last().expect("builtins exist");
    let cases = [
        (0u32, 0usize, None),
        (u32::MAX, CLAUSE_INDEX_LIMIT - 1, Some(largest_tag)),
        (7, 1, Some(BuiltinClass::Object)),
        (u32::MAX, 0, None),
    ];
    for (mixin, index, builtin) in cases {
        let mixin = MixinId::from_index(mixin as usize).expect("fits");
        let packed = AppliedClause::pack(mixin, index, builtin);
        assert!(crate::fits_small_integer(packed), "{packed} is not a SmallInteger");
        assert_eq!(AppliedClause::unpack(packed), Some((mixin, index, builtin)));
    }
}

#[test]
fn a_packed_applied_clause_with_bit_sixty_set_is_negative() {
    let mixin = MixinId::from_index(0).expect("fits");
    let largest_tag = *BuiltinClass::ALL.last().expect("builtins exist");
    assert!(AppliedClause::pack(mixin, 0, Some(largest_tag)) < 0);
}

#[test]
fn an_unpacked_builtin_tag_past_the_builtins_is_refused() {
    // tag 63 names no builtin: 63 - 1 = 62 >= BuiltinClass::ALL.len()
    let packed = ((63i64 << 55) << 3) >> 3;
    assert_eq!(AppliedClause::unpack(packed), None);
}
```

In `resolver/classes.rs`'s test module:

```rust
#[test]
fn a_superclass_clause_of_more_than_two_to_the_twenty_three_clauses_is_refused() {
    assert!(clause_count_error(crate::mixin::CLAUSE_INDEX_LIMIT, "Wide").is_none());
    let message = clause_count_error(crate::mixin::CLAUSE_INDEX_LIMIT + 1, "Wide")
        .expect("refused");
    assert_eq!(
        message,
        "class `Wide` applies 8388609 mixins in its superclass clause; at most 8388608 are allowed"
    );
}
```

In `crates/ns-runtime/tests/image.rs`, next to `an_applied_levels_clause_its_row_does_not_declare_is_a_decode_error` (`:10195`), with its fixture:

```rust
/// A clause index the applied-clause field cannot number is refused before
/// it is looked up, so it is never truncated into another clause.
#[test]
fn an_applied_levels_clause_index_past_twenty_three_bits_is_refused() {
    let mut fixture = GraphFixture::with_source(APPLYING_MODULE);
    let marker = row_named(&fixture, "Marker");
    let applied = row_named(&fixture, "Applied");
    for index in [1u32 << 23, u32::MAX] {
        let bytes = class_records(2, |writer| {
            applied_level_record(writer, (1, marker), None, (applied, index));
            metaclass_record(writer, 0);
        });
        let error = fixture.decode_error(&bytes);
        assert_eq!(
            error,
            decode::DecodeError::ClauseIndexTooLarge { row: applied, index }
        );
        assert_eq!(
            error.to_string(),
            format!(
                "mixin application clause index {index} of row {applied} is past the 2^23 \
                 clauses a class object can name"
            )
        );
    }
}
```

And drop the `(applied, u32::MAX)` case from `an_applied_levels_clause_its_row_does_not_declare_is_a_decode_error`'s loop: that index is now refused by the new check first, and the new test pins it.

- [ ] **Step 2: Run them to verify they fail.** Run: `cargo nextest run -p ns-runtime applied_clause clause_count clause_index_past`. Expected: compile errors.

- [ ] **Step 3: Implement.** In `mixin.rs`:

```rust
/// How many `<:` clauses one superclass clause may apply: the clause index
/// is 23 bits of the class object's applied-clause `SmallInteger` -- a limit
/// a valid program can reach, taken knowingly (decided with the user on
/// 2026-10-03; `docs/design/object-model.md`, "Planned: the object layout").
pub(crate) const CLAUSE_INDEX_LIMIT: usize = 1 << 23;

const CLAUSE_INDEX_SHIFT: u32 = 32;
const BUILTIN_TAG_SHIFT: u32 = 55;
const BUILTIN_TAG_MASK: u64 = 0b11_1111;
const SIXTY_ONE_BITS: u64 = (1 << 61) - 1;

impl AppliedClause {
    /// The three parts as one `SmallInteger`: the 61-bit pattern
    /// sign-extended, so bit 60 makes it negative rather than too large.
    pub(crate) fn pack(mixin: MixinId, index: usize, named_builtin: Option<BuiltinClass>) -> i64 {
        debug_assert!(index < CLAUSE_INDEX_LIMIT);
        let tag = named_builtin.map_or(0, |builtin| {
            1 + BuiltinClass::ALL
                .iter()
                .position(|candidate| *candidate == builtin)
                .expect("every builtin is in BuiltinClass::ALL") as u64
        });
        let pattern = mixin.index() as u64
            | (index as u64) << CLAUSE_INDEX_SHIFT
            | tag << BUILTIN_TAG_SHIFT;
        ((pattern << 3) as i64) >> 3
    }

    /// `None` for a tag that names no builtin, which only a corrupt field
    /// could hold.
    pub(crate) fn unpack(packed: i64) -> Option<(MixinId, usize, Option<BuiltinClass>)> {
        let pattern = packed as u64 & SIXTY_ONE_BITS;
        let mixin = MixinId::from_index((pattern & u64::from(u32::MAX)) as usize)?;
        let index = ((pattern >> CLAUSE_INDEX_SHIFT) as usize) & (CLAUSE_INDEX_LIMIT - 1);
        let tag = (pattern >> BUILTIN_TAG_SHIFT) & BUILTIN_TAG_MASK;
        let named_builtin = match tag {
            0 => None,
            tag => Some(*BuiltinClass::ALL.get(tag as usize - 1)?),
        };
        Some((mixin, index, named_builtin))
    }
}

const _: () = assert!(BuiltinClass::ALL.len() < BUILTIN_TAG_MASK as usize);
```

In `resolver/classes.rs`:

```rust
/// The resolve error for a superclass clause applying more mixins than a
/// class object's applied-clause field can number
/// (`crate::mixin::CLAUSE_INDEX_LIMIT`), or `None`.
pub(super) fn clause_count_error(count: usize, class_name: &str) -> Option<String> {
    (count > crate::mixin::CLAUSE_INDEX_LIMIT).then(|| {
        format!(
            "class `{class_name}` applies {count} mixins in its superclass clause; \
             at most {} are allowed",
            crate::mixin::CLAUSE_INDEX_LIMIT
        )
    })
}
```

In `resolver/mod.rs`'s `resolve_class`, after the duplicate-member loop:

```rust
        if let Some(message) =
            classes::clause_count_error(declaration.mixin_applications.len(), &name.to_string())
        {
            self.errors.push(ResolveError {
                span: declaration.name.span,
                message,
            });
        }
```

In `decode.rs`, add beside `BadMixinApplicationClause`:

```rust
    /// A class record's or a root's `<:` clause index at or past
    /// `CLAUSE_INDEX_LIMIT`. The resolver refuses such a source, and a class
    /// object's applied-clause field cannot number one, so refusing it is the
    /// only answer that does not truncate it into another clause.
    ClauseIndexTooLarge { row: u32, index: u32 },
```

with `Display`: `"mixin application clause index {index} of row {row} is past the 2^23 clauses a class object can name"`. In `decode_applied_clause` and `decode_root_section`, after reading `index` and before the `>= mixin_applications.len()` check: `if index as usize >= crate::mixin::CLAUSE_INDEX_LIMIT { return Err(DecodeError::ClauseIndexTooLarge { row, index }); }`.

- [ ] **Step 4: Run the tests to verify they pass**, then the per-commit gate.
- [ ] **Step 5: Commit**

```bash
git add crates/ns-runtime
git commit -m "feat: the applied clause packs into one SmallInteger, and more than 2^23 clauses is a resolve and a decode error"
```

---

### Task 4: Nothing outside the heap names an object's Rust payload

**Files:**
- Create: `crates/ns-runtime/src/heap/object.rs` (moved: `Object`, `ObjectData`, `ClassData`, `ClosureData`, `Frame` from `types.rs`; `FrameSlot` from `heap.rs:380-383`), visibility `pub(super)`/`pub(in crate::heap)`
- Create: `crates/ns-runtime/src/heap/records.rs` (`ObjectShape`, `NewClass`, `NewClosure`, `Record`, `FrameRecord`)
- Modify: `crates/ns-runtime/src/trace.rs` (the impls for the moved types move into `heap/object.rs`; `trace.rs` keeps the traits and the impls for `FrameKind`, `Continuation` and the rest)
- Modify: every file outside the heap module that names a moved type. Counts (src): `image/encode.rs` 22, `interpreter/mod.rs` 12+5+2, `image/decode.rs` 10+7+4+2, `trace.rs` 8+2+2+1, `interpreter/instantiate.rs` 5+1, `dispatch.rs` 2+2, `builtins/collections.rs` 2, `interpreter/nested.rs` 2, `mixin.rs` 1+2, `builtins/primitive.rs` 1, `interpreter/future.rs` 1, `interpreter/locals.rs` 1, `interpreter/metaclass.rs` 1, `interpreter/value_class.rs` 1, `vm.rs` 1, `compiler/capture.rs` 7 and `compiler/lower.rs` 6 (`Frame {` there is the compiler's own type -- check each hit and leave a hit that is not `types::Frame` alone). Tests: `tests/image.rs`, `tests/vm.rs`, `tests/builtins_text.rs`, `tests/builtins_boolean_numbers.rs`, `tests/builtins_control_flow.rs`, `tests/builtin_object_inheritance.rs`, `tests/instantiate.rs`, `tests/nested_classes.rs`, `tests/vm_initializer.rs`.

**Interfaces:**
- Consumes: nothing new.
- Produces: everything under "heap/records.rs (Task 4)" in "The interfaces every later task relies on". `Heap::object_snapshot`, `object_replace`, `frame_snapshot`, `frame_replace`, `closure_data`, `allocate_object(class, ObjectData)` and the public `allocate_frame(Frame)` are removed.

This is a behaviour-preserving rewrite: no golden, no test expectation changes.

**How a site converts** (every site follows one row):

| Today | After |
|---|---|
| `heap.allocate_object(class, ObjectData::Instance(vec![Word::NIL; n]))` (`Mutator::new_instance_object`, `decide_instantiation`) | `heap.allocate_instance(class, n)` |
| `ObjectData::Array(words)` / `Array(vec![])` | `heap.allocate_array(class, &values)` |
| `ObjectData::String(code_points)` (`new_string_from_code_points`) | `heap.allocate_string(class, code_points)` |
| `ObjectData::Symbol(symbol)` (`new_symbol`) | `heap.allocate_symbol(class, symbol)` |
| `ObjectData::LargeInteger(integer)` / `Float(value)` (`new_integer`, `new_float`) | `allocate_large_integer` / `allocate_float` |
| `ObjectData::Closure(ClosureData { .. })` (`vm.rs` `make_closure_into`) | `heap.allocate_closure(block_class, NewClosure { .. })` |
| `ObjectData::Class(Box::new(ClassData::new(..)))` (`new_class_object`, `new_applied_level`, the bootstrap, metaclasses) | `heap.allocate_class(metaclass, NewClass { .. })`; a field set after creation (`this_class`, `applied_clause`, refusal) through the existing `class_set_*` setters, adding `class_set_applied_clause(class, AppliedClause) -> bool` |
| `ObjectData::Activation(frame)` (`new_activation_object`) | `heap.allocate_activation(class, frame)` |
| `match &object.data { ObjectData::X(..) => .. }` on a snapshot (encoder, `GraphTable::visit_value`) | `match heap.object_shape(object) { ObjectShape::X => .. }` plus the accessor for the payload: `instance_fields`, `array_elements`, `string_code_points`, `symbol_value`, `closure_code`/`closure_captured_frame`/`closure_copied_values`, the `class_*` accessors, `large_integer`, `float_value`, `activation_object_frame` |
| `heap.object_snapshot(object).class` / `.identity_hash` | `heap.object_class(object)` / `heap.object_identity_hash(object)` |
| decoder: `allocate_object(placeholder_class, ObjectData::Instance(Vec::new()))` then `object_replace(object, Object { .. })` | `allocate_placeholder(placeholder_class)` then `fill_placeholder(object, class, identity_hash, Record::X(..))`; `fill_placeholder` writes `DispatchId::NONE` as `object_replace` did, until `stamp_dispatch_ids` |
| decoder: `allocate_frame(placeholder_frame())` then `frame_replace(frame, decoded)` | `allocate_placeholder_frame()` then `fill_frame(frame, FrameRecord { .. })` (values, not words) |
| `heap.frame_snapshot(frame)` (encoder, tests) | `heap.frame_record(frame)` |
| `heap.closure_data(object)` (decoder `verify_closure`, tests) | `closure_code`, `closure_captured_frame`, `closure_copied_values` |
| a test that builds an `Object { .. }` or matches `ObjectData::X` | the allocation or `object_shape` row above |
| `trace.rs` impls of `Object`, `ObjectData`, `ClassData`, `ClosureData`, `Frame` | moved verbatim into `heap/object.rs` |

- [ ] **Step 1: Write the failing tests** in `crates/ns-runtime/tests/heap.rs`:

```rust
#[test]
fn every_kind_of_object_answers_its_shape() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let instance = mutator.heap_mut().allocate_instance(object_class, 2);
        assert_eq!(mutator.heap().object_shape(instance), ObjectShape::Instance);
        assert_eq!(
            mutator.heap().instance_fields(instance).map(|fields| fields.len()),
            Some(2)
        );
        let text = mutator.new_string("abc".to_string());
        let text = text.as_object().expect("an object");
        assert_eq!(mutator.heap().object_shape(text), ObjectShape::String);
        assert_eq!(
            mutator.heap().string_code_points(text),
            Some(CodePoints::from("abc"))
        );
        let symbol = mutator.new_symbol(Symbol::intern("abc"));
        assert_eq!(
            mutator.heap().object_shape(symbol.as_object().expect("an object")),
            ObjectShape::Symbol
        );
        assert_eq!(mutator.heap().object_shape(object_class), ObjectShape::Class);
    });
}
```

(Adapt `new_string`'s argument type and the mutable-heap accessor to what `Mutator` exposes.)

- [ ] **Step 2: Run it to verify it fails.** Run: `cargo nextest run -p ns-runtime every_kind_of_object_answers_its_shape`. Expected: compile errors (`ObjectShape`, `allocate_instance`, `instance_fields`, `string_code_points`).
- [ ] **Step 3: Move the types** into `heap/object.rs` with the trace impls, make them crate-private to the heap module, and create `heap/records.rs` with the five public types exactly as listed in "The interfaces every later task relies on" (each documented: what it is for and who builds it). `FrameKind`, `Continuation` and every other type in `types.rs` stay where they are.
- [ ] **Step 4: Implement the accessors** in `heap.rs`. Each allocation method builds the `Object` it names and goes through `push_object` (so the collection limit applies); `allocate_class` computes the header dispatch id from `metaclass` as `allocate_object` did; `allocate_self_referential_class` keeps its index-first construction. `fill_placeholder` converts each `Record` variant into the `ObjectData` variant it names, storing values with `self.store`; `fill_frame` likewise into `Frame`; `frame_record` converts a `Frame` back with `self.load` and keeps `frame_snapshot`'s debug assertion that the frame is not married. `object_shape` is one `match` on `data`. `instance_fields` mirrors `array_elements`; `string_code_points` clones the payload (copy-out, "The seam").
- [ ] **Step 5: Convert every site** by the table. Then check:

Run: `rg -n "\b(ObjectData|ClassData|ClosureData|FrameSlot)\b|\bObject \{|types::Frame\b|object_snapshot|object_replace|frame_snapshot|frame_replace|closure_data\(" crates --glob '!crates/ns-runtime/src/heap.rs' --glob '!crates/ns-runtime/src/heap/**'`
Expected: no hit, except the compiler's own `Frame` in `compiler/capture.rs`/`lower.rs` if it is a different type.

- [ ] **Step 6: Run the test, then the per-commit gate.** Expected: everything passes unchanged.
- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "refactor: nothing outside the heap module names an object's or a frame's Rust payload"
```

---

### Task 5: `Symbol` is a subclass of `String`; `Symbol new` is refused and `String new` answers an empty String

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns:822-875` (`String`, `Symbol`)
- Modify: `crates/ns-runtime/src/heap/object.rs` (`ObjectData::Symbol` carries its code points too), `crates/ns-runtime/src/heap.rs` (`code_points` lends a symbol's; `str_value`, `string_*` accessors answer for a symbol)
- Modify: `crates/ns-runtime/src/builtins/text.rs:186-306` (`execute_string` serves a symbol receiver; `,` accepts a symbol argument; `symbol_equals` is identity)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs:481-511` (`decide_factory`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:1670-1671`, `:1706-1707` (the hierarchy tests)
- Create: `goldens/library/string/a_symbol_is_a_string.ns`/`.toml`, `goldens/library/string/string_new_answers_an_empty_string.ns`/`.toml`, `goldens/library/string/symbol_new_is_refused.ns`/`.toml`

**Interfaces:**
- Consumes: Task 4's seam.
- Produces: no new Rust API. `Heap::string_size`, `string_code_point_at`, `string_slice`, `string_concatenation`, `str_value`, `string_code_points` answer for a symbol object as for a String of its text; `Heap::is_string` is true for a symbol object; `Heap::strings_equal` and `string_hash` are unchanged in what they answer.

What changes and why (each a consequence of `Symbol` being a `String`, as in every reference kernel -- `prior-art/somns/core-lib/Kernel.ns:435` -- and decided with the user on 2026-09-29/2026-10-03):
- A Symbol now understands `size`, `at:`, `copyFrom:to:`, `,` and `asSymbol` (inherited). `,` and `copyFrom:to:` answer a String, as `new_string_from_code_points` always tags `String`. `asSymbol` of a symbol answers that symbol.
- `,` accepts a symbol argument, since a symbol is a String: `'a' , #b` answers `'ab'`. Its error for a non-String argument keeps its text.
- `Transcript show: #sym` prints `sym` (its leaf reads `str_value`). If a golden pins the old error for it, that golden's expectation changes in this task, with this reason.
- `=` stays class-tight both ways (`text_equals`' exact-class guard is unchanged; `Symbol>>=` overrides); `Symbol>>hash` stays the interned index.
- `Symbol new` is refused; `String new` answers an empty String. A program's own subclass (`MySymbol = Symbol new`, `Text = String new`) has no builtin tag and still instantiates as an ordinary instance, so `goldens/library/builtin-subclass/a_subclass_of_a_builtin_falls_to_objects_methods`, `goldens/language/send/native_that_fails_on_its_receiver_falls_to_objects`, `goldens/runtime/cache/cached_primitive_that_fails_on_a_receiver_of_the_same_class` and `goldens/library/value/a_value_has_no_identity` do not change.

- [ ] **Step 1: Write the failing goldens.** `goldens/library/string/a_symbol_is_a_string.ns` (follow the neighbouring goldens' module shape and `send` selector -- copy `a_string_is_its_code_points.ns`'s header and `main:` form):

```newspeak
class ASymbolIsAString usingPlatform: platform = (|
    private transcript = platform Transcript.
|) (
public main = (
    transcript show: (#abc isKindOf: String) printString; cr.
    transcript show: #abc size printString; cr.
    transcript show: (#abc at: 2) printString; cr.
    transcript show: (#abc copyFrom: 1 to: 2) printString; cr.
    transcript show: (#abc copyFrom: 1 to: 2) class name; cr.
    transcript show: ('a' , #b) printString; cr.
    transcript show: (#a , 'b') printString; cr.
    transcript show: (#a , 'b') class name; cr.
    transcript show: (#a = 'a') printString; cr.
    transcript show: ('a' = #a) printString; cr.
    transcript show: (#abc asSymbol == #abc) printString; cr.
    transcript show: #abc; cr.
)
)
```

(Check how the corpus spells `isKindOf:` and `class name` -- `Object`'s class accessors in `Kernel.ns`. If `isKindOf:` does not exist, use `(#abc class superclass == String)`; if `name` does not answer a String, print `printString` of the class.)

Its `.toml`: `status = 0`, `stderr = ""`, `stdout` = the twelve lines `true`, `3`, `98`, `'ab'`, `String`, `'ab'`, `'ab'`, `String`, `false`, `false`, `true`, `abc`.

`string_new_answers_an_empty_string.ns` prints `String new size printString`, `(String new = '') printString`, `String new class name` → `0`, `true`, `String`.

`symbol_new_is_refused.ns` prints `before`, then evaluates `Symbol new` → `status = 1`, stdout `before`, stderr `Uncaught Newspeak error: cannot instantiate Symbol: a Symbol is made only by a literal or by asSymbol`.

- [ ] **Step 2: Run them to verify they fail.** Run: `cargo nextest run -p ns-runtime goldens -- a_symbol_is_a_string string_new symbol_new` (use the golden runner's filter form). Expected: FAIL (`doesNotUnderstand: #size`, a slotless String, no refusal).
- [ ] **Step 3: Implement the kernel.** In `Kernel.ns`, `Symbol = String new ()`; give `Symbol` documented `=`, `hash`, `asString` and `printString` as today. Its comment says it is a canonical `String`: one object per text (§5.1.6), made only by a literal or `asSymbol`.
- [ ] **Step 4: Implement the payload.** In `heap/object.rs`, `ObjectData::Symbol(Symbol, CodePoints)`; `allocate_symbol` fills the code points from `CodePoints::from(symbol.as_str())`; the private `code_points` lender answers both variants; `symbol_value` reads the first field. In `text.rs`, `execute_string` no longer declines for a symbol receiver (it already goes through `string_size`); `,` takes `heap.string_code_points(argument)` for either; `symbol_equals` becomes identity, `other.as_object().is_some_and(|other| other.same(receiver))`, since a symbol is canonical (§5.1.6) -- so it no longer reads the interned id -- keeping the future decline it has today.
- [ ] **Step 5: Implement the factory.** In `decide_factory`, before the default arm:

```rust
            Some(BuiltinClass::Symbol) => Err(Signal::Error(NewspeakError {
                selector: Symbol::NEW,
                receiver: Value::object(class),
                message: "cannot instantiate Symbol: a Symbol is made only by a literal or by asSymbol"
                    .to_string(),
            })),
            Some(BuiltinClass::String) => Ok(Step::Value(
                self.new_string_from_code_points(CodePoints::from("")),
            )),
```

(Use the exact `NewspeakError` construction the VM-mirror arm uses beside it.)

- [ ] **Step 6: Update the hierarchy tests** (`kernel.rs:1670-1671` → `Symbol`'s superclass is `String`; `:1706-1707` unchanged in substance) and run the per-commit gate. Fix any golden that pinned `show:`'s refusal of a symbol, naming the reason in the commit body.
- [ ] **Step 7: Commit**

```bash
git add crates goldens
git commit -m "feat: Symbol is a subclass of String; Symbol new is refused and String new answers an empty String"
```

---

### Task 6: Every object has a header word, with a 22-bit identity hash

**Files:**
- Create: `crates/ns-runtime/src/heap/header.rs`
- Modify: `crates/ns-runtime/src/heap/object.rs` (`Object { class, header: Header, data }`; `identity_hash` and `dispatch_id` fields go)
- Modify: `crates/ns-runtime/src/heap.rs` (`allocate_identity_hash`, `advance_identity_hash_past`, every reader of the two fields, `dispatch_id`, the restamps)
- Modify: `crates/ns-runtime/src/image/decode.rs:3330-3337`, `:3550-3555` (`IdentityHashExhausted` → `IdentityHashTooWide`), its `Display`
- Modify: `crates/ns-runtime/tests/image.rs:5478-5554`, `:5730-5741` (the ceiling tests move to `2^22`)
- Test: `heap/header.rs` unit tests, `heap.rs` unit tests

**Interfaces:**
- Produces (crate-private to the heap module):

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Format {
    FixedPointers = 0,
    IndexablePointers = 1,
    Frame = 2,
    Closure = 3,
    SixtyFourBit = 4,
    Forwarded = 5,
    Free = 6,
    Class = 7,
    ThirtyTwoBit = 8,   // 8..=9: the low bit is the number of unused units
    Bytes = 16,         // 16..=23: the low three bits are the number of unused units
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Header(u64);
impl Header {
    pub(super) fn new(guard: DispatchId, format: Format, unused: u8, word_count: usize) -> Header;
    pub(super) fn guard(self) -> DispatchId;
    pub(super) fn with_guard(self, guard: DispatchId) -> Header;
    pub(super) fn format(self) -> Format;
    pub(super) fn unused(self) -> u8;
    pub(super) fn identity_hash(self) -> u32;
    pub(super) fn with_identity_hash(self, hash: u32) -> Header;
    pub(super) fn negative(self) -> bool;
    pub(super) fn with_negative(self, negative: bool) -> Header;
    pub(super) fn word_count(self) -> usize;   // 255 means "in the overflow word" (3b)
}
pub(crate) const IDENTITY_HASH_LIMIT: u32 = 1 << 22;
```

`Format::Class` is a format of its own rather than "fixed pointer fields" (`object-model.md`'s table lists class objects under fixed pointers): `is_class`, which the decoder's verification and lookup ask of untrusted objects, then reads the header alone instead of following two class words. Recorded in `object-model.md` by Task 15.

- [ ] **Step 1: Write the failing tests** in `heap/header.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_field_reads_back_what_was_written_and_disturbs_no_other() {
        let guard = DispatchId::from_counter(DispatchId::LIMIT - 1);
        let header = Header::new(guard, Format::Bytes, 7, 254)
            .with_identity_hash(IDENTITY_HASH_LIMIT - 1)
            .with_negative(true);
        assert_eq!(header.guard(), guard);
        assert_eq!(header.format(), Format::Bytes);
        assert_eq!(header.unused(), 7);
        assert_eq!(header.word_count(), 254);
        assert_eq!(header.identity_hash(), IDENTITY_HASH_LIMIT - 1);
        assert!(header.negative());
        assert_eq!(header.0 >> 27 & 0b111_1, 0, "age and the remembered bit stay 0");
        assert_eq!(header.0 >> 33 & 1, 0, "the mark bit stays 0");
    }

    #[test]
    fn a_word_count_past_two_hundred_and_fifty_four_reads_as_the_overflow_mark() {
        let header = Header::new(DispatchId::NONE, Format::IndexablePointers, 0, 10_000);
        assert_eq!(header.word_count(), 255);
    }

    #[test]
    fn a_zero_header_carries_no_guard() {
        assert_eq!(Header(0).guard(), DispatchId::NONE);
    }
}
```

In `heap.rs`'s unit tests:

```rust
#[test]
fn the_identity_hash_counter_wraps_within_twenty_two_bits_and_skips_zero() {
    let mut heap = Heap::new();
    heap.next_identity_hash = IDENTITY_HASH_LIMIT - 1;
    assert_eq!(heap.allocate_identity_hash(), IDENTITY_HASH_LIMIT - 1);
    assert_eq!(heap.allocate_identity_hash(), 1);
    assert_eq!(heap.allocate_identity_hash(), 2);
}
```

In `tests/image.rs`, rewrite `an_identity_hash_at_the_ceiling_is_refused_rather_than_overflowing_the_counter` to write `1 << 22` and expect `DecodeError::IdentityHashTooWide(1 << 22)` with the message `"identity hash 4194304 does not fit the header word's 22 bits"`; rewrite `an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating` to load `(1 << 22) - 1` and assert the next fresh hash is 1 (the wrap); rewrite `a_tag_named_hash_at_the_ceiling_is_refused` the same way.

- [ ] **Step 2: Run them to verify they fail.** Run: `cargo nextest run -p ns-runtime header identity_hash`. Expected: compile errors, then failures.
- [ ] **Step 3: Implement `header.rs`** with the layout of the Global Constraints: `GUARD_MASK = (1 << 22) - 1`; format at 22, 5 bits; age 27-30 and remembered 31 untouched; negative bit 32; mark bit 33 untouched; hash at 34, 22 bits; word count at 56, 8 bits, `min(word_count, 255)`. `Format` decodes from the five bits with `ThirtyTwoBit` covering 8-9 and `Bytes` 16-23, `unused()` answering the low bits of those; any other code is unreachable for a header this heap wrote -- answer `Format::Free` for it rather than panicking, so a corrupt word walks as a gap.
- [ ] **Step 4: Put the header on `Object`.** `Object { class, header, data }`. Each allocation computes its format from the data: `Instance` → `FixedPointers`, `Array` → `IndexablePointers`, `Closure` → `Closure`, `Class` → `Class`, `Activation` → `FixedPointers` (a frame object from Task 9 on), `String`/`Symbol` → `Bytes` or `ThirtyTwoBit` by the code points' width with unused 0 (Task 8 makes the units real), `LargeInteger`/`Float` → `SixtyFourBit` with the negative bit from the sign; the word count is 1 (the class word) plus the payload's length in words. `dispatch_id` reads `header.guard()`; every restamp writes `with_guard`. `object_identity_hash` reads `header.identity_hash()`; `object_ensure_identity_hash`, `object_set_identity_hash` write `with_identity_hash` (the setter `debug_assert!`s the hash is below `IDENTITY_HASH_LIMIT`).
- [ ] **Step 5: The counter.** `allocate_identity_hash`: answer `next`, then `next = if next + 1 == IDENTITY_HASH_LIMIT { 1 } else { next + 1 }`. `advance_identity_hash_past(max)`: `max < IDENTITY_HASH_LIMIT` is guaranteed by the decoder (`debug_assert!` it); `if next <= max { next = if max + 1 == IDENTITY_HASH_LIMIT { 1 } else { max + 1 } }`. Rewrite both docs: hashes are not unique once the counter has wrapped, which a hash need not be (`docs/design/object-model.md`, "The identity hash is 22 bits wide").
- [ ] **Step 6: The decoder.** Replace `IdentityHashExhausted` by `IdentityHashTooWide(u32)` (doc: a hash the header word cannot hold; refusing it is the only answer that does not alias it to another), refused for `hash >= IDENTITY_HASH_LIMIT` in the object loop and the tag-named table; `hash == 0` in the tag-named table stays `UnassignedTagNamedHash`.
- [ ] **Step 7: Run the tests, then the per-commit gate and `cargo nextest run --workspace --release`.** The release size test `an_object_is_the_size_the_amplification_figure_assumes` changes: `Object` loses a `u32` and a `u32` and gains a `u64` -- update its expected size to what `size_of::<Object>()` now reports and its comment to say Task 12 replaces it.
- [ ] **Step 8: Commit**

```bash
git add crates
git commit -m "feat: every object has a header word, and the identity hash is 22 bits wide"
```

---

### Task 7: Instances, arrays, large integers and boxed floats are words

**Files:**
- Modify: `crates/ns-runtime/src/heap/object.rs` (`ObjectData::Words(Box<[u64]>)` replaces `Instance`, `Array`, `LargeInteger`, `Float`)
- Modify: `crates/ns-runtime/src/heap.rs` (the accessors for these four kinds read by `header.format()`), `heap/value.rs` (a `pub(super)` `Word::from_bits`/`bits` pair)
- Modify: `crates/ns-runtime/src/arbitrary_integer/mod.rs` (`ArbitraryInteger::from_sign_and_digits(negative: bool, digits: &[u64]) -> ArbitraryInteger` and `digits(&self) -> &[u64]`, if they do not exist)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`, `builtins/value.rs` (read a `LargeInteger`'s digits in place: `Heap::with_large_integer(object, read: impl FnOnce(bool, &[u64]) -> T) -> Option<T>`)
- Test: `heap.rs` unit tests

**Interfaces:**
- Consumes: Task 6's `Header`, `Format`.
- Produces: `Heap::with_large_integer<T>(&self, object, read: impl FnOnce(bool, &[u64]) -> T) -> Option<T>` (crate-public), the lender `docs/OPEN-WORK.md`'s "a `LargeInteger` is copied for every leaf send" item asks for; `large_integer` stays (copy-out) for callers that keep the integer.

How each kind lies in the words: `FixedPointers` -- the fields as `Word` bits; `IndexablePointers` -- the elements; a `LargeInteger` -- `SixtyFourBit`, the magnitude's digits least significant first, the sign in the header's negative bit, the class word naming `LargeInteger`; a boxed `Float` -- `SixtyFourBit`, one word of `f64::to_bits`, the class naming `Float`. `is_large_integer` and `float_value`'s boxed arm check `SixtyFourBit` and the class's builtin (`class_builtin(object_class(object))`), since both kinds share the format, as Spur's do.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s unit tests (with the module's `named_class` helper):

```rust
#[test]
fn a_large_integer_lends_its_digits_and_its_sign_in_place() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "LargeInteger");
    let integer = crate::ArbitraryInteger::from_sign_and_digits(true, &[1, 2]);
    let object = heap.allocate_large_integer(class, integer.clone());
    assert_eq!(heap.with_large_integer(object, |negative, digits| (negative, digits.to_vec())),
        Some((true, vec![1, 2])));
    assert_eq!(heap.large_integer(object), Some(integer));
}

#[test]
fn an_instance_field_past_its_word_count_answers_none() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "Point");
    let object = heap.allocate_instance(class, 2);
    assert!(heap.instance_field(object, 1).is_some());
    assert!(heap.instance_field(object, 2).is_none());
    assert!(!heap.instance_set_field(object, 2, Value::nil()));
}
```

(`named_class` does not mark its class a builtin, so `with_large_integer` must not depend on the class for the digits: it checks the format, and `is_large_integer` -- which does check the builtin -- is not used here.)

- [ ] **Step 2: Run them to verify they fail.** Run: `cargo nextest run -p ns-runtime lends_its_digits past_its_word_count`. Expected: compile errors.
- [ ] **Step 3: Implement.** Convert the four variants to `Words`, every accessor through the header's format. A pointer word is stored as `Word` bits; the tracer (still in `heap/object.rs`) traces `Words` only for `FixedPointers` and `IndexablePointers`. `replace_resolved_futures` rewrites `FixedPointers`/`IndexablePointers` words. `describe` keeps its texts, deciding by format and builtin.
- [ ] **Step 4: Lend the digits.** Convert the leaves that copy a `LargeInteger` for one read (`execute_large_integer`, `integer_operand`, the derived `=`/`hash` in `builtins/value.rs`) to `with_large_integer`, building a borrowed view where the arithmetic needs an `ArbitraryInteger` only if the arithmetic module offers one (else keep the copy at those sites and say so in the commit body).
- [ ] **Step 5: Run the tests, then the per-commit gate.**
- [ ] **Step 6: Commit**

```bash
git add crates
git commit -m "feat: instances, arrays, large integers and boxed floats are words read by format"
```

---

### Task 8: Strings and symbols are byte- and 32-bit-indexed words

**Files:**
- Modify: `crates/ns-runtime/src/heap/object.rs` (`String` and `Symbol` variants go: both are `Words` of format `Bytes`/`ThirtyTwoBit`, units packed little-endian into the words, the unused count in the format's low bits)
- Modify: `crates/ns-runtime/src/heap.rs` (`code_points`, `string_*`, `str_value`, `symbol_value`, `describe`), `crates/ns-runtime/src/code_points.rs` (`CodePoints::from_bytes(Box<[u8]>)`, `from_wide(Box<[u32]>)`, `as_bytes() -> Option<&[u8]>`, `as_wide() -> Option<&[u32]>`, keeping the canonical-width invariant)
- Modify: `crates/ns-runtime/src/symbol.rs` (`Symbol::lookup(text: &str) -> Option<Symbol>`: a lookup that never interns)
- Test: `heap.rs` unit tests, `crates/ns-runtime/tests/builtins_text.rs`

**Interfaces:**
- Produces: `Symbol::lookup`. `Heap::symbol_value(object)` answers `Some` exactly for an object whose class is the builtin `Symbol` (`class_builtin(object_class(object)) == Some(BuiltinClass::Symbol)`) and whose text the interner holds -- always, since `Mutator::new_symbol` and the decoder intern before they allocate.

A symbol object no longer carries its interned id: `symbol_value` finds it by the object's text (`Symbol::lookup`), which is a hash of the text. Its callers are a `Symbol>>hash` send, the derived value walk, `Message>>sentTo:`, `moduleNamed:` and error paths ("Symbol object callers" in the plan's research) -- none per ordinary send; `Symbol>>=` is identity since Task 5 and reads no id.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s unit tests:

```rust
#[test]
fn a_string_of_nine_bytes_spans_two_words_and_answers_each_code_point() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "String");
    let object = heap.allocate_string(class, crate::CodePoints::from("abcdefghi"));
    assert_eq!(heap.string_size(object), Some(9));
    assert_eq!(heap.string_code_point_at(object, 8), Some(u32::from('i')));
    assert_eq!(heap.string_code_point_at(object, 9), None);
    assert_eq!(heap.str_value(object).as_deref(), Some("abcdefghi"));
}

#[test]
fn a_wide_string_keeps_its_width_and_its_text() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "String");
    let object = heap.allocate_string(class, crate::CodePoints::from("€uro 😀"));
    assert_eq!(heap.string_size(object), Some(6));
    assert_eq!(heap.str_value(object).as_deref(), Some("€uro 😀"));
}
```

And in `symbol.rs`'s tests: `Symbol::lookup("never interned anywhere 7f3a")` is `None`; `Symbol::lookup(Symbol::intern("abc").as_str()) == Some(Symbol::intern("abc"))`.

- [ ] **Step 2: Run them to verify they fail.** Expected: the tests compile against Task 5's representation and pass for size/text; they fail on `Symbol::lookup` (missing). Write `lookup` last so the representation tests can be run before and after Step 3 to confirm nothing moves.
- [ ] **Step 3: Implement.** Pack units into words: a byte string's unit `i` is byte `i % 8` of word `i / 8`, little-endian; a 32-bit string's unit `i` is half `i % 2` of word `i / 2`; the unused count is `words * per_word - size`. `allocate_symbol(class, symbol)` packs `symbol.as_str()`. `symbol_value` checks the class's builtin, rebuilds the text (`str_value`) and calls `Symbol::lookup`. Remove `ObjectData::String`/`Symbol`.
- [ ] **Step 4: Run the tests, then the per-commit gate.** Task 5's goldens and every string golden pass unchanged.
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: strings and symbols are byte- and 32-bit-indexed words"
```

---

### Task 9: Reified frames are objects of the frame format, and the frame arena goes

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the frame arena `frames`, `FrameSlot`, `next_frame_collection`, the frame half of `Forwarder`, `Rotation` of frames, `compact`'s second table, `reset_collection_limits`, `traced_size`, `heap_statistics`, every `frame_*` accessor, `reify`, `pop_activation`, `closure_home`, `activation_object_frame`), `heap/object.rs` (`Frame`, `FrameSlot`, `ObjectData::Activation` go), `heap/mark.rs` (one bit set, one work list), `trace.rs` (`Tracer::frame` stays as a method, implemented by every tracer as the object it names)
- Modify: `crates/ns-runtime/src/heap/world.rs` (no change in order; the frame forwarding goes through objects)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:1519-1522` (`new_activation_object` answers the frame object), `builtins/activation.rs:126` (`object_frame`), the bootstrap (record the `Activation` class on the heap: `Heap::set_activation_class`)
- Modify: `HeapStatistics`/`CollectorStatistics` (`live_frames`, `frame_slots` go) and their readers: `crates/ns-cli/src/entry.rs:34`, `crates/ns-runtime/tests/collector.rs:215-305`
- Modify: `crates/ns-runtime/tests/image.rs:2566-2669` (`frame_grow_temporaries` goes; see Step 5)
- Test: `heap.rs` unit tests (rewritten as listed), `tests/heap.rs`

**Interfaces:**
- Consumes: Task 2's `code_id`/`code_id_of_method`/`code`/`code_method`; Task 6's `Format::Frame`.
- Produces: `Heap::frame_object`, `Heap::object_frame` (replacing `activation_object_frame`); `Heap::set_activation_class(class)`, the class every frame object `reify` allocates is an instance of -- a handle the heap holds, so `begin_marking` marks it as a root and `forward_everything` forwards it, and a bare-heap unit test calls `set_activation_class(named_class(&mut heap, "Activation"))` before its first `reify`. `FrameReference` keeps its type, its brand and its debug origin and epoch, and now wraps an object index. `frame_register_shape` answers `(program_counter, argument_count, register_count)`. `frame_grow_temporaries` is removed.

A frame object (format `Frame`, class `Activation`) holds ten fixed fields, then its registers:

| Field | Holds |
|---|---|
| 0 sender | a frame object or `nil` |
| 1 home | a frame object or `nil` |
| 2 receiver | any value |
| 3 current class | a class object or `nil` |
| 4 program counter | `SmallInteger` |
| 5 code id | `SmallInteger` (`CodeId`), or `nil` for `TopLevel` |
| 6 kind | `SmallInteger`: 0 method, 1 closure, 2 top level, 3 initializer, 4 superclass clause, 5 superclass message, 6 mixin application message (the `FRAME_*` numbers) |
| 7 kind operand | the closure (kind 1), the class (kinds 3-6), else `nil` |
| 8 uncontinuable | `true`/`false` |
| 9 married | `SmallInteger` stack-entry index while married, else `nil` |

`FrameKind::Method(method)` is rebuilt from `code_method(code id)`; `reify` registers a method frame's code with `code_id_of_method`. The tracer skips the registers while field 9 is not `nil`: the stack holds them then.

`reify` allocates the frame with its code's full `register_count` registers, all `nil`; the divorce in `pop_activation` copies the registers into it when `retains_registers` is set and leaves them `nil` otherwise. So no frame object ever grows, and a frame read past its arguments and temporaries answers `nil` as before. The decoder's frames are allocated, until Task 13, with the arguments and temporaries the wire carries.

- [ ] **Step 1: Rewrite the heap unit tests that name the frame arena** (each keeps its name and what it pins):
  - `an_activation_that_is_never_reified_leaves_nothing_behind`: assert `heap.objects.len()` did not grow.
  - `reifying_an_activation_twice_answers_the_same_frame`: assert one object was allocated.
  - `popping_a_reified_activation_divorces_its_frame`: `frame_register_shape == (4, 1, register_count)`.
  - `a_frame_slot_is_the_size_the_amplification_figure_assumes`: delete (Task 12 pins the slot).
  - `marking_reaches_a_married_frame_through_its_entry`, `marking_reaches_a_frame_through_an_activation_object`: assert `marker.objects.is_set(frame object's index)`.
  - `an_activation_object_answers_its_frame`: `object_frame(frame_object(frame)) == Some(frame)`, `describe` answers `"an Activation"`.
  - `reset_collection_limits_reads_each_arenas_own_length`: one arena.
  - `a_move_gives_every_slot_a_new_index_and_rewrites_what_names_it`: frames move with the objects.

  And add:

```rust
#[test]
fn a_married_frame_survives_a_collection_and_reads_its_live_registers() {
    let mut heap = Heap::new();
    let activation = heap.push_activation(header(), &[Value::small_integer(7)], 2);
    let frame = heap.reify(activation);
    heap.set_register(heap.activation_register_base(activation), 1, Value::small_integer(9));
    let mut root = Value::object(heap.frame_object(frame));
    heap.collect_for_test(&mut [&mut root]);
    let frame = heap.object_frame(root.as_object().expect("an object")).expect("a frame");
    assert_eq!(heap.frame_local(frame, 0).as_small_integer(), Some(7));
    assert_eq!(heap.frame_local(frame, 1).as_small_integer(), Some(9));
    assert!(heap.frame_activation(frame).is_some());
}
```

(Use the real signatures of `push_activation`, `header()` and `collect_for_test`; the test pins that a married frame's marriage index and live reads survive compaction.)

- [ ] **Step 2: Run them to verify they fail.** Expected: compile errors (`frame_object`, `object_frame`).
- [ ] **Step 3: Implement the frame object** and every `frame_*` accessor over it; `FrameReference` keeps its own handle type with an object index; `check_frame_origin` stays. `allocate_activation` goes: the frame object is the activation object, so `new_activation_object(frame)` answers `frame_object(frame)`. `allocate_placeholder_frame` allocates a frame object with no registers; `fill_frame` reallocates the slot's payload with the record's register count (the arena slot keeps its index).
- [ ] **Step 4: One arena for the collector.** `Marker` keeps one `MarkBits` and one work list; its `Tracer::frame` marks the frame's object index. `Forwarder::frame` applies the object relocation. `compact` compacts one arena; `move_everything` rotates one; `begin_marking`/`drain_marking` lose the frame half; `traced_size`, `heap_statistics`, `collector_statistics` lose the frame terms. `request_collection` is reached from `reify` through `push_object`.
- [ ] **Step 5: `frame_grow_temporaries` goes.** Its only caller, `tests/image.rs`'s `a_closure_and_its_paused_home_frame_round_trip`, grows a dead home frame by one temporary to give the closure a register to read: with full register files the home frame already has its code's registers, so the test drops the grow and writes the existing register.
- [ ] **Step 6: Run the tests, then the per-commit gate and `cargo nextest run --workspace --release`.**
- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat: reified frames are objects of the frame format, and the frame arena goes"
```

---

### Task 10: Closures are words

**Files:**
- Modify: `crates/ns-runtime/src/heap/object.rs` (`ClosureData`, `ObjectData::Closure` go), `crates/ns-runtime/src/heap.rs` (`allocate_closure`, `closure_*`, `fill_copied_values`, `closure_set_feedback_base`, `closure_home`), `crates/ns-runtime/src/interpreter/inline_cache.rs:29` (`FeedbackBase::{raw, from_raw}`, `pub(crate)`)
- Test: `heap.rs` unit tests

**Interfaces:**
- Consumes: Task 2's `code_id`/`code`; Task 9's frame objects.
- Produces: no new public API; `NewClosure` is unchanged.

A closure object (format `Closure`, class `Block`): field 0 the code id (`SmallInteger`), field 1 the captured frame (a frame object), field 2 the feedback base (`SmallInteger`, `FeedbackBase::raw()`), then its copied values. `allocate_closure` registers the code with `code_id` (an identity-map probe per `MakeClosure`, measured in Task 14); `closure_activation` answers `code(id)`.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn a_closure_keeps_its_code_frame_feedback_base_and_copied_values_in_its_words() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "Block");
    let frame = heap.allocate_placeholder_frame();
    let code = trivial_code();
    let object = heap.allocate_closure(class, NewClosure {
        code: code.clone(),
        captured_frame: frame,
        copied: vec![Value::small_integer(5)],
        feedback_base: FeedbackBase::NOT_ASSIGNED,
    });
    let (body, captured, base) = heap.closure_activation(object).expect("a closure");
    assert!(std::rc::Rc::ptr_eq(&body, &code));
    assert!(captured.same(frame));
    assert_eq!(base, FeedbackBase::NOT_ASSIGNED);
    assert_eq!(heap.closure_copied_values(object).map(|values| values.len()), Some(1));
}
```

- [ ] **Step 2: Run it before the change.** Run: `cargo nextest run -p ns-runtime keeps_its_code_frame_feedback_base`. Expected: PASS -- the test is written against Task 4's API, which this task does not change; it pins that the conversion keeps what a closure answers, so it must still pass after Step 3.
- [ ] **Step 3: Implement** the layout; trace the captured frame and the copied values (fields 1 and 3..); `replace_resolved_futures` rewrites the copied values only, as today.
- [ ] **Step 4: Run the tests, then the per-commit gate.**
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: closures are words: code id, captured frame, feedback base and copied values"
```

---

### Task 11: Class objects are words, and a value-class refusal is heap data

**Files:**
- Modify: `crates/ns-runtime/src/heap/object.rs` (`ClassData`, `ObjectData::Class` go), `crates/ns-runtime/src/heap.rs` (every `class_*` accessor and setter, `allocate_class`, `allocate_self_referential_class`, `header_dispatch_id`, `class_set_application`, `class_install_application`, `describe`, `class_display_name`)
- Modify: `crates/ns-runtime/src/types.rs:109-116` (`ValueClassRefusal<'gc>::NonValueEnclosingObject { level: Option<Symbol>, offender: ObjectReference<'gc> }`, `described` gone)
- Modify: `crates/ns-runtime/src/interpreter/value_class.rs` (it records the offender, not its description), `interpreter/nested.rs:229-251`, `image/decode.rs:3029-3057` (store through `class_set_value_refusal`, which now allocates the `Array`), `interpreter/instantiate.rs:1386-1418` (render with `describe` at refusal time)
- Modify: `crates/ns-runtime/src/builtins/behavior.rs:44-52`, `builtins/value.rs:522-528` (application identity by `ApplicationIndex`)
- Test: `heap.rs` unit tests; the four refusal goldens in `goldens/library/value/` (unchanged expectations)

**Interfaces:**
- Consumes: Tasks 2, 3, 6.
- Produces: `Mutator::record_value_class_refusal(&mut self, class: ClassReference<'gc>, refusal: ValueClassRefusal<'gc>) -> bool` -- makes the level's symbol object with `new_symbol` and the two-element `Array` of the builtin `Array` class, then stores it with `Heap::class_set_value_refusal(&mut self, class, refusal: ObjectReference<'gc>) -> bool`; its callers are `nested.rs`'s `after_clause` and the decoder's `rebuild_applications`, which both hold the `Mutator`. `Heap::class_value_refusal(&self, class) -> Option<ValueClassRefusal<'gc>>` reads the `Array` back, the level through `symbol_value`; `Heap::class_set_applied_clause(&mut self, class, applied: AppliedClause) -> bool` (from Task 4) packs it, registering the declaration with `mixin_id`; `class_applied_clause`/`class_applied_origin` rebuild the `AppliedClause` from the packed field and `mixin(id)`; `Heap::class_application_index(&self, class) -> Option<ApplicationIndex>` answers field 7.

A class object (format `Class`, class its metaclass) holds eight fields:

| Field | Holds |
|---|---|
| 0 enclosing | an object or `nil` |
| 1 declaring class | a class object or `nil` |
| 2 superclass | a class object or `nil` |
| 3 this class | for a metaclass, its class; else `nil` |
| 4 builtin | `SmallInteger` index into `BuiltinClass::ALL`, or `nil` |
| 5 value-class refusal | `nil`, or an `Array` of the offender and the level's name as a symbol or `nil` |
| 6 applied clause | `AppliedClause::pack`, or `nil` |
| 7 application | `SmallInteger` `ApplicationIndex` |

The instances id that `ClassData.dispatch_id` copied is read through the application (`application(index).dispatch_ids.instances`). `class_own_slot_base` and `class_inherits_value`, which `InitializeSlot` and every initializer's slot write ask, read the table entry without cloning the `Rc` (`self.tables.applications[index].own_slot_base`).

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s unit tests (`named_class` as Task 4 left it):

```rust
#[test]
fn a_class_objects_applied_clause_round_trips_through_its_field() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "Level");
    let declaration =
        std::rc::Rc::new(crate::mixin::Mixin::empty(Symbol::intern("Applying")));
    assert!(heap.class_set_applied_clause(
        class,
        crate::mixin::AppliedClause {
            declaration: declaration.clone(),
            index: 3,
            named_builtin: Some(BuiltinClass::Array),
        },
    ));
    let origin = heap.class_applied_origin(class).expect("an applied level");
    assert!(std::rc::Rc::ptr_eq(&origin.declaration, &declaration));
    assert_eq!(origin.index, 3);
    assert_eq!(origin.named_builtin, Some(BuiltinClass::Array));
}

```

And in `crates/ns-runtime/tests/heap.rs`, where a `Mutator` makes the symbol and the `Array`:

```rust
#[test]
fn a_value_class_refusal_keeps_its_offender_and_its_level() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        let object_class = mutator.builtin_class(BuiltinClass::Object);
        let class = mutator.builtin_class(BuiltinClass::Value);
        let offender = mutator.heap_mut().allocate_instance(object_class, 0);
        assert!(mutator.record_value_class_refusal(
            class,
            ValueClassRefusal::NonValueEnclosingObject {
                level: Some(Symbol::intern("Counter")),
                offender,
            },
        ));
        let Some(ValueClassRefusal::NonValueEnclosingObject { level, offender: kept }) =
            mutator.heap().class_value_refusal(class)
        else {
            panic!("the refusal was recorded");
        };
        assert_eq!(level, Some(Symbol::intern("Counter")));
        assert!(kept.same(offender));
    });
}
```

(The test borrows the `Value` class object only as a class to hang the field on; it does not instantiate it. Use the mutable-heap accessor `Mutator` exposes.)

- [ ] **Step 2: Run them to verify they fail.** Expected: compile errors (the refusal's new shape).
- [ ] **Step 3: Implement** the layout and the accessors. `value_class.rs` keeps its walk; where it read `described`, it carries the offender. `value_class_refusal_error` renders `heap.describe(Value::object(offender))` when `new` refuses, with today's two texts. `behavior:sharesApplicationWith:` and `classes_can_be_equal` compare `class_application_index` (a new accessor answering field 7) instead of `Rc::ptr_eq`.
- [ ] **Step 4: Run the tests, then the per-commit gate.** The four refusal goldens pass unchanged: their offenders are instances, whose description is fixed by their class's name.
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: class objects are words, and a value-class refusal is heap data rendered when new refuses"
```

---

### Task 12: Each arena slot is one block of words, traced by format

**Files:**
- Modify: `crates/ns-runtime/src/heap/object.rs` (`Object` and `ObjectData` go; the slot is `Box<[u64]>`: word 0 the header, word 1 the class word, then the fields), `crates/ns-runtime/src/heap.rs` (`objects: Vec<Box<[u64]>>`, every accessor, `forward_everything`, `drain_marking`, `compact_arena`'s caller), `trace.rs` (no `Trace` impl for an object remains; `heap.rs` traces a block by format)
- Test: `heap.rs` unit tests; the release size tests

**Interfaces:**
- Produces: nothing new for callers. Inside the heap: `fn trace_block<T: Tracer>(block: &mut [u64], tracer: &mut T)` -- the class word always; `FixedPointers`, `IndexablePointers`, `Class`: every field; `Closure`: the captured frame and the copied values (the code id and feedback base are immediates and trace as nothing); `Frame`: the ten fixed fields, and the registers only when field 9 is `nil`; `Bytes`, `ThirtyTwoBit`, `SixtyFourBit`: nothing.

Every accessor checks the field index against the header's word count and, in a debug build, the header's word count against the block's length (`word_count() == 255 || word_count() == block.len() - 1`).

- [ ] **Step 1: Write the failing tests:**

```rust
#[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
#[test]
fn an_object_slot_is_one_boxed_block_of_words() {
    assert_eq!(std::mem::size_of::<Box<[u64]>>(), 16);
    // The arena's element type is `Box<[u64]>`: the decode amplification of
    // Task 13 is computed from 16 bytes per slot plus the block.
}

#[test]
fn a_block_is_its_header_its_class_word_and_its_fields() {
    let mut heap = Heap::new();
    let class = named_class(&mut heap, "Point");
    let object = heap.allocate_instance(class, 2);
    let block = &heap.objects[object.index as usize];
    assert_eq!(block.len(), 4);
    assert_eq!(Header(block[0]).format(), Format::FixedPointers);
    assert_eq!(Header(block[0]).word_count(), 3);
}
```

- [ ] **Step 2: Run them to verify they fail.** Expected: `heap.objects` is still `Vec<Object>`.
- [ ] **Step 3: Implement** the collapse. `object_class` reads word 1 through `load`; `object_set_class` writes it through `store`. The forwarding and marking walks call `trace_block`. Delete the release test `an_object_is_the_size_the_amplification_figure_assumes` in favour of the new one.
- [ ] **Step 4: Run the tests, then the per-commit gate and `cargo nextest run --workspace --release`.**
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: each arena slot is one block of words, traced by format"
```

---

### Task 13: Frames are object records on the wire, and the decoder sizes every record before it allocates

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs:210-244` (`OBJECT_FRAME = 10`; `RETIRED_OBJECT_BODIES = [4, 5, 9]` with 9's reason; "Next free number: 11")
- Modify: `crates/ns-runtime/src/image/encode.rs:860-1952` (`Pending::Frame` goes; a frame object is an object record; `encode_graph_section` writes one count; `encode_frame` becomes the `OBJECT_FRAME` body; the full register file)
- Modify: `crates/ns-runtime/src/image/decode.rs:2229-3495`, `:3694-3860` (one table; `MIN_FRAME_RECORD_BYTES` goes; the two passes; `verify_graph`'s register-less exemption goes)
- Modify: `crates/ns-runtime/src/image/inspect.rs:443-565`, `:748-772`, `:860-876` (one table; frames render as object records)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs:203-276`, `:321-364`; `crates/ns-runtime/tests/image.rs` (every `graph_counts(writer, objects, frames)` → one count; the frame-table tests as listed in Step 1); `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)
- Test: `tests/image.rs`, `tests/opcode_tags.rs`

**Interfaces:**
- Produces: the wire layout below; `DecodeError::FrameRecordUnderAnotherClass` replaces `ActivationHeader` for the frame record; `DecodeError::BadFrameId` goes (a frame is an object id: `BadObjectId`), and a frame reference naming an object that is not a frame record is `DecodeError::NotAFrameReference(u32)`.

**The wire layout.** The graph section is `u32 object count`, the object records, the roots. An `OBJECT_FRAME` record's header class tag is `Activation`'s builtin tag (23) and it is the only body that may carry it; its body is, in order: the kind tag and payload as `encode_frame` writes them today; `u32 program counter`; `bool uncontinuable`; the receiver (a value); the sender and the home (each `bool` + `u32` object id); the current class (`bool` + class tag); `u32 register count`, then that many values. The kind comes first so that the decoder's first pass can size the record: the register count is on the wire, so the first pass needs no body lookup. The decoder refuses a register count other than the body's `register_count` in `verify_graph` (`FrameRegisterFile`, now without the empty-file exemption), and a method frame's selector as today (`UnknownSelector`). Every reified frame is written with its full register file, `nil` where nothing was copied.

**The two passes.** Pass 1 reads every record's header and body without storing anything -- values are parsed and discarded, ids range-checked against the object count -- and records each record's word count; a count that would allocate more words than `8 *` the bytes remaining refuses with `UnexpectedEndOfFile` before anything is allocated. Then it allocates every object at its size (`allocate_placeholder` with the size, a new `pub(crate)` parameter) and pass 2 reads the records again and fills them. Frames are filled after `rebuild_applications`, as today, since `FRAME_METHOD` resolves a selector.

- [ ] **Step 1: Write and rewrite the tests.**
  - `opcode_tags.rs`: pin `OBJECT_FRAME == 10`, `RETIRED_OBJECT_BODIES == [4, 5, 9]`, `OBJECT_FRAME` in the live list and the no-sharing family.
  - `image.rs`: `graph_counts` takes one count; rewrite `one_frame_prefix`, `closure_over_frame`, `foo_frame*`, `activation_record` to the new body; `a_frame_id_out_of_range_is_a_decode_error_not_a_panic` → an object id 99 → `BadObjectId(99)`; `a_huge_object_or_frame_count_does_not_allocate_gigabytes` → the one count; `a_frame_with_no_registers_at_all_verifies` → `a_frame_with_no_registers_for_a_body_that_has_some_is_refused` (`FrameRegisterFile`); the five `OBJECT_ACTIVATION` tests → `OBJECT_FRAME` (a record under tag 9 is refused as `InvalidTag { what: "object body" }`; a frame record under another header is `FrameRecordUnderAnotherClass`; `Activation`'s tag on another body stays refused); `a_closure_capturing_a_block_activation_round_trips` asserts the captured frame now carries its full register file; `the_graph_walk_consumes_the_section_exactly` keeps its frame case.
  - New: `a_frame_reference_naming_a_string_is_refused` (`NotAFrameReference`); `a_frame_record_whose_register_count_would_exhaust_the_bytes_is_refused_before_allocating` (`u32::MAX` registers → `UnexpectedEndOfFile`).
- [ ] **Step 2: Run them to verify they fail.**
- [ ] **Step 3: Implement** the encoder, the decoder's two passes and `inspect`'s walk. `encode::unnameable_class`'s doc and the `Activation` header rule move to `OBJECT_FRAME`.
- [ ] **Step 4: The amplification.** With one table and full-size placeholders, the peak of a rejected load is bounded by the first pass's size check: compute the worst case per record kind against its smallest record (a frame object: 16-byte slot + 8 × (2 + 10) bytes against its minimum record; a class object: 16 + 8 × 10 against the 9-byte metaclass record; a value: an 8-byte word against a 1-byte `VALUE_NIL`) and the per-value ratios of `image-format.md`'s planned section, take the largest, and record it -- release and debug -- in the comments at `decode.rs:2444-2478` (rewritten), for Task 15 to carry into `CLAUDE.md` and `memory.md`. Write a test `the_first_pass_refuses_a_record_count_the_bytes_cannot_hold` that pins the bound's premise: an image whose declared sizes exceed `8 *` its remaining bytes is refused without allocating (assert `heap_statistics().live_objects` did not grow).
- [ ] **Step 5: Regenerate the golden fixture** (`cargo nextest run -p ns-runtime regenerate_the_golden_fixture --run-ignored only`), and check with `ns-next inspect` that it decodes.
- [ ] **Step 6: Run the tests, then the per-commit gate and `cargo nextest run --workspace --release`.**
- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat: frames are object records on the wire, and the decoder sizes every record before it allocates"
```

---

### Task 14: Measured against the tree the branch started from

**Files:** none in the repository until Task 15 records the numbers.

- [ ] **Step 1:** Follow the `measuring-performance` skill and `execution.md`'s protocol ("What the eight-byte value moved", "The Are We Fast Yet benchmarks"): three pinned release builds of `master` at the branch point and of the branch head, the same explicit `RUSTFLAGS`, the sixteen benchmarks and the microbenchmarks under `goldens/tool/bench/` (the four that are not in the tree written out from `execution.md`, "What spec-conformant lookup moved"), pooled.
- [ ] **Step 2:** Record, per program, the change and its spread; profile the two largest regressions and name where the time went (the guard read through the block's pointer, the code-id probe per `MakeClosure` and `reify`, the symbol lookup by text). This is the interim measurement the design expects to regress (`object-model.md`, "Planned: the object layout"); it decides nothing, and anything it shows worth fixing before 3b goes into `docs/OPEN-WORK.md`.

---

### Task 15: The documentation describes the built state

**Files:**
- Modify: `docs/design/object-model.md` (the object layout from "Planned" to built for 3a's part, in the present tense, with `Format::Class`'s reason; the class-object, closure, frame and refusal paragraphs; "Decisions"), `docs/design/memory.md` (the frame arena's end, trace by format, the one arena, the amplification breakdown), `docs/design/image-format.md` (`OBJECT_FRAME`, the two passes, the 22-bit hash, the clause limit, the amplification), `docs/design/dispatch.md` (the guard in the header word), `docs/design/core-library.md` (`Symbol` under `String`, `String new`, `Symbol new`), `docs/design/execution.md` (Task 14's numbers as "What the object representation moved"; "Activations" for the frame object), `docs/design/README.md` (the map's status cells)
- Modify: `CLAUDE.md` (the amplification figures; the handle paragraphs -- no frame arena; "A cache records what the lookup decided" reads `ClassData.builtin` as a class-object field; "A running activation is an `ActivationReference`" unchanged in substance)
- Modify: `docs/OPEN-WORK.md` (remove: "`Symbol new` and `String new` answer an instance with no text", the `LargeInteger` copy item if Task 7 lent the digits everywhere, the register-less pairing half; keep: the ID reuse and weak cache under live editing, the recorded guard under step 5; add what Task 14 leaves open; "Next" names road step 3b), `docs/PROGRESS.md` (the milestone, "Where the next session starts": road step 3b)

- [ ] **Step 1:** Rewrite each section named above; every statement in the present tense, no "planned" for what is built.
- [ ] **Step 2:** Check `git diff --stat` is Markdown only, then commit (no gates for a Markdown-only commit):

```bash
git add docs CLAUDE.md
git commit -m "docs: the object representation is built, and road step 3b is next"
```

---

### Task 16: The end-of-batch gates

- [ ] **Step 1:** Run the `finishing-a-batch` skill: the four stress modes (moves, caches, collection, every instruction wide), `run-corpus` plain and under stress, the benchmarks profile, `cargo nextest run -p ns-lsp`, `verifyPlugin`, and the check that a fresh session can start from the documentation alone.
- [ ] **Step 2:** Fix what they find, each fix its own commit through the per-commit gate.
- [ ] **Step 3:** Request the final whole-branch review (on Fable, per the user's standing instruction) before merging into `master`.
