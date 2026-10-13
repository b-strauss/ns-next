# Memory — object lifetime, identity, collection

> **Status:** built. Objects live end to end in one object space owned by
> one `Heap`, in memory the heap reserves and commits through
> `crates/ns-memory` ("The object space"), each object its header word,
> class word and fields in road step 3's layout
> ([object-model.md](object-model.md), "The object layout"), addressed by
> handles that are 64-bit byte offsets; activations live on an activation
> stack in the same `Heap` and become frame objects in the space only when
> something reifies one.
> Identity, layout ownership and the seam that routes every access through
> `Heap` are built. The space has two generations ("Generations"): a
> nursery of two survivor spaces and eden below the old space, collected
> by a copying scavenger with age-based tenuring, whose roots include a
> remembered set that the write barrier in `heap.rs`'s store accessors
> keeps. A full collection reclaims an object of either generation once
> nothing reaches it: marking walks the roots through `trace.rs`'s one edge
> enumeration and sets each reached object's mark bit in its header,
> compaction slides the survivors of both generations into the old space
> from its first offset and gives committed memory back with hysteresis,
> and every handle is rewritten through a live-word bitmap, in one
> stop-the-world pass. Both run only at a safepoint of the run loop, on a
> request an allocation sets -- a scavenge once eden holds its threshold, a
> full collection once the old space reaches its limit -- or between
> entries, never inside an allocation itself. The root
> discipline it runs on, in "Checked roots and a collector", is built in all
> three of its parts: nothing re-enters the run loop from Rust, so no Rust
> frame is live below a safepoint (step 2a); the run loop polls a safepoint,
> every handle the collector traces is enumerated, a host keeps values
> across runs only as `Root`s, and a debug-build stress mode moves
> everything at the safepoints to check all of it (step 2b); and every
> movable handle carries a lifetime brand, so that holding one across a
> safepoint is a compile error (step 2c). The canonical symbol table is
> weak; everything else the collector traces is strong.
> **Planned:** allocation by a bump in eden inlined into the instructions
> that allocate ("Planned: inline allocation").
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the
> handle types and their brand, every accessor and allocation, the write
> barrier and the remembered set, the host root table, the move, the
> triggers, the scavenge, marking, compaction and forwarding),
> `heap/space.rs` (the object space: the reservation, its four regions,
> the bumps, the overflow word and the walk), `heap/scavenge.rs` (the
> scavenger and the tenuring threshold), `heap/compaction.rs` (the
> live-word bitmap a collection forwards through, and the move's shift),
> `heap/object.rs` (the block of words an object is, `pointer_ranges`,
> which names its pointer words by its format, and `trace_block`),
> `heap/header.rs` (the header word),
> `heap/records.rs` (the seam's value types), `heap/tables.rs` (the per-heap
> tables of mixins, applications and code),
> `heap/mark.rs` (the marker's work list), `heap/world.rs` (the step
> state at rest, the entry that lends it out, `StepRoots`, `World::collect`
> and `World::scavenge`),
> `interpreter/mod.rs` (`Mutator`, and `Interpreter::enter` with the
> doctests that pin the brand), `trace.rs` (every root the collector
> traces, marking, forwarding and scavenging alike, in every build),
> `interpreter/safepoint.rs`
> (the safepoint, `Interpreter::tenure_now`, `Interpreter::collect_now` and
> the switches of the five stress modes: moves, caches, collection,
> scavenges, and every instruction wide),
> `vm.rs` (one entry per stretch between two safepoints, and the poll),
> `heap/value.rs` (`Value`, and `Word`, what a heap object stores),
> `mixin.rs` (layout, and the one remaining iterative
> `Drop`), and the memory layer's `crates/ns-memory/src/lib.rs`,
> `page_map.rs` and `platform/{mod,windows,posix}.rs`.

## What exists

**The class owns the slot layout**, and slot access resolves through the
*current* layout, so a layout change plus instance migration is expressible
rather than something the representation forecloses. Layout lives on
`MixinApplication`; see [object-model.md](object-model.md).

**Identity is stable and hashed from a counter.** Every object's identity
hash, 22 bits of its header word, is drawn from a counter the first time it
is asked for, never derived from its address; the counter wraps within the
22 bits and skips 0 ([object-model.md](object-model.md), "The identity hash
is 22 bits wide"). This is required by identity-keyed
collections, by image roots, and by `become:`; an address-derived hash would
break the moment an object moved or an identity was swapped.

**Objects live in one object space and activations on a stack, not in
`Rc<RefCell<_>>`.** One `Heap` owns `space: Space`, a reservation of
`ns-memory` in which every object lies end to end as its words, and the
activation stack every running activation lives on. A *reified*
activation -- one a block captured, the exception machinery named or the
image decoder rebuilt -- is an object of the frame format in the same
space. An `ObjectReference` or a `FrameReference` is the byte offset of
the object's header from the space's base, an `ActivationReference` an
index into the stack. The representation is described below, and the stack in
[execution.md](execution.md), "Activations". An object is reclaimed once the
collector's mark pass cannot reach it, so a cycle is no longer the special
case that leaks: the tracer follows every field, not a reference count, and a
cycle nothing outside it reaches goes with the rest of the garbage at the next
collection ("The shape: mark, compact, forward" below). The stack needs no
collector for the same property: an entry is popped when its activation
returns, and one that was never reified leaves nothing behind. That is not a
problem the image format inherits: the format is defined against the
*logical* value model rather than against this representation, so replacing
it again would not break images.

### The representation: one space of objects and typed handles

The model began as the Nova JavaScript engine's: a value carries either its
data inline or a handle the heap resolves, with no `Rc` and no `RefCell`
anywhere in it. Nova's `Value` is a Rust enum and its handle an index into
a typed arena; ours is one tagged word, Spur's, which holds either
([object-model.md](object-model.md), "Values and objects"), and the handle
is a byte offset into one space of objects, as the address-based engines
lay out their heaps ("The object space"). Nova is the useful comparison
rather than V8 because it solves this problem in Rust, where the borrow
checker is available as an enforcement mechanism and a C++ engine's
conventions are not.

```rust
Value  = one tagged word: a handle, a SmallInteger, a SmallFloat64 or a constant   // Copy
Word   = the bare word a heap object stores, 8 bytes in every build
ObjectReference     { offset: NonZeroU64 }   // private to the heap module; 8 bytes in a release build
FrameReference      { offset: NonZeroU64 }   // likewise, a frame object's; ClassReference is an alias for ObjectReference
ActivationReference { index: u32 }           // likewise, into the activation stack

Heap { space: Space,                  // every object end to end: header, class word, fields
       stack: ActivationStack, tables: Tables,   // mixins, applications, code
       next_identity_hash: u32 }
```

What that removed is not an abstract elegance but part of the measured cost of
the previous model, itemized in [execution.md](execution.md)'s measurements
section: the `Rc` box and its reference-count traffic on every frame link and
every shared handle, and a `RefCell` borrow on every single register read and
write. A register access is now a slice index into an owned `Vec`. Over
`bench_loop.ns` on a release build that measured as **19.4–19.6 ms before,
16.4–16.8 ms after** — about 15%. The rest of the cost of an activation nothing
captures — a `Vec` apiece for its arguments and temporaries, and a heap entry
— went with the activation stack. A reified activation takes one object in
the space, its frame object, allocated with room for its whole register file, into
which it copies its registers on return — which, since closure conversion,
only an activation whose block can still reach them by reference does. [execution.md](execution.md) has both:
"Lifecycle" for when the registers are copied, "What the activation stack
moved" for the figures.

`ObjectReference`, `FrameReference`, `ActivationReference` and `Value` are `Copy`. A handle has no
destructor and nothing to clone, which is why `Value` could become `Copy` at all,
and it is what makes a handle cheap to hand around.

Three properties follow from an offset that did not follow from a pointer.

**An object can name itself in safe Rust.** Building an object that names
itself — a bootstrap class whose own class is itself — used to need
`Rc::new_uninit` and `Rc::from_raw`, which were the crate's `unsafe`. `Heap`
allocates the object, then writes its class word with its own offset, in
safe Rust (`Heap::allocate_self_referential_class`). The crate forbids
`unsafe` code outright (`#![forbid(unsafe_code)]` in `lib.rs`; "The brand"
below says why it needs none); the space's own `unsafe` is in
`crates/ns-memory` ("The memory layer").

**There is no reference chain to unwind.** See "Iterative teardown" below.

**A handle is only meaningful against the heap that minted it.** See "A handle
is space-relative" below.

The image format writes ids, not offsets, so where an object sits never
reaches an artifact: images are architecture-neutral by construction, which
is requirement 4 in [liveness-and-reflection.md](liveness-and-reflection.md),
and identity hashes come from a counter rather than an address, so nothing
about identity depends on where an object sits.

### A handle is space-relative

An offset is meaningful only against the `Heap` whose space it was minted
in: not globally unique, space-relative.

Reading a handle through the wrong heap is silent — an offset minted by
another heap may lie inside this heap's space and name a different,
perfectly valid object, so the read answers the wrong thing rather than
failing. A `debug_assertions`-only guard makes that explicit rather
than conventional: each `Heap` carries a distinct id, each handle records the
id of the heap that minted it, and every accessor in `heap.rs` that takes a
handle asserts the two match. The check of an `ActivationReference` also
compares a generation, because a stack index is reused as soon as its entry
is popped. An `ObjectReference` or a `FrameReference` also carries the heap's
move epoch, which the same check compares ("The move stress mode" below). In
a release build neither the fields nor the checks exist, and a handle is
exactly its 64-bit offset, a `NonZeroU64`, so an `Option` of a handle is one
word too (`a_handle_and_an_optional_handle_are_one_word`, `heap.rs`, a
release-build test). The offset is never 0: the first object starts at
byte 16 ("The object space").

The room the actor milestone needs is still there. Actors are designed to get one
heap each so that pauses stay local
([liveness-and-reflection.md](liveness-and-reflection.md)), which will ask how a
handle encodes *which* heap. A handle's field is private to the `heap`
module, so making it carry a heap tag is a change to one module — and the
debug guard is already the place that knows a handle has an origin at all.

One deliberate escape exists, and it is opaque rather than a hole.
`Heap::object_identity_key` is `pub(crate)` and returns the offset as a
`usize` for the places that need a handle as a `HashMap`/`HashSet` key rather
than a comparison: `image/encode.rs`, `image/decode.rs`,
`interpreter/shipped_world.rs` (the shipped world's index by object) and
the derived equality walk of `builtins/value.rs`. A frame is an object, so a frame's key is its frame
object's. Every one stores it, compares it, or looks it up; none does
arithmetic on it, and none assumes it is an offset into anything. So the
property that matters survives — a handle's *encoding* is still `heap.rs`'s
alone to change — but the narrower claim that nothing outside the module ever
sees the bits is not true, and these are where to look first if the encoding
changes.

It sits on `Heap`, not on the handle, so that the origin check applies to them
like it does to every accessor. Handed a handle and no heap, the operation has
nothing to check against, and two handles from different heaps then produce the
same key and collide silently in a caller's map. Folding the debug-only `origin`
field into the key instead was tried and reverted: that makes the collision
impossible in exactly the build that would have reported it and leaves it
exactly as possible in the build that ships.

### Iterative teardown

Rust has no iterative drop glue, so releasing a deep reference graph recurses and
a sufficiently deep one aborts the process. Since an image is untrusted input and
its graph depth is attacker-controlled, that is not acceptable.

Objects and activations have no such graph: an object is words in the
space, a word holds an offset, and dropping the `Heap` releases the space's
reservation in one call and drops the activation stack's two flat vectors.
The hand-written iterative `Drop` implementations they used to need are
gone.

**`MixinApplication` still owns a real chain** — `superclass:
Option<Rc<MixinApplication>>` — and keeps its hand-written iterative `Drop`
(`crates/ns-runtime/src/mixin.rs`). It is the only *iterative* `Drop` left in
the crate, which is not the same as the only recursive graph: `Mixin`'s
`nested: Vec<Rc<Mixin>>` and `class_side: Option<Rc<Mixin>>` links drop
recursively too.

The alternative — a depth ceiling — stays rejected **for the application
chain**, because that depth is genuinely unbounded: an application chain is as
long as the class chain it mirrors, `image::decode::rebuild_applications` builds
one link per class record, and a Newspeak program can legitimately declare a
deep inheritance chain. Refusing one would exclude a program the runtime already
runs from source.

A ceiling is the right answer for the `Mixin` tree, and `image/mixins.rs` uses
one (`MAX_MIXIN_DEPTH`). The difference is where the bound comes from: nesting
depth is already bounded by `ns-syntax`'s parser, so the ceiling is derived from
what the front end accepts rather than chosen, and it excludes nothing the
runtime would otherwise run. That file names the three walks it protects, of
which `Rc<Mixin>`'s own drop glue is one. A ceiling is rejected where it would
refuse valid programs, not on principle.

The same reasoning applies to traversal rather than teardown: the image encoder
walks the graph over an explicit stack, because an image controls its
depth.

### The seam

The `heap` module -- `crates/ns-runtime/src/heap.rs` and its submodules,
`heap/value.rs` among them, which puts a handle's offset into a value's word and
takes it out again -- is the only place that knows what a handle is and what
an object's words hold. Every read and write of an object, a frame or a
running activation goes through a `Heap` accessor, so replacing the store
behind them touches that module rather than the 193 call sites (109 under
`src`, 84 in the integration tests) the workspace held before the seam
existed. The replacements so far are the evidence that this works.
Replacing `Rc<RefCell<_>>` with an arena of 32-bit indices changed **no
call site outside `heap.rs` and `types.rs`**, for either the frame
conversion or the object one. Road step 3a first narrowed the seam
so that nothing outside the `heap` module names a payload type, then turned
every object kind into a block of words behind it. Road step 3b replaced
that arena with the object space and the index with a byte offset; outside
the `heap` module one call site changed, the decoder's, because
`fill_placeholder_class` now answers whether the placeholder has a class
object's size -- an object in the space cannot be resized, so the decoder
refuses the image instead. Callers build objects
through typed allocations (`allocate_instance`, `allocate_class`,
`allocate_closure`, `reify`, the decoder's `allocate_placeholder` and its
in-place `fill_*`) and the value types of `heap/records.rs` (`NewClass`,
`NewClosure`, `FrameRecord`, `ObjectShape`, `PlaceholderFormat`), which hold
values and handles and never the words an object is stored as.

What the seam hides is the store behind a handle, not the handle types. Those
are part of every signature on purpose: a function that means *the running
activation* takes an `ActivationReference`, and anything that stores one takes a
`FrameReference`, which is what makes storing a stack handle a compile error
([execution.md](execution.md), "Representation"). Changing a handle type is
therefore a change to its callers; changing what stands behind one is not.

The accessors copy out rather than lend out. That follows what the call
sites already did — borrow, clone one field, drop the borrow — and it is
what keeps a rooting discipline from being needed at every site that calls
back into the interpreter while holding a reference.

**The seam is compiler-enforced.** `ObjectReference`, `FrameReference`,
`ActivationReference` and the `ClassReference` alias are declared in `heap.rs`
with a **private** inner field, so reading a handle's offset from anywhere else
— any other module of `ns-runtime` included — fails to compile. There is no
grep gate to remember and no visibility to widen: an accessor that is missing
has to be added in `heap.rs`, which is where the store is. The one way out is
the pair of `pub(crate)` identity-key accessors described above, which hand
the offset over as an opaque key and are used as one — a named exception rather
than an unenforced boundary, and one the origin guard still covers, since they
are accessors on `Heap` like any other.

Where a site needs several fields of one object, frame or activation at once, a
batched accessor answers them together rather than the caller taking several —
`activation_receiver_and_current_class`, `frame_register_shape` and
`frame_record` are the present examples. This is
not only style: the lexical-send head (`sends::decide_lexical_send` today), on
every implicit-receiver send, once took one `RefCell` borrow yielding both the
receiver and the current class;
splitting that into two separate accessor calls measured as a real cost,
recovered by batching them back into one (see [execution.md](execution.md)'s
measurements section). On the activation stack those are two field reads of one
entry rather than two borrows, so the cost batching removes is smaller than it
was; whether it still pays is a measurement nobody has taken.

**An accessor is named noun first.** One that reads or writes the state behind
a handle starts with what it reads through: the handle's kind -- `object_`,
`class_`, `frame_`, `activation_` -- or, for state only one kind of object
carries, that kind -- `array_`, `instance_`, `closure_`, `string_`,
`activation_object_`. Then comes what it reads (`object_class`,
`array_element`, `array_element_count`, `instance_field_count`, `frame_home`),
or, for a write, its verb -- `set_` unless the write does more than set -- and
what it writes (`object_set_class`, `instance_set_field`,
`class_set_application`, `frame_set_program_counter`,
`activation_set_program_counter`, `class_clear_superclass`,
`object_ensure_identity_hash`). Three kinds of name are not
accessors of one handle's state and keep their own shape: a predicate on what
an object is (`is_class`, `is_closure`, `is_string`, ...), a conversion of a
payload to its Rust value (`str_value`, `symbol_value`, `float_value`,
`large_integer`), and an operation named by its verb -- the allocators, the
stack's (`push_activation`, `pop_activation`, `top_activation`, `reify`,
`retain_registers`), `fill_copied_values`, `stamp_dispatch_ids`, the
decoder's placeholder fills (`shape_placeholder`, `fill_placeholder_field`,
`fill_placeholder_string`, `fill_placeholder_closure`,
`fill_placeholder_class`, `fill_frame_fields`, `fill_frame_register`) and the
collector's. `with_class_declared_slots` and `with_large_integer` lend a
slice to a closure rather than copying it out, and their `with_` says so. A register is read and written
through a `RegisterBase`, not a handle, so the register is itself the noun:
`register` and `set_register`. A free function that takes the `Heap` or the
`Mutator` takes it as its first parameter.

### Known rough edges in the seam

Carried deliberately rather than fixed. Recorded so that whoever reshapes this
file next starts from a list rather than from rediscovery.

- **Copy-out costs, and the accessor that could now avoid them.**
  `builtins/text.rs`'s string dispatch no longer clones the payload: its
  accessors -- `string_size`, `string_code_point_at`, `string_slice`,
  `string_concatenation`, `strings_equal` and `string_hash` -- copy out a
  size, a code point, a slice, a `bool` or a hash, the work done inside
  `heap.rs`, and `str_value` stays as the UTF-8 boundary, an owned
  `String` for the transcript and the error texts. Measured on 2026-10-01
  (release, two pinned builds per side): 25,000 `size` sends over a
  25,000-character string went from 22.3 ms to 2.4 ms, and `size` with
  `at:` from 40 ms to 3 ms. A string's units are read in place out of its
  words, and a `LargeInteger`'s digits are lent in place to `=`, `hash`, the
  comparisons and the sign tests (`with_large_integer`); its arithmetic
  still copies both operands' digits out (`large_integer`). The encoder
  reads each record through the copying accessors (`instance_fields`,
  `frame_record`, ...), and converts each `String` to UTF-8 once. Copy-out
  is a property of an accessor's *return type*, not of the store, and the
  object space changed no accessor's: what the store decides is whether a
  *borrowing* accessor is expressible at all. Under `Rc<RefCell<_>>` one
  would have had to hand back a `Ref` guard, which is unusable at a site
  that calls back into the interpreter; since the arena, and in the space
  today, an accessor can lend out a slice of an object's words tied to the
  `&Heap` borrow, and the borrow checker enforces the rest.
  Whether to lend the encoder its
  records, or `LargeInteger` arithmetic its operands, is a measurement
  ([OPEN-WORK.md](../OPEN-WORK.md)); the encoder is not on a measured path.

## Checked roots and a collector

**Complete:** the safepoint, the tracing, host roots, the stress mode, the
lifetime brand and the collector are all built. A collection can run — only
at a safepoint in the run loop — and moves objects. Every handle the runtime
holds is already either where the collector finds and rewrites it, which the
stress mode checks, or a compile error, which the brand makes it. Designed on
2026-09-13 as step 2 of Milestone 4, in three
parts that each landed green and measured on their own: **2a** removed every
re-entry of the run loop from Rust ([execution.md](execution.md), "No re-entry
from Rust"); **2b** is the safepoint, the enumeration of everything the
collector traces, host roots, and a debug-build stress mode that moves every
object at the safepoints and so checks the enumeration against the whole
suite; **2c** brands every movable handle with a lifetime, so that holding one
across a safepoint is a compile error. The collector itself is step 5,
complete since 2026-09-20: a release build's trigger requests a collection
once the bytes the objects occupy reach its limit, served at the run loop's
next safepoint ("The trigger and the growth policy" below).

**The collector has to move objects.** `become:`-style identity forwarding is
a requirement, not a convenience: adding an instance variable to a
class with live instances allocates new-shape instances, copies fields, and
atomically forwards the old identities to them. That rules out a conservative non-moving collector,
which makes `become:` hard. A copying or compacting collector can implement it,
and MMTk supports object forwarding.

A moving collector must not only *find* every root but *update* it, and roots
held in Rust locals across a collection are the hard case: under `Rc` those
references kept their objects alive and never needed updating, and under a
moving collector they are roots the collector cannot see and handles it cannot
rewrite. The design answers that in three layers: where a collection may
happen, what may be on the Rust stack when it does, and what the compiler
checks.

### Where a collection can happen: only at a safepoint in the run loop

An allocation never collects. Past a threshold it *requests* a collection
instead — "The trigger and the growth policy" below — which runs at the next
safepoint of the run loop. `vm::run` polls one at every switch
to another activation — a push, or a caller resumed — and at every backward
jump, so every call and every loop iteration passes one, a loop that makes no
send included; a forward jump does not poll. Across a safepoint `run`
holds `FrameRegisters` — the running `ActivationReference`, its
`Rc<CompiledCode>`, its `program_counter`, its `FeedbackBase` and its
`RegisterBase` — and no handle the collector would move: each stretch of
instructions between two safepoints is an entry of its own, and the
safepoint is outside every one, so the compiler refuses anything else ("The
brand" below); every live value is in a register
on the activation stack or in one of the places "What the collector traces"
below lists. The step state's reusable argument buffer is empty there — a
send gathers into it, decides and pushes within one instruction, and it is
cleared when it is given back — so it is not a root, and a debug assertion
before every move, collection and scavenge says so
(`World::check_quiescent_state`).

The one other place a collection is sound is between entries, where only
roots hold values and no step is running: `Interpreter::collect_now`
(`interpreter/safepoint.rs`) collects there, for tests only, and every
image load tenures everything there once, between the shipped-name section
and the graph pass (`Interpreter::tenure_now`; "Generations").

The first four systems below chose this placement; the last four collect
elsewhere, and their right-hand column is what that costs their runtime code:

| System | Where a collection can happen | What its runtime code does about the handles it holds |
|---|---|---|
| Cog/Spur | Allocation past `scavengeThreshold` only sets `needGCFlag` and forces an interrupt check, and goes on into eden's reserve; the scavenge runs at the next interrupt check, a send or a backward jump. `forceInterruptCheck` sets `stackLimit` to all ones, so the stack check every frame build already makes is the poll (`src/spur64.cog/cointerp.c`). | Little: no primitive calls back into Smalltalk. |
| Nova | `Vm::inner_execute` checks `agent.check_gc()` at the top of the instruction loop (`nova_vm/src/engine/bytecode/vm.rs`); an allocation only adds to `alloc_counter`. | `GcScope`/`NoGcScope` lifetimes and `Scoped` roots, because nearly every builtin can call JavaScript (`GARBAGE_COLLECTOR.md`). |
| CPython ≥ 3.12 (not in `prior-art/`) | Moved its cycle collector from allocation to the eval breaker (gh-97922), because collecting at an allocation ran it where the VM was inconsistent. | Reference counts. |
| piccolo on gc-arena (not in `prior-art/`) | Only outside `Arena::mutate`. The whole VM is stackless — a Rust callback returns a `Sequence` rather than calling Lua — so control returns outside `mutate` between steps. | Nothing to root: a generative `'gc` lifetime makes a pointer that outlives `mutate` a compile error. |
| V8 (not in `prior-art/`) | Any allocation. | `Handle<T>` through a `HandleScope`, checked by the `gcmole` static analysis and by debug-only `DisallowGarbageCollection` scopes; now moving to direct handles, which require conservative stack scanning. |
| HotSpot (not in `prior-art/`) | Safepoint polls between bytecodes, at compiled code's method entry and exit and non-counted back-edges, and allocation slow paths — precise because the JIT emits an oop map at each. | The VM's own C++ holds oops in `Handle`s under a `HandleMark`, with a debug-only `NoSafepointVerifier`. |
| Primordial Soup | Any allocation: `Heap::AllocateNormal` scavenges when new space is full (`vm/heap.cc`). | An eight-slot `HandleScope` (`vm/heap.h`), by convention; no primitive calls back into Newspeak. |
| SOMns | The JVM's safepoints. | Nothing: Java references are precise through the JIT's stack maps. |

Allocation as a collection point is rejected. Every allocating Rust path — each
native, the compiler, the image decoder with its maps of placeholders — would
have to root what it holds across every allocation, which is the burden V8 and
HotSpot's runtime carry with handles and verifiers; and a JIT would need a
stack map at every inline allocation and could not keep a handle in a register
across one. HotSpot affords it for Java code only because its JIT emits the
maps, and ns-next's Rust code has none. What the run-loop placement costs is
overshoot: a request made during a step waits for the next safepoint, so the
heap can pass its threshold by what one step allocates. A large object going
straight to the old space and eden's reserve past its scavenge threshold
absorb that, as they do in Cog and Primordial Soup, and an allocation that
finds even the reserve spent goes old ("Generations").

**The poll** (in `vm::run_stretch`, forced inline into `vm::run`) is one
compare of a flag the heap owns, `Heap::safepoint_requested`. When the flag is
set the stretch writes its frame back and leaves its entry, and `run` calls
`Interpreter::safepoint` (`interpreter/safepoint.rs`), `#[cold]` and out of
line, so `vm.rs` holds the compare and the call and nothing of what a
safepoint does. The safepoint serves a request -- a full collection if one
is pending, which empties the nursery too, and otherwise a pending scavenge
-- and clears the flag, except under a stress mode below, which keeps it
set. The collector's triggers set it in a release build too:
`allocate_block`, through which `reify` allocates its frame object as every
allocation does, compares eden's occupied bytes against the scavenge
threshold, and the old space's against the collection limit when an
allocation goes old, a comparison and not a counter, and requests a
scavenge or a collection when one is reached ("The trigger and the growth
policy" below).

That the slow path serves the request, which a release build's trigger now
also raises, is what keeps the poll in the binary. With an empty release slow
path, fat LTO proved the call free of side effects and deleted it — then the
branch, then the flag's load — so the release binary carried no poll at all,
and the first measurement of it measured nothing. Clearing the flag leaves a
compare-and-branch at both poll sites and a cold call in `vm::run`'s release
assembly. The collector's trigger relies on the same thing: a measurement of
the poll means something only on a binary whose assembly still contains it,
so the price it shows is the price the trigger pays rather than one the
optimizer hid.

Measured with the poll in the binary, on a release build in one sitting
against its parent, no source was slower: from −3.0% on `bench_block.ns` to
+0.9% on `bench_new2.ns`, every group overlapping its parent's. Measured with
the rest of step 2b against the commit 2b started from, in two sittings, no
source was slower beyond the sittings' noise, and `bench_block.ns` was 4%
faster for another reason ([execution.md](execution.md), "What 2b moved",
which also has the one borderline group). So Cog's trick was not
needed; it stays the fallback should a later measurement show the poll's price
on the send path: a request lowers the limit `Mutator::push` already
compares the activation depth against, so a send pays nothing it does not pay
anyway.

### No Rust frame is live below a safepoint

Rust enters the run loop only as the host, its outermost caller
(`Interpreter::run_from_host`). `do:`, instantiation and the nested-class
accessor, which once ran Newspeak code in nested runs of the loop, are
trampoline steps with continuations, as `whileTrue:` is (step 2a, built;
[execution.md](execution.md), "No re-entry from Rust"). A Rust frame below a
nested run would hold values across that run's safepoints, and rooting them is
what Nova's `Scoped` values are for; ns-next removed the frames instead. So at
every point between two instructions the Rust stack holds the host and the run
loop and nothing between them — piccolo's property — and what is left to root
is only what the host keeps ("Host roots" below). The safepoint relies on it.

### The brand, checked by the compiler rather than by convention

Holding a handle across a safepoint, and a native re-entering the run loop,
are compile errors rather than mistakes the stress mode finds at run time
(step 2c). Every handle a collector can move — `ObjectReference`,
`ClassReference`, `FrameReference`, and so `Value` and every type that
contains one (`NewClass`, `NewClosure`, `FrameRecord`, `ValueClassRefusal`,
`FrameKind`, `Continuation` and the state its variants carry, `Step`,
`Signal`, `NewspeakError`, `Heap` itself) — carries a lifetime `'gc`,
invariant (`heap.rs`'s `Brand<'gc>`, a zero-sized
`PhantomData<fn(&'gc ()) -> &'gc ()>`) so that it can be neither shortened nor
extended. A release handle is still exactly its 64-bit offset.

**The step state is a `Mutator<'gc>`** (`interpreter/mod.rs`): the heap, the
builtin classes, the application caches, the argument buffer and the spare
instantiation boxes, and every method a step uses — heap access, allocation,
the send decisions and pushes, the natives, the compiler, the image codec. At
rest it is a `Mutator<'static>` held by a `World` (`heap/world.rs`), which the
`Interpreter` owns beside what only the run loop needs between steps. The only
way to reach it is `Interpreter::enter`, which calls `World::enter`: that runs
a closure `for<'gc> FnOnce(&mut Mutator<'gc>) -> R` — gc-arena's `mutate` —
and hands it the at-rest state as it is, `body(&mut self.mutator)`, with no
cast and no `unsafe`. The call instantiates `'gc` as `'static`, and the
closure cannot tell: the compiler checks it once for every `'gc`, so it can
neither assume that `'gc` is `'static` nor let a branded value out, since `R`
is chosen outside the `for<'gc>` and cannot name it. The crate forbids
`unsafe` code (`#![forbid(unsafe_code)]`, `lib.rs`), and the brand needs none.

**Nothing branded leaves an entry, and nothing in an entry reaches the loop.**
`Mutator` offers no way back to the `Interpreter`, so no step — no decision,
native or continuation, each of which is handed only a `&mut Mutator` — can
run `vm::run`, reach the safepoint or start a host's send. The loop makes one
entry per stretch between two safepoints, and the safepoint is outside every
one; what the loop carries from one stretch to the next names no brand —
`FrameRegisters`: the running `ActivationReference`, its code, its
`program_counter`, its `FeedbackBase` and its `RegisterBase` — and a run ends
inside its last entry, its answer rooted for the host (`vm::run`, and
[execution.md](execution.md), "The VM" and "The loop over locals"). A host
makes one entry per read. `World::enter` is `#[inline(always)]`, and a
lifetime is gone before code is generated.

**Only the `heap` module changes a brand.** A host root's slot is shared with
a host outside every entry, so it holds a `Value<'static>`, and `Heap::stash`
passes a value through `at_rest` on the way in, `fetch` through `branded` on
the way out — two functions of `heap/value.rs`, `pub(super)` so that only
`heap` reaches them, each monomorphic in its direction. The split does not
make rebranding impossible inside `heap`: composing the two,
`branded(at_rest(value))`, carries a value from one brand to another. What it
guarantees is that no single call does, so a misuse there has to spell out
both calls and pass visibly through `'static` at the call site. They are the
only conversions of a brand, and there is **no public `unbind`** — which is
where Nova's discipline leaves a hole that its own documentation closes by
convention ("NEVER unbind a `Value` into a local variable"). A stored `Word`
carries no brand either, so `Heap::load` and `Heap::store` are `pub(crate)`:
public, the pair would carry a value from one heap's brand into another's,
`heap_b.load(heap_a.store(value))`, past every debug check. `Heap::new` and
`Heap::move_everything` are private to the heap module, so only `World` builds
the heap an interpreter has and only `World` moves it — never while a
`Mutator` is lent out, since `enter` holds `&mut self`.

**Three `compile_fail` doctests pin it**, on `Interpreter::enter`
(`interpreter/mod.rs`), each beside a passing twin that differs from it in the
one line that matters, so that each fails for the reason it names: answering a
handle out of an entry ("lifetime may not live long enough"), storing one in a
host's own field (E0521), and a host's send from inside an entry, the shape a
native re-entering the loop would have (the interpreter is already borrowed by
the entry).

**No branded code can hold a stale handle.** `same` compares two handles with
no heap in reach, so a debug build can check their move epochs only against
each other, and two stale handles from the same old epoch would pass it where
a live handle against a stale one panics. Within one entry nothing moves, and
no handle survives from one entry to the next, so every handle `same` is
handed was minted or read in the entry that compares it. Only a bare `Heap` in
`heap.rs`'s own unit tests can still make a stale handle, and those tests are
about the debug check itself.

`ActivationReference` stays unbranded: the collector moves no stack entry, and
its generation check already covers an index reused after a pop.

**Why no cast is needed.** The heap stores handles in their at-rest form and a
step sees them branded, so something has to connect the two. Converting each
payload safely was rejected for its cost ("Considered and rejected: a safe
conversion per payload" below), and one audited lifetime cast of the whole
step state was chosen in its place, then found unnecessary ("Considered and
rejected: an audited lifetime cast"). The two systems that cast have reasons
that do not apply here. gc-arena's `Gc<'gc, T>` is a real pointer whose `'gc`
must end with the borrow of the arena, so `mutate` has to bring its root, a
generic type projected through `Rootable<'gc>`, down from its at-rest lifetime
to that shorter one, which the compiler cannot check through the projection.
Nova's `Bindable for Box<T>` rebrands a payload that already exists from one
lifetime to another (`prior-art/nova/nova_vm/src/engine/context.rs`). Here
`'gc` names no real reference, only the `PhantomData` in a handle, and
`Mutator` is a plain type with a lifetime parameter: no step state is ever
brought from one brand to another, because the at-rest `'static` is itself one
of the lifetimes the closure is checked for.

The stress mode below keeps running under the brand: the brand covers what
Rust holds, the stress mode what the tracer knows. Its price is ergonomic —
every host and test reads through `enter` — and that price is accepted. What
the whole step measured is in [execution.md](execution.md), "What 2c moved":
level with the commit it started from on every source, every group overlapping
in both sittings. A first measurement had read 1–2.5% slower on the
instantiation and block benchmarks; the brand commit's hot machine code turned
out identical to its parent's, moved into another 64-byte phase by smaller
cold code ahead of it, which is why a measurement now builds every binary it
compares with function starts pinned to 64 bytes — the shipped build stays
unpinned ([execution.md](execution.md), "What removing the re-entries moved",
the protocol).

### Host roots

A host keeps values across runs: `ns-next bench` keeps a program's module
classes and `main:args:`'s arguments across its 22 samples, assembling and
sending to a fresh application each time, and `build` encodes the
application its factory and initializers assembled. A run may move every object, so
what a host holds across one must be where a move rewrites it: in the heap's
root table, which the tracer walks. A `Root` (`heap.rs`) is an `Rc`-shared
`Cell<Value>` slot the table also holds — gc-arena's `DynamicRootSet` model —
so releasing one needs no access to the heap: once the last clone outside the
table is dropped, the table holds the only reference, and its next sweep drops
the slot. The table is swept at every move, and by `stash` whenever it has
doubled since its last sweep (never below 64 slots), so it stays proportional
to the roots a host still holds in a release build too, where nothing moves.

Inside an entry, `Mutator::stash` roots any value and `Mutator::fetch` reads one
back (`Heap::stash`/`fetch`). Outside every entry, `Interpreter::stash` roots an
immediate a host built itself — `nil`, a Boolean or a `SmallInteger`; a float is
made only by `Mutator::new_float`, inside an entry, since it may be boxed — and
takes a `Value<'static>`: a host can build an immediate but never hold an object
handle outside an entry, so a `Value<'static>` holds none. All of them take
`&self`, the table sitting behind a `RefCell`, so that a host can stash an
argument inside the call that sends it — `interpreter.send(&receiver, selector,
&[interpreter.stash(value)])` — as `DynamicRootSet::stash` allows. A value
`fetch` answers carries the brand of the entry it was read in, so it is good
until that entry ends, and a host reads the root again in the next one.

Every host entry takes and answers roots: `Interpreter::send`, `instantiate`,
`load_module` and `platform`, and `image::load`, whose `LoadedImage` carries
every module's root class and the image's roots as `Root`s. A send or an instantiation
answers a `HostResult`, `Result<Root, HostSignal>`: its value rooted
(`Mutator::to_host`), a Newspeak error as a `HostError` whose receiver is a
`Root`, or a `^` that reached the host as `HostSignal::Return`. `Signal` and
`EvaluationResult` are crate-private, so no plain value leaves a run by any other
way than `fetch`; and since a `Root` can hold any value, `instantiate` refuses
one that holds no class object with a Newspeak error. The codec runs inside
an entry: `image::package` and `image::snapshot` take a `&Mutator<'gc>` and
branded values, which a host fetches from its roots in the
same entry — encoding runs no code, so nothing moves under it — and
`image::load` decodes inside one entry of the interpreter it bootstraps and
roots what it answers before that entry ends. `Root`, `HostSignal` and
`HostError` have no `Debug`, for the reason `Signal` has none (`CLAUDE.md`).

A host reads the heap only through `Interpreter::enter`, and nothing branded
leaves an entry, so a root is the only way a host can hold a value across a
run — the CLI and every test hold what they keep that way. That is a
compile-time guarantee ("The brand" above), no longer one only the stress
mode checks.

### What the collector traces

`trace.rs` enumerates the roots, type by type: a `Trace` impl hands every
field that holds an `ObjectReference`, a `ClassReference`, a `FrameReference`
or a `Word` to a `Tracer`, one `&mut` at a time. An object is no Rust type
with an impl: `heap/object.rs`'s `pointer_ranges` names the indices of its
pointer words by the format in its header
([object-model.md](object-model.md), "The object layout") -- the one
enumeration of an object's pointer words, which marking reads the words
through (`Heap::drain_marking`) and forwarding rewrites them through
(`trace_block`), so a field one of the two must reach cannot be missed by
the other. A
`Word` -- a class word, an instance's or an array's field, a register, a
copied closure value, an object field of a class object, a closure or a
frame -- goes to `Tracer::word`,
which marks or rewrites it by the offset it holds: it carries no origin or
epoch to check, since `Heap::store` checked the value it was made from
([object-model.md](object-model.md), "The debug build keeps its exact
check").

- The bootstrap: `builtin_classes`, `platform_singleton`, `vm_mirror` — the
  VM mirror, the one object whose method table holds the leaves — and
  `kernel`, `Kernel.ns`'s one instance and the enclosing object of every
  builtin class ([core-library.md](core-library.md), "The bootstrap:
  placeholders first, kernel second"), and `shipped_world`, every shipped
  module's instance and every shipped object under its name
  (`interpreter/shipped_world.rs`; [image-format.md](image-format.md),
  "Shipped names"). The step state holds all five beside
  the heap, with the depth limit's reserve below, and one `Trace` impl
  enumerates the six, `StepRoots` (`heap/world.rs`), which marking
  (`World::mark_everything_reachable`), forwarding after a collection or a
  stress move (`forward_step_state`) and the scavenge (`World::scavenge`)
  all trace, so a root added later is named once and reached by all three.
  The mirror is also the kernel's
  `vmMirror` slot and the kernel every builtin class's `enclosing` object,
  but a root of their own keeps both alive and forwarded whatever those
  links are.
- The activation stack: each entry's receiver, `current_class`, the
  `ClassReference` or closure object its `FrameKind` names, the frame it is married
  to, and the state its continuation carries — a `WhileLoop`'s condition and
  body, a `DoLoop`'s receiver and action, a `NestedClassCreation`'s
  enclosing object and declaring class, an `Instantiation`'s instance, levels
  with their arguments, and phase, an `Unwind`'s target frame and the value or error it carries,
  a `VmErrorSignalled` error's receiver, and a `Future` forcing's future and
  that of every forcing chained behind it; and every register. `StackEntry`'s impl is
  in `heap.rs`, where its fields are private.
- Every object in the space, by its format: its class word; an instance's
  or an array's fields; a closure's captured frame and its copied values,
  never its code id or feedback base; a class object's enclosing object,
  declaring class, superclass, for a metaclass its class (`this_class`), and
  its value-class refusal, never its builtin tag, applied clause or
  application, which are numbers; a frame object's ten fixed fields --
  sender, home, receiver, current class, the kind's operand among them, the
  rest numbers or constants the tracer passes over -- and its registers. A
  married frame's registers are its entry's, traced with the stack, so its
  own are skipped. The digits of a `LargeInteger`, a boxed `Float`'s bits and
  a string's code points are never handed to the tracer, since they would
  read as handles.
- The host root table.
- The kernel-private `Activation` class, every frame object's class, which
  the heap holds (`Heap::activation_class`).
- The depth limit's reserve, `Mutator.depth_reserve`: the frame of the
  activation an error near the limit arose in, marked, forwarded and
  scavenged with the bootstrap's roots through `StepRoots`
  ([execution.md](execution.md), "The depth limit").
- For a scavenge only, the remembered set: every old object that may name a
  young one, scanned by its format ("Generations"). Marking and forwarding
  need no such set, since they reach or walk every object.
- The heap's canonical symbol table ([object-model.md](object-model.md),
  "Values and objects"), forwarded by a move but not a root for marking: it
  is weak, as Primordial Soup's `symbolTable` is -- a symbol no program holds
  any more is free to go, and the next literal of its text allocates a new
  canonical one ("Weak references", below); a scavenge forwards its young
  entries and clears the ones it did not copy.

**Every impl names every field and matches every variant**, with no `..` and
no `_` arm, so a field or a variant added later does not compile until someone
decides whether it holds a handle; `pointer_ranges` likewise matches every
`Format`, so a new format does not compile until its pointer words are
named. That is what keeps the enumeration complete
as the types grow; the stress mode checks that the places it is applied to are
complete. Enumerating is `trace.rs`'s and rewriting is the heap module's
(`Forwarder` in `heap.rs`, `Scavenger` in `heap/scavenge.rs`), since only
the heap module can mint a handle. `trace.rs` exists in
every build, since the collector runs in every build.

A reified frame is not a root of its own: it is reached through the closure
that captured it, or through the live entry it is married to. Two things the
step state keeps hold no handle, on purpose, and a debug assertion before
every move checks both: a send's argument buffer, empty between instructions
("Where a collection can happen" above); and the spare boxes finished
instantiations leave for the next ones, emptied of every handle when they are
put back ([execution.md](execution.md), "No re-entry from Rust"). A nested
class being created is marked in its own cache slot by a primitive (`false`),
while the enclosing object stays in the continuation of the clause's stack
entry, where the stack is traced anyway.

**A handle lives only where the tracer can rewrite it in place**: in an
object's words, in a stack entry, in a continuation that one stack entry owns, or in
the step state's own roots — never behind a shared `Rc`, which the tracer could reach
only through a shared reference. So a closure's handles live in the
closure object's own words, and its code, which holds no handle, is named by
a code id into the heap's table of compiled code, shared with the
`MakeClosure` instruction that made it; and everything else
that needs a closure — `FrameKind::Closure` on each of its activations,
`WhileLoop.condition` and `.body`, a `DoLoop`'s `LoopAction::Closure`, `Step::Block` — names the
closure object, whose captured frame and copied values the tracer then
rewrites once, in its words.
A native loop's state is boxed in its continuation (`Box<WhileLoop>`,
`Box<DoLoop>`), owned by exactly one stack entry.

The inline caches ([dispatch.md](dispatch.md), "3b — The caches") add
nothing to this list, by design, and are not traced: an entry holds a
dispatch id and a target — an `Rc` to a method or a mixin, a field index, a
primitive, a level count, a family's `feedback_base` — and never a handle,
the defining class included, which a hit recomputes from the receiver. The
polymorphic pool and the global cache hold the same entries, and the family
map holds `Rc`s to code, which hold no handle. The table lives in the step
state, per heap, so nothing in it is ever reached from another heap, and a
move leaves every entry valid. The numbers the caches add to traced records
are not handles either, and the tracers say so: a stack entry's
`feedback_base` (its `Trace` impl) and a closure's feedback-base field
(`trace_block`), and the dispatch id every object carries in its header
word, which is what a guard reads ([dispatch.md](dispatch.md), "The dispatch
id"). The ids a cache guards on are per heap and never serialized. The
header's id is a number, not a handle: the tracer does not visit the header
word, and a move carries it along with the rest of the object.

**A resolved future is traced as its value.** Before it marks, a collection
(`World::collect`) rewrites every object field that holds a resolved future
of a simultaneous slot clause to hold its value instead
(`Heap::replace_resolved_futures`, called through
`interpreter::future::replace_resolved_futures`): an instance's and an
array's fields -- an indirection vector is an `Array` -- and a closure's
copied values. A class object holds no `Value` a future could stand in, a
string or symbol none at all, and a frame object is exempt as a register
file is, below. The
marker then finds a future that only object fields reached unreachable, and
the compaction reclaims it. A resolved future's value is never itself a
future (`Mutator::future_computed` follows a future of a future before it
records one, and one whose fields crafted code made so is malformed and
left in place), so one pass leaves none behind.

Registers and host roots are exempt -- a reified frame's, the activation
stack's, and every `Root`. `FutureImplementation`'s own
`doesNotUnderstand:`, whose activation forced it and is still returning its
answer, runs with the future as `self` -- a register that must keep naming an instance of the
class whose method runs -- and a host that holds the future holds that
object and may ask for its state; rewriting either would change what they
read. A future held there
survives the collection and is replaced by the first one after it lets go,
so a resolved future lives at most until then. The walk is gated on
`Mutator::futures_made`, so a program that never made a future pays nothing
for it, and the move stress mode (`World::move_everything`) does not
replace: moving is not collecting ([object-model.md](object-model.md),
"Simultaneous slots"). A scavenge does not replace either: the walk reads
every object of both generations, which a scavenge exists not to do, so a
resolved future stays until the next full collection.

### The move stress mode

The brand checks what Rust locals hold; it cannot check that the list above
is complete, and nothing checks that at compile time. A debug-build
stress mode checks it by running. With `NS_NEXT_STRESS_MOVES` set to anything
but empty or `0`, every interpreter the process bootstraps — the `ns-next`
binary the CLI tests spawn included — keeps the safepoint flag set and moves
everything at the safepoints its schedule picks, so the whole existing suite,
which pins every program's outcome, runs under it unchanged.

A move (`World::move_everything`: `Heap::move_everything`, then the step
state's own roots) shifts the objects of each region that holds any -- the
past survivor space, eden and the old space -- within that region, and
rewrites every traced handle: a compacting collection that treats everything
as live. When a region's objects start at its first offset they shift up by
`8 * (2 + epoch % 31)` bytes, `epoch` the new move epoch, varied so that
moves do not all shift alike, and otherwise back down to the first offset,
so no object keeps its offset at any move, young stays young, and no region
creeps upward (`compaction::Shift`); each nursery region keeps 264 bytes
free past what it fills (`REGION_SLACK`), so a shift of up to 256 bytes
never pushes an object out of it. The remembered set holds raw offsets that
no tracer reaches, so the move relocates each through the same shift
before the next scavenge reads it as a header. Every `ObjectReference` and
`FrameReference` carries, in a debug build and beside the origin it already carries,
the heap's move epoch when it was minted or last forwarded, and every accessor
compares the two, so a handle the tracer missed panics at its first read
("stale handle") instead of perhaps printing a wrong answer. The shift
checks the forwarding; the epoch checks the completeness. The `Forwarder`
checks each handle it rewrites as well — its origin, and that its epoch is the
one being moved from — because a foreign or stale handle stored in the heap is
a mistake the move would otherwise make permanent. A value stored in a heap
object or a register is a bare `Word` with no epoch to check, so `Heap::store`
refuses a foreign or stale value when it is stored, and the move rewrites the
word by its offset. `ActivationReference` carries no
epoch: no stack entry moves, and its generation already covers an index reused
after a pop. V8's `--stress-compaction`, SpiderMonkey's compacting GC zeal mode
and HotSpot's `ScavengeALot` exist for the same reason. Here the stress mode
also comes first: the tracing and forwarding the collector will need are built,
and held to the whole suite, before the collector is.

**The schedule is budgeted, not every safepoint.** A move costs work
proportional to what it traces — the objects, the activation stack's entries
and registers, the canonical symbol table and the host roots
(`Heap::traced_size`) — so moving at every
safepoint is quadratic in a deep recursion: the tests and goldens that recurse
to 100,000 activations, and the `bench_*` fixtures, would take hours per suite
run in a debug build. So while the traced size is at most
`STRESS_MOVE_BUDGET`, 1,000, every safepoint moves; above it, a safepoint moves
once the safepoints since the last move reach the traced size over the budget,
rounded up (`StressMoves::due`, `interpreter/safepoint.rs`). That spends about
1,000 handle rewrites per safepoint on average, the size-proportional pacing a
collector's allocation-driven trigger has, and every small program keeps a
move at every safepoint. The prior art uses a fixed frequency instead:
HotSpot's `ScavengeALotInterval`, SpiderMonkey's `JS_GC_ZEAL=mode,N`, V8's
`--gc-interval`. A fixed interval of one is the quadratic case, and any larger
one takes coverage from the small tests to spare the large ones, so pacing by
size was chosen instead, with the user, on 2026-09-13. A move comes only at a
safepoint, and a run that makes no send and closes no loop — a host's send of
`answer = ( ^42 )` — passes none, so a test made only of such runs is not
moved at all.

What it costs is time. The whole suite, test binaries already built, takes
190.3 s under the variable against 13.1 s without it, measured at 691 tests
either way (2026-09-14, after step 2c). Almost
half of that is `crates/ns-runtime/tests/trampoline.rs`, 90.8 s against 3.1 s,
nearly all of it in `the_depth_limit_is_exact_and_counts_alike_under_both_lowerings`;
most of the rest is the three CLI test binaries that spawn `ns-next` per
golden — `build_load.rs` 48.3 s against 3.5 s, `run_goldens.rs`
24.5 s against 1.6 s, `snapshot_load.rs` 20.7 s against 1.7 s — so
`cargo test -p ns-cli` alone takes about 97 s against 7.5 s. `run-corpus`,
whose six modules are small, takes about a third of a second either way.

**The inline caches have a stress mode of their own**, `NS_NEXT_STRESS_CACHES`,
which shares this one's safepoint: set, every safepoint first vacates every
inline cache, then the move schedule runs if it is on too, and either mode
keeps the safepoint flag set ([dispatch.md](dispatch.md), "Invalidation").
It moves nothing, so it checks the caches' filling and invalidation rather
than the roots; the suite takes about 1 min 5 s under it, against about
3 min 9 s under `NS_NEXT_STRESS_MOVES` (2026-09-16, 3b's head). The two are
separate gates, not combined, because an entry holds no handle for a move to
break.

The stress runs are gates: `CLAUDE.md` requires the whole suite and
`run-corpus` — every runnable module through `run` and through `build` then
`load`, under both lowerings — under the variable at the end of a batch, as
the `finishing-a-batch` skill lists them. A release build contains none of it:
the epoch, the move, the schedule and the test hooks
(`Interpreter::move_now`, `moves`, `stress_safepoints`, `set_stress_moves`)
are `#[cfg(debug_assertions)]`, and only the poll is left, whose price is
measured in "Where a collection can happen" above; `trace.rs` stays, since
the collector and the scavenger trace through it in every build.

### Considered and rejected: Nova's scopes

Nova's `GcScope`/`NoGcScope` with `Scoped` roots was this section's plan until
2026-09-13. It roots what a Rust frame holds across a call that can collect, and
it would have covered the four re-entry paths without removing them. It was
rejected because the paths were to go anyway — core-library's rule that a native
must not call back into Newspeak, and suspension and the debugger, need them
gone ([core-library.md](core-library.md), [image-format.md](image-format.md),
"Planned: suspend and resume") — and with them gone, as they now are, Nova's
machinery would stand with nothing to root, its `unbind` a hole in the static
guarantee.
A `Scoped` value is also retained for the whole call, past its last use, which
the brand never does. Engineering effort was not the constraint, so the brand
was built directly rather than after it.

### Considered and rejected: a safe conversion per payload

The heap stores handles at rest and a step sees them branded. Converting each
payload safely at the boundary — at a push, a pop, an allocation — rebuilds
it, and a `Box<WhileLoop>`, `Box<DoLoop>`, `Box<NestedClassCreation>` or
`Box<Instantiation>` can change its lifetime in safe Rust only by
reallocating: at every push and pop of a stack entry that carries one, which
is every iteration of a native `whileTrue:` or `do:` and every step of an
instantiation. The reuse of `spare_walks`' boxes, which measured 3–13% on the
instantiation benchmarks, could not have survived either. Rejected for that
cost, with the user, on 2026-09-13. Nothing is converted at a push, a pop or
an allocation: the whole step state is handed over as it rests.

### Considered and rejected: an audited lifetime cast

Chosen with the user on 2026-09-13 in the per-payload conversion's place: the
step state resting as `Mutator<'static>`, and `World::enter` casting `&mut
Mutator<'static>` to `&mut Mutator<'gc>` once per entry — gc-arena's
`Arena::mutate`, which casts its root the same way — as the crate's one
`unsafe`, under a crate-wide `deny(unsafe_code)`. Found unnecessary in the
review of the step's brand on 2026-09-14, because the higher-ranked closure
alone gives the guarantee: `body(&mut self.mutator)` compiles under
`#![forbid(unsafe_code)]`, and a closure that answers a handle still fails
with "lifetime may not live long enough", one that stores a handle in a
host's field with E0521. Why gc-arena and Nova need a cast and this does not
is in "The brand" above.

### Considered and rejected: conservative stack scanning

Scanning the Rust stack conservatively and *pinning* whatever ambiguous words
point at is the other way to avoid a handle discipline, and it is what V8 has
been moving toward: it ships in Oilpan, and pages with incoming ambiguous edges
go into a quarantine that is not evacuated. It was rejected here for two reasons.
Pinning is exactly the capability `become:` needs to not have, and after a decade
of work it is still not deployed in V8's own JavaScript heap — which is a fair
signal about how much of the difficulty is hidden.

### What this leaves for the collector itself

With the handles behind the seam and the roots checked, the collector's own
root set is the list in "What the collector traces" above. Most of it is two
things: the bootstrap, standing in for upstream's object store, and the
activation stack ([execution.md](execution.md), "Activations") — a
contiguous, precisely typed pair of arrays rather than a pointer-chased chain
of individually allocated frames. The safepoint, the tracing (`trace.rs`)
and the forwarding (`Forwarder`) exist in every build; the move
(`Heap::move_everything`) exists in a debug build, where the stress mode
checks the roots with it. The scavenger traces the same roots, plus the
remembered set, and a large object goes straight to the old space rather
than to a space of its own ("Generations").

Closure conversion ([execution.md](execution.md), "Closure conversion") was
built directly before the collector for the collector's sake. Before it, a
block that outlived its method kept that method's whole reified frame alive —
every register, including ones the block never reads — so the collector would
have retained more than the program can reach through its variables. A
surviving block now holds what it captured: its copied values, and the
indirection vector of any scope whose written variables it needs, plus its
creator's frame record without registers.

### The interim cost, and the one question still open

- **Neither arena reclaimed a slot before the collector**, and that was the
  interim cost the milestone's ordering accepted between the arena and step
  5. It was measurable, and the collector closes it. Peak working set on a
  release build, before the arena, after it, after the activation stack, and
  after the collector; the middle two columns were measured against each
  other in one sitting on 2026-09-10, the arena figures reproducing exactly
  the ones taken when the arena landed, and the last column is `014f0ff`
  against the `dc0ffe5` baseline of "How the collector is measured" below,
  both under `bench --iterations 10`:

  | Command | Entry sends | Before the arena | After the arena | After the activation stack | After the collector |
  |---|---|---|---|---|---|
  | `run`, `bench_loop.ns` | 1 | 4.6 MB | 4.6 MB | 4.6 MB | 4.7 MB |
  | `bench --iterations 10`, `bench_loop.ns` | 22 | 4.6 MB | 340.7 MB | 4.6 MB | 4.8 MB |
  | `bench --iterations 10`, `bench_send.ns` | 22 | — | 324.9 MB | 4.6 MB | 4.8 MB |
  | `bench --iterations 10`, `bench_block.ns` | 22 | — | 966.9 MB | 446.0 MB | 5.6 MB |

  `dc0ffe5`, "After the collector"'s own baseline, measures 228.8 MB on
  `bench_block.ns` under the same `--iterations 10` — about half the 446.0 MB
  "After the activation stack" figure above, taken on 2026-09-10. The
  likely contributors: closure conversion writing a body's frame back with
  no registers when nothing reaches it by reference, the 56-byte `Object`,
  and where a `Vec` sits against its own doubling threshold when the run
  ends. This is not decomposed.

  Conditions, because the metric is platform-specific and the numbers are
  useless without them: Windows 11, release build, **peak working set**
  (`PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize`) read after the process exits.
  PowerShell's own `Process.PeakWorkingSet64` returns 0 once the process has
  exited on this platform, so the figures come from `psapi!GetProcessMemoryInfo`
  called on the still-open handle. On Linux the comparable metric is
  `VmHWM`/`ru_maxrss`; expect different absolute numbers and the same shape.

  The shape is what matters more than the figures. An activation that nothing
  reified leaves nothing behind, so a program that creates no block per
  iteration now runs in the same 4.6 MB however many activations it builds —
  `bench_loop.ns` and `bench_send.ns`, whose blocks are created once, before
  the loop starts. An `ifTrue:`/`and:`/`whileTrue:` argument the compiler
  inlines creates no block at all, so it reifies nothing either: 300,000 calls
  of `clip: x = ( x < 0 ifTrue: [^0]. ^x + 1 )` peak at 4.6 MB (2026-09-11,
  under the conditions above).

  **What was unbounded — every reified frame and every object the process had
  ever allocated, not what was live — is now bounded by the collector.**
  `bench_block.ns` shows it most clearly: every iteration creates a block, so
  every iteration reifies one frame and allocates one closure, and before
  step 5 nothing gave either back. Under `bench --iterations 10` its peak
  fell from 228.8 MB at `dc0ffe5` — the commit directly before the collector
  — to 5.6 MB at the head, level with `bench_loop.ns` and `bench_send.ns`
  ("How the collector is measured" below has the full measurement, against
  the recorded expectation). The heap is now proportional to what is live
  rather than to what the process has ever created, because a collection
  runs at the next safepoint once the objects' bytes reach its limit ("The
  trigger and the growth policy" below). Closure conversion's own peak rise
  on this source under `--iterations 30` — 454.7 MB to 466.4 MB, attributed
  to the indirection-vector `Array` a written capture now allocates
  ([execution.md](execution.md), "What closure conversion moved") — was
  exactly this kind of cost: garbage the heap kept only because nothing
  reclaimed it. The allocation itself did not change; what changed is that
  the collector now takes it back.

  Peak does not climb smoothly with the work done. Sweeping `bench
  --iterations` over `bench_loop.ns` on the arena binary, before the activation
  stack, when every activation took an arena entry:

  | `--iterations` | 1 | 2 | 4 | 8 | 10 |
  |---|---|---|---|---|---|
  | Peak | 46.4 MB | 88.4 MB | 103.7 MB | 183.2 MB | 340.7 MB |

  The step shape is **consistent with** the arena's `Vec` doubling: the copy at
  a doubling point has the old and the new buffer resident at once, and would
  stall the run that triggers it. The benchmark maxima, measured in the same
  sitting as the last column above, do not contradict it, but they do not
  single it out either: they moved with retention. On `bench_send.ns`, whose
  retention the stack removed (324.9 MB to 4.6 MB above), the roughly doubled
  maxima went away — 40.4–40.8 ms over a 23.5 ms median under the arena,
  20.8–21.6 ms under the stack. The same happened on row two of the send
  decomposition ([execution.md](execution.md), "What the activation stack
  moved"); its peak working set was not measured, so that its retention went
  too is inferred from its shape, which creates no block per iteration.
  `bench_block.ns`, which then grew both arenas every iteration, still had a
  maximum well above its median: 63.8–67.9 ms over medians of 37.6–39.9 ms in
  all six `bench` runs of the 2026-09-11 before-and-after measurement of the
  retirement. `bench` reports only the median, minimum and maximum, so how many
  runs were slow is not measured. Pre-sizing the arenas with `with_capacity` is
  the control that would settle the doubling explanation, and it has not been
  run. This is recorded because a collector's design should know the interim
  may have a latency cost as well as a memory one.
- **The application cache grows without bound** in the presence of live editing,
  because every re-application mints a fresh entry that nothing evicts. It is
  settled with road step 5's edits, which replace a mixin under its id
  ([liveness-and-reflection.md](liveness-and-reflection.md), "Planned with
  road step 5: an edit replaces the mixin under its id"). Its key is a pair
  of the heap's ids since road step 3a, so evicting weakly is decidable; it
  stays strong until live editing reclaims an id ("Weak references" below).

### What an object costs

An object costs its words in the space and nothing else: 8 bytes a word in
both builds -- the header word, the class word, then the fields
([object-model.md](object-model.md), "The object layout") -- and one
overflow word more, before the header, when the words after the header
number more than 254, which the header's byte cannot count ("The object
space"). There is no slot, no pointer to the words and no allocation of
the Rust allocator per object. So the layout a debug build tests is the
layout a release build runs, and an object costs the same in both:

| Object | Words | Bytes |
|---|---|---|
| an instance of n fields, an `Array` of n elements | 2 + n | 16 + 8n |
| a class object | 2 + 8 | 80 |
| a closure with c copied values | 2 + 3 + c | 40 + 8c |
| a frame object of r registers | 2 + 10 + r | 96 + 8r |
| a `LargeInteger` of d digits | 2 + d | 16 + 8d |
| a boxed `Float` | 2 + 1 | 24 |
| a string or symbol of n code points | 2 + ⌈n/8⌉, or 2 + ⌈n/2⌉ when wide | 16 + 8⌈n/8⌉, or 16 + 8⌈n/2⌉ |

Each adds 8 bytes, its overflow word, once its words after the header pass
254: an `Array` of 254 elements or more, a frame of 244 registers or more.
The handles the Rust side holds are larger in a debug
build -- 16 bytes against 8, a `Value` 16 against 8 -- because a debug handle
carries the id of the heap that minted it and the move epoch the stress mode
checks it against; nothing inside an object is a Rust-side handle.

### The amplification bound of a rejected load

A **failed** image decode holds its placeholders until the load returns, and
that peak is what the collector's design and the image format's guarantee
need, stated carefully because the careless version is wrong.
`decode_graph_section` allocates every record's object before it fills any,
because a record must be addressable before the records that reference it
are read. The decoder allocates them only after the load's one tenure,
which runs between two entries; nothing before it runs Newspeak, since a
shipped name is resolved by table lookup, and the graph pass allocates
every placeholder old, after the tenure, and never polls a safepoint,
so nothing collects while the placeholders exist and a rejected image
cannot give them back part-way through ("Generations"). It is
not retained past that: `image::load` owns its `Interpreter` in a local, so
every `Err` path drops it and the space with it -- the cost is a transient
peak during loading, not a leak for the process's life.

**The decoder reads the graph section twice**
([image-format.md](image-format.md), "Two passes"). The first pass reads
every record as the second will and allocates nothing beyond transient,
bounded buffers; it sizes each record's object from the record's own bytes,
refuses what the second pass would refuse about counts, ids, a symbol named
twice and a `FutureImplementation` record of another field count, and refuses before
anything is allocated when the objects would take more than eight words a
byte of the input left. Then each record's placeholder is allocated at the
size the first pass read, and the second pass fills each in place, every
value stored as it is read and a string packed straight from the buffer, so
no record is ever built beside its placeholder. Since every placeholder is
sized from its own bytes, each byte belongs to exactly one record and one
value, and the ratios below take a maximum rather than a sum. Against the
bytes of the graph section and, for symbols' text, the symbol section (the
derivation is `decode_value`'s doc comment, `image/decode.rs`):

| Contribution | Release | Debug |
|---|---|---|
| A record's placeholder against its smallest record: a metaclass record of 9 bytes becomes a class object of 80 | 8.9x | 8.9x |
| The same with the decoder's own tables -- a handle per record in the placeholder table (8 bytes in a release build, 16 in a debug one), the first pass's 8-byte size, a metaclass record's pending index | **10.7x** | 11.6x |
| A value's word against its encoding: a one-byte `VALUE_NIL` becomes an 8-byte word | 8x | 8x |
| A UTF-8 byte of a string packed in the 32-bit form | 4x | 4x |
| An allocating value: an `i64` beyond 61 bits, or a float outside the immediate range, 9 bytes for a word and a 24-byte object (the `i64`'s one digit held in a boxed slice until its object is made, 4.4x while it lives) | 3.6x | 3.6x |
| A one-digit `VALUE_LARGE_INTEGER`, 14 bytes for the same 32 (its digits read into a `Vec` first, 2.9x while it lives) | 2.3x | 2.3x |
| A `VALUE_FRACTION` of two `i64` parts beyond 61 bits, 19 bytes for a word, a 32-byte fraction and two 24-byte `LargeInteger`s (its two digits held until each part's object is made, at most 5.5x while they live) | 4.6x | 4.6x |
| The root list, read after every record into `Value`s, `reserve_for` capping its reservation at the bytes left | 8x | **16x** |
| A symbol's text, allocated once per symbol-table entry | ≤ 4x of the symbol section | ≤ 4x of the symbol section |

The other record kinds stay below the metaclass record: a class record is at
least 19 bytes for the same 80 (4.2x; 5.5x and 5.9x with the decoder's
tables), a frame record at least 21 for 96 (4.6x; 5.7x and 6.5x), an
instance, array, string or symbol record at least 11 for 16 (1.5x), a
closure record at least 17 for 40 (2.4x), a byte-array record of `n` bytes
`14 + n` for `8 × (5 + ⌈n/8⌉)`, its alignment reserve not on the wire, at
most 40 for 14 (2.9x; 4.0x and 4.6x with the decoder's tables). A
placeholder is filled in place, so no record's fields are laid out beside
it.

**So the transient peak of a rejected load is about 10.7x of the graph
section's bytes in a release build (a graph of metaclass records) and about
16x in a debug one (a graph that is all root list), plus at most 4x of the
symbol section's bytes.** A fraction of two integers beyond 61 bits but
within `i64` is the worst allocating value: it costs a fraction and two
`LargeInteger`s for the 19 bytes of a tag and two `i64`s, as a lone such
integer costs a `LargeInteger` for the 9 bytes of an `i64`
([image-format.md](image-format.md), "Integers and floats on the wire", "A
`Fraction` is a value on the wire too"); a `VALUE_LARGE_INTEGER` that fits
`i64` and a fraction outside its normal form are refused before an object is
made, and a digit count is bounded against the bytes left before its `Vec`
is allocated (`read_large_integer_parts`).

**The shipped-name section adds no factor to that peak.** Its reader
(`shipped_names::decode_section`) bounds the count by `MAX_SHIPPED_NAMES`
(65,536) and by the bytes left at `MIN_SHIPPED_NAME_BYTES`, 9 an entry,
before it reserves a `Vec` of 16-byte names, at most 1.8x of the section's
bytes and 1 MiB; resolving them (`shipped_names::resolve`) reserves, for as
many entries, a set of indices and a `Vec` of resolved names of 32 bytes
(40 in a debug build, whose handle is two words), about 7.7x of the
section's bytes (8.5x) for the moment it runs and at most about 5 MiB, all
of it before any placeholder exists. A name the loading world does
not have and a name given twice are refused, so a section that resolves
lists at most as many names as the shipped world has, a few hundred; what
the graph pass holds beside its placeholders -- the names and their
resolution, a second resolution in the graph's own entry with the set
dropped again -- is therefore bounded by the shipped world, a few KiB,
not by the input. The figures above stand: about 10.7x of the graph
section's bytes in a release build and 16x in a debug one, plus at most 4x
of the symbol section's, plus that constant.

**What the figures do not count**: the Rust allocator's own overhead per
allocation of the decoder's tables; a table's `Vec` that doubles while the
decoder fills it, which holds both buffers for a moment; the object space's
commit growth, which commits at least double what it had committed, the
10 MiB nursery included, so the space may have up to about twice the
nursery and the placeholders together committed -- committed, not
touched, so it is charged against the commit limit but not the working
set ("The object space"); the old space the load's tenure commits before
it copies, for the worst case of everything young being promoted, which
the nursery bounds rather than the input; and the decoded `Vec<Instruction>` of
one compiled body at a time while the code table is verified ("The trigger
and the growth policy" below). Each stays proportional to the input.

`bounded_count` is what keeps all of this proportional to the input rather
than to a 32-bit count -- without it a crafted `u32::MAX` would allocate
billions of placeholders -- and the first pass's sizing is what keeps a
placeholder proportional to its own record. The residual factor is the price
of allocating before filling, which the format's back-references require.

This is also *better* than the model it replaced, which is the part that
makes it an accepted cost rather than a regression. Under `Rc<RefCell<_>>`
each placeholder was its own heap allocation with the same lifetime, plus a
refcount and an allocator round trip per record; the space makes each
placeholder one bump of the old space's top. The collector's interest here is
the peak, not a lifetime bug.

### The shape: mark, compact, forward

Decided with the user on 2026-09-20 over the arena the heap then had, and
kept for the object space; built. The collector marks from the roots,
slides the survivors down to the old space's first offset, and rewrites
every handle through a forwarding table, stop-the-world. Since road step 4
it is the full collection over both generations, and compacts the
nursery's survivors into the old space with the old space's own; a
copying scavenger collects the nursery alone between full collections
("Generations" below).

Three things the representation settles before any taste enters:

- **MMTk is out.** It is the one Rust framework whose collectors move, but its
  handles are raw addresses. A raw address as a handle makes every access a
  dereference that needs `unsafe`, and a stale one undefined behaviour rather
  than a wrong but safe read, so it would take with it the checked access, the
  origin guards, the epoch check and the brand's proof that every access is
  safe. That is a different runtime, not a choice of collector. The object
  space owns its memory as the address-based engines do but keeps handles as
  byte offsets read through bounds-checked slices, which keeps all four
  ("The object space").
- **Reference counting is out**, because cycles need tracing and `become:`
  needs forwarding.
- **Conservative scanning is out**, decided earlier and on its own grounds
  ("Considered and rejected: conservative stack scanning").

What is left is the choice between a non-moving free list, a semispace
copier, and mark-compact. Over the arena it was made on, every slot was the
same size, so a free list had no fragmentation to solve, and a copier moved
slots, not the words behind them, so it bought little locality. In the
space objects are variably sized, which gives a free list fragmentation to
manage; a sliding compaction leaves none and keeps the survivors in the
order they were allocated, which is the locality a copier would buy,
without a copier's second half of the space.

Mark-compact was chosen for four reasons:

- **It reaches the goal of the step.** The space shrinks and its pages go
  back to the operating system (`Space::release_above`). A free list holds
  the heap at its historic maximum for the life of the process, which is the
  figure step 5 exists to fix.
- **Rewriting is the cheap part here rather than the expensive one.** The
  forwarding table is a live-word bitmap with a prefix count per 64 words:
  O(1) per handle, one cache line a lookup, about 3% of the space while a
  collection runs ("Compaction and forwarding" below). Nova, the one engine
  whose handles are not addresses, pays more than it needs to: its
  `CompactionList` stores shift runs and binary-searches per handle
  (`prior-art/nova/nova_vm/src/heap/heap_bits.rs:1345`, `:1457`).
- **Its work is bounded by the space, not by the process's history.** Marking
  visits only what is live; the compaction's walks step through every
  allocation, the dead included, reading one word to step over a dead one,
  and copy only the survivors. A free list's sweep walks the same space and
  keeps all of it.
- **It uses the machinery that exists and is under test.** `trace.rs`,
  `Forwarder`, the epoch check and the move stress mode were built for exactly
  this shape; under a free list they would stay a debug-only appendage with no
  production role.

The prior art points the same way. Nova (`prior-art/nova/nova_vm/src/heap/`),
whose handles are indices, is mark-and-sweep with compaction over per-type
vectors and is not generational. Strongtalk's
full collection is a mark-compact despite the name of `markSweep.cpp`
(`prepare_for_compaction`, `compact`), under a scavenged young generation.
Spur compacts its old space by sliding, planning every survivor's new
address before moving any (`planCompactSavingForwarders`, `compact`,
`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:62399`, `:61596`),
which is the compaction here. Every current HotSpot collector moves, and the one
free-list old space it had, CMS's, was deprecated in JDK 9 and removed in
JDK 14. Primordial Soup is the one that keeps a free list, in its old space
(`vm/heap.h`, seven size classes) -- and with it the fragmentation a sliding
compaction does not leave.

### Marking

The mark bit is bit 33 of each object's header word
([object-model.md](object-model.md), "The object layout"), set while a
collection runs and cleared as compaction moves each survivor, so that
outside a collection no object is marked; a debug build checks that at the
start of every marking. A bit in the header needs no side table and is read
in the cache line of the object's own header; Spur and Primordial Soup keep
theirs there too.

The marker (`heap/mark.rs`) is a `Tracer` (`trace.rs`) beside `Forwarder`,
through which the roots enter its explicit work list; `Heap::drain_marking`
then pops each offset, skips an object already marked, sets the bit and
pushes every unmarked object its pointer words name, read through
`pointer_ranges` -- the one enumeration of an object's pointer words, which
forwarding's `trace_block` walks too ("What the collector traces"). The bit
is set when an object is visited, not when it is pushed, so an object
reached twice before its visit is popped twice and visited once, which also
ends the traversal on a cycle. The traversal is iterative because the
graph's depth is the program's to choose — the reasoning that already made
`MixinApplication`'s `Drop` iterative. Sharing one edge enumeration between
marking and forwarding is what keeps the two from drifting: a field a later
change adds is named in exactly one place, and the compiler refuses that
place until it is named.

The roots are the ones a move forwards -- the heap's own, which
`Heap::begin_marking` pushes (the `Activation` class, the activation
stack's entries and registers, the host roots), and the step state's,
`StepRoots` (the builtin classes, the `platform` singleton, the VM mirror,
the kernel, the shipped world and the depth limit's reserve) -- with the canonical symbol
table removed, since that is weak. The remembered set is not a root here:
marking reaches every reachable object of both generations along its
edges, an old one that names a young one included. The difference from a
move is the direction: a move walks every object because everything counts
as live, while the collector starts at the roots and reaches an object only
along an edge.

### Compaction and forwarding

Three walks over the space's occupied ranges -- the past survivor space,
eden and the old space (`Space::occupied`) -- each range from its first
object to its top, stepping from one allocation to the next by the word
count in each header, or in the overflow word before it
(`space::object_at`). The first records every word of each marked object's
allocation, its overflow word included, as live in the forwarding table, a
`ForwardingTable` (`heap/compaction.rs`): one bit per word of the space up
to the old space's top, the nursery included, and, per chunk of 64 words,
the number of live words before the chunk, filled in once the bits are
(`summarize`). The table is summarized in two segments, the old space's
chunks first and the nursery's after them, so a survivor's new position is
the old space's first offset (`NURSERY_END`) plus the live words before it
in that order -- the chunk's prefix plus the `popcount` of the live bits
below it in the chunk -- one entry, a mask and a `popcount` per lookup, the
bits and the prefix side by side so that a lookup reads one cache line;
`NURSERY_END` is a multiple of 64 words, so no chunk straddles the two
generations. The second walk slides the old space's survivors down to
their positions -- a destination there is never above its source, so
copying down never overwrites an allocation the walk has yet to read --
and then copies the nursery's survivors, the past survivor space's and
then eden's in address order, above the last old one: a copy reads below
`NURSERY_END` and writes above it, so the two never overlap. Each survivor's
mark bit and age are cleared as it lands. The remembered bits are cleared
first, at the offsets the remembered set holds, and the set emptied: once
everything is old nothing needs remembering. The new top is `NURSERY_END`
plus the live words, the nursery is empty, and a pending scavenge request
is served by the collection with it. The third walk is the existing
`Forwarder` over everything that holds a handle -- the roots and every
object by its format (`forward_everything`) -- each offset looked up in the
table. The table takes two `u64`s per 64 words of the space below the old
space's top, about 3%, and lives only while the collection runs (decided
with the user on 2026-10-03; "Decisions" below). Why the full collection
compacts both generations into the old space, and what was rejected, is
decision 2 under "Generations".

Frames are objects in the same space, so a closure's captured frame, a
frame's sender and home and the objects a frame holds are one graph with one
mark, one compaction and one forwarding table.

The epoch advances, and it is what makes a stale handle loud rather than
silent. A reclaimed old object does not linger to be read the way a free
list's would: the old space ends at the new top, and every accessor reads
through the slice below it, so an offset into the vacated tail finds no
word; one below the new top names whatever survivor slid there, and carries
an old epoch. The collection poisons nothing, unlike a scavenge
("Generations"): it rewrites every word of every object by format, so no
stored word can keep naming the nursery it emptied, and a Rust-side handle
that does carries an old epoch. Protecting the vacated pages waits until
generated code dereferences addresses ("The object space").

The move stress mode stays beside the collector rather than being folded into
it. It moves everything without liveness, so it isolates the completeness of
the root enumeration from the correctness of marking: when the collector's own
gate fails, a green move stress says the fault is in marking. `Forwarder`
therefore takes its mapping as a parameter, a `Relocation` -- the shift for
the stress mode, the forwarding table for the collector.

### The trigger and the growth policy

A scavenge and a full collection happen only at a safepoint of the run
loop, as decided on 2026-09-13, or between entries. An allocation sets the
request at most; it never collects.

The tests are comparisons, not counters, in `allocate_block`, through which
every allocation enters the space, `reify`'s frame object included, and
which holds the region's new top already. A young allocation compares
eden's occupied bytes against the scavenge threshold, 7.5 MiB -- eden's
8 MiB less its 512 KiB reserve (`EDEN_RESERVE`) -- and requests a scavenge
once it is reached. An allocation that goes old compares the old space's
occupied bytes -- its top less its start -- against the collection limit
and requests a full collection once it is reached; so does every scavenge,
after it has promoted. One predictable branch on each path, and no new
state on the hot path. At the safepoint a pending full collection is
served first, since it empties the nursery as well; otherwise a pending
scavenge.

After a full collection the limit becomes `max(floor, live * 2)` in bytes
(`collection_limit`), `live` the old space's occupancy after the compaction,
so that between two full collections no more is promoted or allocated old
than is live and the marking work per byte is amortized constant. The
floor -- 512 KiB of objects, about 4,096 objects at a 128-byte average
(`COLLECTION_FLOOR`, `heap.rs`) -- keeps a small program out of the full
collector altogether: `bench_loop.ns` and `bench_send.ns` never collect,
and of the twenty-four measured programs only Havlak and DeltaBlue reach a
full collection ([execution.md](execution.md), "What generations moved").
Soup's `GrowthPolicy` and HotSpot's free-ratio control have the same shape.

The space commits as it grows (`Space::ensure_committed`): when the heap is
made it commits the whole 10 MiB nursery and the old space's first 256 KiB
(`MINIMUM_COMMIT`), and a growth commits at least double what is committed
-- the committed range starts at the base, so the nursery counts, and the
first growth takes it from 10.25 MiB to 20.5 MiB -- and at least what the
allocation needs, rounded up to a page, so a run of allocations commits a
logarithmic number of times. Committed is not touched: a page costs the
working set only once something is written into it, which is what makes
eden's first fill a cost each process pays once
([execution.md](execution.md), "What generations moved"). Compaction gives
memory back against the same limit that triggers a collection
(`collection_limit`, which `compact` and `reset_collection_limits` share): a
space with more than twice its next limit committed past the nursery gives
back what lies above the nursery plus the limit (`Space::release_above`,
never below the old space's top or its first commit), and one within twice
it keeps what it has. Measured against the live set instead,
the two part under the floor -- a live set far below the floor would cut
the space to twice that, and the next cycle, allowed to reach the floor,
would commit its way back at every collection; on the arena, where the same
choice cut a buffer of slots, that was about 5% of `bench_value.ns`
([execution.md](execution.md), "What exceptions cost"). The price is
retention: under the floor the space keeps up to twice the floor committed
past the nursery after a collection, 1 MiB, in either build.

The image decoder's graph pass never collects, because it never polls — it
does not run in the run loop — and it starts after the load's one tenure
("Generations"). The transient peak described under the decode
amplification above therefore stands: a rejected image's placeholders are
reachable until validation finishes, so they are live, and the
amplification figures stay valid.

A second transient, not in those figures and not measured: a compiled body is
stored as eight-byte instructions, and the decoder reads each body back into a
full decoded `Vec<Instruction>` (48 bytes an instruction, literal clones
included) once, in `verify_code_table`, whose two checks that read every
instruction (`verify_body`, `verify_outer_locals`) share it; it is dropped
before the next body is decoded. Everything else at load reads the encoding in
place: `CompiledCode::from_builder` derives `family_size` and `outer_reach` off
the builder's tables, and `block_nesting`, `lexical_parents` and the call-site
numbering walk the body's blocks and send sites
(`EncodedBody::created_blocks`, `for_each_emitted_mut`). The memory stays
bounded by the input, since every instruction consumed at least one wire byte.

In a release build the two triggers are what the safepoint's slow path serves.

### Weak references

**The canonical symbol table is weak** (`canonical_symbols`,
`Heap::prune_canonical_symbols`). As a strong root every symbol object a
program ever made would outlive the process, since a selector literal makes
one. It is not a root for marking; after marking and before compaction, an
entry whose object is unmarked becomes `None`, and the rest are forwarded like
everything else. The table has an inverse map, from a symbol object's offset to
its `Symbol` (`Heap::symbol_of`), keyed by the offset so that it holds no
handle, pruned with the table and rebuilt after every compaction and
move.
Nova has exactly this pair: `shift_strong_u32_index` answers an index,
`shift_weak_u32_index` answers an `Option`.

**A scavenge prunes the table too**, since a young symbol can die in
either kind of collection (`Heap::forward_weak_symbols`). It is not a
scavenge's root either: once the copying is done, a young entry whose
object was copied -- its old place a forwarder -- names the copy, a young
entry whose object was not is gone, becomes `None` and leaves the inverse
map, and an old entry is left alone, since a scavenge reclaims nothing old.
The inverse map follows each moved entry to its new offset. Every entry is
checked for its heap and its epoch as the scavenger checks a handle it
traces, and rebranded into the scavenge's epoch.

This does not weaken §5.1.6. Equal symbols are one object as long as
`Mutator::new_symbol` is the only way to make one. If a symbol object is
collected then nothing held it — anything that did would have been a root and
kept it live — so the next `new_symbol` mints a fresh one whose identity is
again unique, and no live comparison can tell the difference. An identity hash
belongs to the object rather than to the symbol; a program that keeps the
number `identityHash` answered instead of the object has no guarantee today
either, and Squeak's weak symbol table behaves the same way.

**The application cache stays strong, and the reason is live editing rather
than the collector.** Its key is a pair of the heap's ids -- the mixin's
`MixinId` and the superclass application's `ApplicationIndex`
(`heap/tables.rs`) -- not two addresses, so a freed `Rc<Mixin>` handing its
address to a newly allocated one can no longer make the new mixin hit the old
entry and be given a **different, perfectly valid** application, the failure
class the handle invariants exist to prevent. The heap's tables still map a
mixin's and a body's address to its id (`Rc::as_ptr`), which is sound only
because each table holds the strong `Rc` that keeps the address valid and
unique for as long as the entry lives -- forever, until an id is reclaimed.
So weakness is decidable now, and what makes it sound when it comes is that
the cache's key and value use an application's full-width index, never its
22-bit guard id, and the reuse rules: a full-width index is reclaimable once
no class object, no application-cache key or value and no other
application's superclass link names it; a guard id once no object header, no
inline- or global-cache entry and no recorded guard names it; a mixin id once
the `Mixin` itself is dropped -- its table entry freed, no declaration,
enclosing mixin or code holding it -- and no application, no
application-cache key or value and no class object's applied-clause field
names it ([object-model.md](object-model.md), "An application has two ids").
A class's change of application restamps every instance's header by a heap
walk (`Heap::class_set_application`) before the old guard id could become
reclaimable. Nothing drops a mixin before live editing, so the ids are
minted and none is reclaimed, and the cache stays strong until then (decided
with the user on 2026-10-03); its unbounded growth under live editing
([liveness-and-reflection.md](liveness-and-reflection.md)) ends with the
first edit's reclamation, which road step 5's atomic install brings
([liveness-and-reflection.md](liveness-and-reflection.md), "Planned: road
step 5, mirrors and atomic installation"). The same per-mixin id serves dispatch's open
option of guarding a private lexical site on N's mixin
([dispatch.md](dispatch.md), "Decisions").

The cache holds no handle at all — a pair of numbers for a key, an `Rc` for a
value — so it is neither a root nor a forwarding target.

Nothing else is weak. An inline-cache entry holds no handle by construction, so
it survives a collection untouched and simply misses when its guard no longer
matches. Compiled code holds none either: `Literal::Symbol` is an interner
index, not an object.

### Frames, the activation stack and the marriage

The activation stack is a root, and nothing there is reclaimed: an entry is
popped when its activation returns, so the stack is bounded by the deepest the
program has been. Marking walks every `StackEntry` — receiver, `current_class`,
`kind`, the continuation and `reified` — and every register in `stack.values`.

A frame object is collected exactly as every other object is, with the same
mark bit and the same forwarding table: there is no separate store for
frames.

**A married frame cannot be collected, and that needs no rule of its own.**
`StackEntry::trace` follows `reified`, so a frame whose activation is still
running is reachable from the stack and is marked. Leaving that to the root set
rather than to a special case is what keeps a later change from silently
freeing a running activation's frame.

The marriage survives compaction in both directions: the frame object's
married field holds a *stack* index as a `SmallInteger`, and no stack entry
moves, and `StackEntry::reified` is forwarded like any other handle. While
it is married the frame's own registers are nil and skipped by the trace,
since the live ones are the entry's. The divorce on pop writes the program
counter and the uncontinuable flag, clears the marriage, and copies the
registers into the frame object only when the activation retains them.

**A frame can be tenured while it is married.** A scavenge copies a young
married frame like any other object -- its registers skipped, since they
are the entry's -- and `StackEntry::reified` is rewritten to the copy, in
the survivor space or the old space; the married field still holds the
stack index. While it is married an old frame needs no remembering: its
live registers are on the activation stack, which every scavenge traces as
a root. The divorce is a store into an existing object, so it takes the
barrier's rule: when it copies the registers into an old frame and one of
them names a young object, it remembers the frame (`Heap::pop_activation`),
since from then on the stack no longer covers them. A later
`StoreOuterLocal` into the divorced frame goes through
`Heap::frame_set_local` and its barrier ("Generations").

Pinned across a move and a collection by `heap.rs`'s unit tests
(`a_married_frame_survives_a_move_and_is_divorced_into_its_moved_object`,
`a_frame_divorced_after_a_collection_holds_its_registers_only_when_retained`).

What keeps a frame alive past its return is a closure's captured frame, or
another reified frame's `sender`/`home`. Closure conversion shrank what a
frame retains -- a body nothing reaches by reference keeps its registers nil
-- but did not remove the frame: a closure still names its captured frame,
because the home of a non-local return hangs on it. A surviving closure
therefore retains its frame and, through that frame's `sender`/`home`,
possibly a chain. That is the semantics rather than a defect, and it is what
`bench_block.ns` measures.

### The gates the collector needs

The existing gates cover the completeness of the roots and the rewriting,
because the move stress treats everything as live. The collector's own failures
are different: **premature reclamation** — a missed root, a reachable object
gone, silent wrong output at exit status 0 — and **false retention**, which no
test sees at all and only a measurement shows.

There are five stress modes, each a gate of its own at the end of a batch,
each set by an environment variable to anything but empty or `0`, all of
them debug builds only: `NS_NEXT_STRESS_MOVES` ("The move stress mode"),
`NS_NEXT_STRESS_CACHES` ([dispatch.md](dispatch.md), "Invalidation"),
`NS_NEXT_STRESS_COLLECT`, `NS_NEXT_STRESS_SCAVENGE` ("Finding a missed
barrier: the scavenge stress mode"), each of these four keeping the
safepoint flag set while it is on, and `NS_NEXT_STRESS_WIDE`, which emits
every instruction in a wide form ([execution.md](execution.md), "The
compact instruction encoding").

`NS_NEXT_STRESS_COLLECT=1` runs a full collection at every safepoint rather
than at a limit, paced by heap size like `STRESS_MOVE_BUDGET`
(`StressSchedule`, `interpreter/safepoint.rs`), since collecting at
literally every safepoint is quadratic in a deep recursion. Under it every
golden and every corpus module runs at the highest collection frequency there
is, so a missed root kills its object at the earliest possible moment and the
recorded outputs catch the difference. `NS_NEXT_STRESS_SCAVENGE=1` does the
same with a scavenge, after a check of the remembered set, and is what finds
a store that missed the write barrier.

False retention is caught by counting rather than by observing: a
`debug_assertions`-only `collect_now`, beside `move_now`, and a
`heap_statistics` answering the objects lying in the space and how many of
them are young, the bytes they occupy and the bytes committed for it. An
object in the space is counted until a collection reclaims it, so between
two scavenges the count includes eden's dead objects; a test that counts
collects first. A program that discards n closures is back at its starting
size after a collection; one that keeps n closures keeps n frames.
`--collector-statistics` prints the scavenges and their total and longest
pause, the full collections and theirs, and the objects lying in the space,
on one line to stderr (`crates/ns-cli/src/entry.rs`).

### How the collector is measured

The baseline is `dc0ffe5`, the merge of closure conversion into `master`, with
`be3eb40` carried beside it as in every measurement since step 2. All eight
sources, two sittings, every compared binary built with the protocol's explicit
flags. `dumpbin /disasm` before any timing: `vm::run`'s step must still be one
function. That matters more here than usual, because the trigger adds a
comparison to `push_object` and the safepoint's slow path gets real code for
the first time — `safepoint` being `#[cold]` and `#[inline(never)]` is checked,
not assumed.

Two metrics, not one: throughput as always, and peak working set, since the
memory is the point of the step. The table above gains a fourth column. Beside
them, reported separately, the number of collections, the total pause and the
longest single pause, which `bench` learns to report the way it reports cache
statistics.

The expectation is recorded before the measurement, so that the result is not
read to fit afterwards. `bench_block.ns`'s peak falls substantially.
`bench_loop.ns` and `bench_send.ns` run in 4.6 MB, stay under the floor and
never collect, so their throughput must not move measurably, and the send
benchmarks must not feel the added comparison in `push_object`. And
`bench_block.ns`'s maxima — 63.8-67.9 ms over medians of 37.6-39.9 ms today,
consistent with the arena `Vec`s doubling — are the figure the pause has to
justify itself against. A falling median with a rising maximum is a result, not
a puzzle.

**Measured 2026-09-20, against both `dc0ffe5` and `be3eb40`, by the protocol
above.** The peak held exactly as expected: `bench_block.ns` under
`bench --iterations 10` falls from 228.8 MB at `dc0ffe5` to 5.6 MB at the
head, level with `bench_loop.ns` and `bench_send.ns`, which stay at 4.6-4.8 MB
throughout. `bench_loop.ns`, `bench_send.ns` and `bench_megamorphic.ns` report
zero collections apiece — they stay under the 4096-slot floor — and those are
exactly the three sources whose throughput does not move: −0.3%, −0.0% and
−0.4% against `dc0ffe5`, all three groups overlapping in both sittings. The
send benchmarks do not feel the trigger's comparison in `push_object`. Three
more sources — `bench_do.ns`, `bench_new.ns` and `bench_new2.ns` — do
collect, 26 to 54 times over ten iterations, and stay level with `dc0ffe5`,
their groups overlapping the baseline's in both sittings, means spanning
−2.1% to +0.6%. `bench_newarg.ns` collects 107 times, the second-most of the
eight, and is the narrowest of the level sources at +2.9% pooled: entirely
above `dc0ffe5` in the second sitting and overlapping it by only 0.01 ms in
the first. All four of these collecting, still-level sources stay 5.3-18.0%
below `be3eb40`.

**`bench_block.ns` is the one source that contradicts the expectation, and it
does so plainly: +12.2% against `dc0ffe5`, the one source whose pooled
groups do not overlap in either sitting and the only one beyond a couple of
percent** (still 5.3% below `be3eb40`). It is the only one of the eight
that allocates a closure per iteration, so the only one that collects
enough to show: `bench --collector-statistics` over `bench --iterations 10`
records 269 collections, 15.479 ms total pause, 0.202 ms longest. Pause
accounts for roughly 1.4 ms of the 2.46 ms the head adds per round; the
remaining amount is not accounted for by pause and is recorded here as
unexplained rather than guessed at. The longest single pause, 0.202 ms, is far
too small by itself to be the mechanism behind the older maxima gap this
section flagged in advance (63.8-67.9 ms over medians of 37.6-39.9 ms); this
measurement recorded only the five medians per round, not each run's own
minimum and maximum, so whether that gap persists under the collector is not
re-tested here and stays open. The full throughput tables, the collector
statistics for every source and the inline check are in
[execution.md](execution.md), "What the moving collector moved".


### What owning the memory gave

Before the object space, an object's words were one separate Rust
allocation, its block, behind a 16-byte slot of an arena: reading a field,
or the guard in the header, was two dependent loads, the slot's pointer and
then the block, usually in different cache lines, and a collection dropped
the dead blocks one call into the system allocator each, which the program
then allocated again as it ran on. Since road step 3b an object is its words
in the space, and a handle reaches them with one bounds-checked index from
the space's base; allocation is a bump of the top, and a collection frees
nothing one by one -- it slides the survivors down and moves the top.

**What the allocator cost was measured on 2026-09-28**, before road step
3a, when an object's variable part was a payload of its own beside a 56-byte
slot, by swapping the
global allocator of `ns-cli` in scratch builds (three builds a side, two
sittings, medians, the measurement protocol of [execution.md](execution.md)),
against the Windows heap the shipped build uses:

| Source | mimalloc | A bump allocator that never frees |
|---|---|---|
| `bench_loop.ns` | level | +6.8% |
| `bench_send.ns` | level | +2.6% |
| `bench_block.ns` | **−15.6%** | −10.3% |
| `bench_megamorphic.ns` | level | +6.6% |
| A scratch source churning short-lived lists, instances and strings | **−8.9%** | −5.6% |

An xperf profile of the shipped allocator put 16.9% of `bench_block.ns`'s
samples and 11.0% of the churning source's in `ntdll.dll`'s heap; under
mimalloc about 3% and 2% remained in the allocator's own functions, with about
1.3% more in the drop glue of the payload types the slots then held. The bump
allocator, which never reuses memory, was slower than mimalloc and on three
sources slower than the Windows heap: memory that is never recycled is cold.
The space reuses its memory -- a compaction slides the survivors into the
bytes the dead left -- and measured `bench_block.ns` 22.1% faster than the
arena, the allocation-heavy microbenchmarks 7.4% to 13.1% faster
([execution.md](execution.md), "What the object space moved").

**That measurement answered only what the allocator calls cost**, and it is
not why the address-based engines own their memory. They do because of what
owning it makes possible:

- **Fields inline with the header.** Primordial Soup, Strongtalk and Spur lay
  an object out as its header followed by its fields in one allocation, so a
  field is one load at a fixed offset (Strongtalk's compiled field load is
  `movl dst,[base + offset*oopSize - Mem_Tag]`,
  `prior-art/strongtalk/vm/compiler/codeGenerator.cpp:930-941`). Built: road
  step 3a put the fields inline, and road step 3b the objects in the space,
  so a field is one load from the space's base plus the handle and the
  field's offset, behind the slice's bounds check.
- **A nursery where death is free.** Soup's semispaces
  (`prior-art/primordialsoup/vm/heap.h:40-73`), Strongtalk's eden and
  survivors and Spur's scavenger copy the survivors and never touch the dead,
  so a young collection costs what survives. Built: road step 4 put the
  nursery at the low end of the space's reservation ("Generations").
- **Allocation a JIT can inline.** Cogit bumps `freeStart` against
  `scavengeThreshold` in machine code for `new`, closures and boxed floats
  (`genPrimitiveNew`, `genCreateFullClosure:…`,
  `genInstantiateFloatValue:into:scratchReg:scratchReg:mayFail:`,
  `prior-art/opensmalltalk-vm/src/spur64.cog/cogitX64SysV.c:16464`, `:19026`,
  `:15181`);
  Strongtalk does it in generated stubs its compiled code calls
  (`test_for_scavenge`, `prior-art/strongtalk/vm/prims/generatedPrimitives.cpp:52-69`).
  A `malloc` call cannot be inlined; a bump of the space's top can, which
  road step 6 does ("Planned: inline allocation").
- **A moving collector owns the space it moves objects in.** Built: the
  collector slides the objects within the space it owns.

So the question was never where blocks live but whether they are separate
at all: a block region beside the arena would have kept both the second
load and the per-object free.

**An object is size-stable on every hot path.** Both reference runtimes
implement a growable collection in Newspeak over a fixed `Array`: Primordial
Soup's `List` (`prior-art/primordialsoup/newspeak/Collections.ns`) holds
`data ::= Array new: capacity` and on overflow does `data:: data copyWithSize:
(capacity >> 1 + capacity | 7)`; SOMns's `Vector`
(`prior-art/somns/core-lib/Kernel.ns`) allocates `Array new: 2 * storage size`
and copies. ns-next's `List` in the shipped `CollectionsFactory.ns` does the
same. **Every object is fixed-size once allocated: no block grows in
place** -- a frame object is allocated with its code's full register file,
and an image's frame record carries exactly that many registers
([image-format.md](image-format.md), "Verification") -- so the object
space needs no growth-in-place case. What changes an
object's size is reflective and rare -- edit-and-continue adding locals to a
live activation, a MirrorBuilder adding or removing slots of live instances
(requirements 2 and 3 of
[liveness-and-reflection.md](liveness-and-reflection.md)) -- and will take a
new object: for instances, road step 5's atomic install will forward the old
ones to their replacements in one bulk forward, a walk of every root the
collector traces (`trace.rs`) and every heap object that rewrites every
reference ([object-model.md](object-model.md), "Planned: `become:` as an
eager bulk forward"); for a frame, the activation will be divorced into the
larger frame object and the VM will rewrite the references to the old one by
the same walk, the new frame allocated old and born remembered ("The object
layout", there).

**No existing arena crate does what the space needs.** Every arena crate
models one lifetime pattern -- everything lives until a reset -- where this
wants another: a tracer decides per object, and what lives is moved.
`bumpalo` offers no individual deallocation and no linear address space;
`typed-arena` hands out `&mut T` that pins its contents; `slab`, `slotmap`,
`generational-arena` and `id-arena` are fixed-slot object arenas; an
`allocator-api2` bump allocator behind `Vec<Value, _>` never reclaims.
Compaction is not an allocator feature -- it needs to know the references --
so it stays ours. The one level where a crate fits is pages, which the
memory layer reaches through `windows-sys` and `libc`.

## The memory layer

`crates/ns-memory` reserves one contiguous address range, commits, decommits
and protects page ranges of it, and lends accessible committed ranges as word
slices (`Reservation`, `Protection` and `MemoryError` in
`crates/ns-memory/src/lib.rs`; `PageMap`, its record of which pages are
committed and how they are protected, in `page_map.rs`; the operating-system
calls in `platform/windows.rs` and `platform/posix.rs`). It is the only crate
in the workspace with `unsafe` code, and `ns-runtime` keeps
`#![forbid(unsafe_code)]`. That is the shape `unsafe` takes in this workspace
([README.md](README.md), "Principles"): confined to one small layer whose
interface is safe again -- a malformed range is an error value, and a slice is
lent only over memory that is committed and accessible -- so everything above
it keeps the compiler-checked guarantee. This is step 1 of the road in
[OPEN-WORK.md](../OPEN-WORK.md), "The road to a high-performance VM". The
heap is its user: each `Heap`'s object space is one `Reservation` ("The
object space" below). `Reservation` is not `Send` or `Sync`, since it holds
a `NonNull`, and nothing needs it to be: the space belongs to one `Heap`,
and an interpreter, which holds `Rc`s throughout, is not `Send` either.

- **One contiguous reservation per heap**: `Reservation::reserve` rounds the
  size up to the platform's reservation granularity and commits nothing. The
  heap holds its object space in it, reserved once at a generous size --
  64 GiB on a 64-bit target and 1 GiB otherwise (`DEFAULT_RESERVATION`,
  `heap/space.rs`), since reserved address space costs nothing until it is
  committed -- and committed as the space grows; the nursery lies at its
  low end, below the old space ("Generations"). A handle is an offset from the
  reservation's base, so the base must be fixed and the reservation
  contiguous, as HotSpot's compressed oops require; Spur's segments at
  scattered addresses (`addSegmentOfSize:`,
  `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:63653`) do not fit.
  Strongtalk has this shape: `ReservedSpace` reserves, `VirtualSpace::expand`
  commits (`prior-art/strongtalk/vm/runtime/virtualspace.cpp:27-31`, `:73-135`;
  `VirtualAlloc` with `MEM_RESERVE`, then `MEM_COMMIT`,
  `prior-art/strongtalk/vm/runtime/os_nt.cpp:403-414`).
- **Operations**: `reserve`, `commit` a range, `decommit` a range when a space
  shrinks, `protect` a range as no-access or read-write, and lend each
  accessible, committed range as its own `&[u64]` (`words`) and `&mut [u64]`
  (`words_mut`, and `split_words_mut` for several disjoint ranges at once),
  and the committed read-write range at the base up to an end, `0..end`, in
  O(1) (`prefix`, `prefix_mut`): the reservation keeps the end of that range
  as a field, refreshed whenever its page map changes, so lending it is one
  comparison rather than a walk of the map; dropping the reservation
  releases it. Ranges are byte offsets from the
  base: commit, decommit and protect take page-aligned, non-empty ranges
  inside the reservation, the slices 8-byte-aligned ones that are committed
  read-write throughout and no longer than `isize::MAX` bytes, and a
  malformed range is `MemoryError::InvalidRange` (or `None` from a slice
  accessor) with no system call made. `protect` and `decommit` take the
  reservation by `&mut`, so no lent slice can be alive across a change of what is
  accessible. After a failed `decommit`, or a failed `protect` to no-access,
  the page map records the most restrictive state the call could have left
  (`record_failure`): it may under-report access after a failure but never
  over-report it, so a lent slice never covers memory a partly failed call
  changed. The heap does not store slices borrowed from its own
  reservation, which would make it self-referential; it re-borrows the
  prefix up to the old space's top at every access (`Space::words`,
  `words_mut`) and indexes it by the offset, so an offset at or above the
  top finds no word: that slice is the bounds check the accessors rely on.
  The old space lies above the nursery, so the one slice covers all four
  regions and the heap needs no other; `split_words_mut` is unused by it.
- **Two implementations behind one interface**: Windows (`VirtualAlloc`,
  `VirtualFree`, `VirtualProtect`, through `windows-sys`) and POSIX (`mmap`
  with `PROT_NONE` to reserve, `mprotect` to commit and protect, a re-mapping
  of the range with `mmap(MAP_FIXED | MAP_PRIVATE | MAP_ANON, PROT_NONE)` to
  decommit, which gives the memory back on Linux and macOS alike where
  `madvise(MADV_DONTNEED)` does not on macOS; it is mapped without
  `MAP_NORESERVE`, since a `PROT_NONE` private mapping is not charged anyway,
  so a later commit by `mprotect` is accounted where the kernel accounts
  commit -- Linux under strict overcommit (`overcommit_memory=2`) -- and can
  fail there as a value; under Linux's default heuristic overcommit and on
  macOS, which does no commit accounting, first touch can still end in the
  out-of-memory killer, as for any process. Windows always accounts a commit,
  so there it always fails as a value. `munmap` releases, all through `libc`).
  The POSIX half is checked with clippy for Linux and macOS targets, not run.
  `windows-sys` and `libc` are declaration-only crates, with no run-time code
  of their own. Primordial Soup has the same split behind the same three verbs
  (`prior-art/primordialsoup/vm/virtual_memory.h`, `virtual_memory_win.cc:65`,
  `virtual_memory_posix.cc`); it commits everything at once, which a single
  large reservation cannot.
- **A failure is a value, never a panic**: a reservation or a commit the
  operating system refuses answers a `MemoryError::Refused` carrying the
  operation and the system's error number. What running out of memory does
  is the heap's to decide: it aborts ("The object space").
- **No-access protection of a vacated range is supplied, and the heap does
  not use it yet.** Soup protects a vacated range around its scavenges and
  mark-sweeps (`from_.NoAccess()`,
  `prior-art/primordialsoup/vm/heap.cc:341-344`, `:653-655`), and since an
  offset into a protected range finds no slice, a stale one would fail the
  accessor's bounds check at once. The crate supplies `Protection::NoAccess`
  and the slice refusal; why the heap protects nothing is in "The object
  space" and, for the nursery a scavenge vacates, "Generations".
- **Executable memory for the JIT tiers** comes to the same crate when they
  do, with its `unsafe` entry into generated code.

## The object space

Built by road step 3b on 2026-10-03
([its plan](../superpowers/plans/2026-10-03-1934-ns-next-object-space.md)),
after the eight-byte value (road step 2), the compact instruction encoding
and road step 3a, which fixed the layout of an object's words; 3b changed
where the words live and nothing about the layout; road step 4 split the
space into two generations. Every object of a `Heap` lies in one space
(`heap/space.rs`), a `Reservation` of the memory layer above: each its
header word, its class word and its fields ([object-model.md](object-model.md),
"The object layout"). The space is four regions, low to high -- two
survivor spaces, eden and the old space ("Generations") -- and in each the
objects lie end to end from the region's start up to its top.

- **The reservation and its growth.** A heap reserves 64 GiB on a 64-bit
  target and 1 GiB otherwise (`DEFAULT_RESERVATION`): address space costs
  nothing until it is committed. When the operating system refuses it (an
  address-space limit, a sandbox), the request is halved and retried down to
  1 GiB (`MINIMUM_RESERVATION`, `reserve_falling_back`) before the heap
  aborts. The first commit is the whole 10 MiB nursery and the old space's
  first 256 KiB (`MINIMUM_COMMIT`), and a growth commits at least double
  what is committed and at least what the allocation needs, rounded up to a
  page, never past the reservation (`Space::ensure_committed`), so a run of
  allocations commits a logarithmic number of times. A compaction gives back
  what lies above the nursery and the next limit once more than twice the
  limit is committed past the nursery ("The trigger and the growth
  policy"). Each constant is one line to change.
- **Allocation is a bump of a region's top** -- eden's for a young object,
  the old space's for a large one, one the image decoder makes, or one that
  found eden spent (`Space::allocate_young`, `allocate_old`, through
  `Heap::allocate_block`; "Generations") -- as Soup's `Heap::Allocate`
  (`prior-art/primordialsoup/vm/heap.h:373-387`), and every allocation
  writes its object's header, class word and fields in place, so no object
  is built beside the space and copied in. Objects are 8-byte aligned and at
  least 16 bytes, a header and a class word.
- **A handle is a 64-bit byte offset from the reservation's base** to the
  object's header, a `NonZeroU64`, so an `Option` of a handle is one word
  too, and base plus handle is the object's address, the form generated code
  will use. The first allocation starts at offset 16 (`FIRST_OBJECT`), not
  0, so an all-zero word -- a missed initialization -- is never a live
  reference. There is no 32 GB ceiling: HotSpot's compressed oops save
  memory because a Java reference field is 32 bits, while every field here
  is a full `Value` word able to hold any value, so the offset can be as
  wide as the word allows after its tag. No handle caps the number of
  objects either; the reservation does. The words in the space are 8 bytes
  in every build, so the layout a debug build tests is the layout a release
  build runs. The Rust-side types -- `ObjectReference`, and `Value` in Rust
  variables, structures, continuations and host roots -- carry the heap id
  and the move epoch in full in a debug build, so the origin guard and the
  epoch check stay exact wherever a stale handle can arise. Every store
  checks in a debug build that the stored Rust-side value's heap id and
  epoch are current before it becomes a word (`Heap::store`), so a stale
  value cannot be laundered into a field; a word read out of an object is
  given the current ones (`Heap::load`), since a full collection rewrites
  every word it walks by format and cannot miss one the way it can miss an
  unknown Rust field. A scavenge walks only the young objects and the
  remembered old ones, so an old object whose store missed the write
  barrier keeps a word naming a young object the scavenge moved or
  reclaimed; that is what `NS_NEXT_STRESS_SCAVENGE`'s check of the
  remembered set and the poison over the vacated nursery find
  ("Generations"). The one
  exception is deliberate: a married frame's inline registers are skipped,
  because while it is married its registers live on the activation stack,
  which is traced as a root.
- **The overflow word.** The header's top byte counts the words after the
  header up to 254; 255 says the count is in an overflow word *before* the
  header, whose top byte is that same 255 and whose low 56 bits are the
  count, so a walk standing at an allocation's first word tells an overflow
  word from a header, as Spur's does (`numSlotsOfAny:`, `objectAfter:`,
  `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:55584`, `:55781`).
  The space is its first use: the arena's blocks knew their own length.
  The space is walked linearly by these counts (`space::object_at`), over
  the ranges its regions occupy (`Space::occupied`: the past survivor
  space, eden and the old space, in address order), which is how
  compaction, a move, the forwarding of every object, the replacement of
  resolved futures and the debug check of the mark bits visit every object,
  so none can miss a region; the scavenger's Cheney scan and the
  remembered set's verification walk the same way over the regions they
  read.
- **Access stays behind `Heap`'s accessors**, which read the words through
  `Space::words`, the slice `[0, old top)` that `Reservation::prefix` lends in
  O(1) and that covers the nursery below the old space, so every access is
  bounds-checked against the top and a stale offset
  reads a wrong object at worst, never undefined behaviour and never memory
  outside the objects. `Heap::block` ends where the header's count says, so
  no field, element or register accessor reaches the next object's header
  -- the guarantee the arena's separate blocks gave. The access path is
  forced inline (`#[inline(always)]` on `pointer_words`, `pointer_words_mut`
  and `pointer_word`, `#[inline]` on `Reservation::prefix`): the space made
  `Heap::block` larger, LLVM then outlined `pointer_words`, and the call
  alone held 5.3% to 8.2% of a benchmark's samples
  ([execution.md](execution.md), "What the object space moved"). The
  accessors still copy out; lending an object's fields as a slice is the
  open question "Known rough edges in the seam" names.
- **Frames stay objects of the frame format** in the space, with their
  registers inline. Married and divorced frames keep their meaning
  ("Frames, the activation stack and the marriage"); a frame object is
  allocated with its code's full register count, so none ever grows, and
  whether a divorce copies the registers stays decided per activation from
  the block's `outer_reach`. The activation stack of running activations is
  unchanged until road step 7, the execution model for compiled code.
- **The full collector is mark and sliding compaction** over the variably
  sized objects of both generations, after Spur's planning compactor,
  which slides marked objects
  down and plans their new addresses before moving any (`compact`,
  `planCompactSavingForwarders`,
  `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:61596`, `:62399`).
  The mark bit is in the header ("Marking"); the forwarding addresses live
  in a side table while a collection runs, a live-word bitmap with a prefix
  count per 64 words, where Spur saves each object's first field to make
  room for one ("Compaction and forwarding"). Between full collections a
  copying scavenger collects the nursery ("Generations").
- **The weak symbol table and the replacement of resolved futures** walk
  the space and read fields by format; the inverse symbol map is keyed by
  offset ("Weak references").
- **The triggers** compare eden's occupied bytes against the scavenge
  threshold, and the old space's against a limit reset after each full
  collection, with a 512 KiB floor ("The trigger and the growth policy").
- **The move stress mode shifts every region's objects within the
  region** by `8 * (2 + epoch % 31)` bytes, `epoch` the new move epoch, up
  from the region's first offset, and back down to it at the next move, so
  every handle changes at every move and young stays young ("The move
  stress mode"). The other four stress modes are in "The gates the
  collector needs".
- **Running out of memory aborts the process.** A commit the operating
  system refuses, or a reservation used up, ends in
  `std::alloc::handle_alloc_error`, as a failed `Vec` push does, so
  allocation stays infallible and no hot path carries a `Result`
  (decided with the user on 2026-10-03). A Newspeak-level out-of-memory
  error is open ([OPEN-WORK.md](../OPEN-WORK.md)).
- **No page is protected, in either build** (decided with the user on
  2026-10-03, and kept for the nursery on 2026-10-04). The accessors lend
  only `[0, old top)`, so a stale offset at or above the top -- into the
  tail a compaction vacated -- already finds no word, and one below the top
  lands in live, compacted memory, which is what the epoch check is for.
  The nursery lies inside that slice whatever its regions hold, so a debug
  build poisons what a scavenge vacates instead ("Finding a missed
  barrier: the scavenge stress mode"). Protecting the vacated pages pays
  when code dereferences addresses itself, which is road step 8's
  generated code; it is open until then ([OPEN-WORK.md](../OPEN-WORK.md)).
- **The space belongs to a `Heap`**, never to a thread or the process,
  because an actor is meant to be a per-heap scope
  ([liveness-and-reflection.md](liveness-and-reflection.md)).

**What changes in `CLAUDE.md`, and when.** Its invariants describe what is
built, so each changes when its road step lands, not before. Road steps 1,
3a, 3b and 4 are done: the crate list names `crates/ns-memory`, the one crate
with `unsafe`, and the heap's space in its reservation, a nursery of eden
and two survivor spaces below the old space, a Cheney scavenger beside the
mark-and-slide collector, which compacts both generations, and the barrier
in `heap.rs`'s store accessors; "the crate forbids
unsafe code" names `ns-runtime`; "An image is untrusted input" states the
figures of "The amplification bound of a rejected load" above, and that
the decoder allocates only after a tenure between entries, the
platform-path sends may collect at their safepoints like any run, and the
graph pass allocates old, after the last tenure, and never polls a
safepoint, so the peak argument stands; the handle
paragraphs describe a 64-bit byte offset and name no frame arena; "A
cache records what the lookup decided" reads the builtin tag as a field of
the class object, which `act` still reads afresh on every send; the end-of-batch
gates name five stress modes, `SCAVENGE` among them; and "Every store into
an existing object goes through `heap.rs`'s store accessors and their write
barrier" is an invariant of its own. Still to
come: "The
VM mirror is a capability" gains, as a consequence, that no program reaches
`become:` -- a leaf on the mirror that only the shipped mirrors module's
atomic install uses -- and that the mirrors refuse every edit to a shipped
mixin, which seals the mirror's class and keeps every object a shipped name
names out of the forward (road step 5; [object-model.md](object-model.md),
"Planned: `become:` as an eager bulk forward"). "Decided per activation from the
block's `outer_reach`" stands, since the divorce keeps that rule.

## Generations

Built by road step 4 on 2026-10-04
([its plan](../superpowers/plans/2026-10-04-2030-ns-next-generations.md)).
The space has a young generation, the nursery, below the old space. A
copying scavenger collects the nursery alone, at a cost in proportion to
what survives; the mark-compact of "The shape: mark, compact, forward" is
the full collection, over both generations. The code is `heap/space.rs`
(the regions and their constants), `heap/scavenge.rs` (the scavenger and
the tenuring threshold), `heap.rs` (the barrier, the remembered set, the
triggers, `Heap::scavenge` and `Heap::compact`), `heap/world.rs`
(`World::scavenge`, `StepRoots`) and `interpreter/safepoint.rs` (the
safepoint that serves a request, `Interpreter::tenure_now` and the scavenge
stress mode). What it measured is in [execution.md](execution.md), "What
generations moved".

### The layout and its constants

- **Four regions, low to high**: two survivor spaces of 1 MiB each
  (`SURVIVOR_BYTES`), eden of 8 MiB (`EDEN_BYTES`, from `EDEN_START`,
  2 MiB), then the old space from `NURSERY_END`, 10 MiB, to the end of the
  reservation. Eden is eight times a survivor space, HotSpot's
  `SurvivorRatio=8` (decision 3 below). The survivor spaces lie first and
  the old space above the nursery, as Spur lays its heap out
  (`setHeapBase:memoryLimit:endOfMemory:`,
  `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:60020`) and
  Strongtalk its generations
  (`prior-art/strongtalk/vm/memory/universe.cpp:64-79`).
  The first survivor space starts at `FIRST_OBJECT`, so it is 16 bytes
  smaller than the second. `NURSERY_END` is a multiple of 64 words, so no
  chunk of the forwarding table straddles the two generations.
- **Young means an offset below `NURSERY_END`** (`space::is_young`): one
  comparison against a constant, in the barrier, in the scavenger and
  wherever else a generation is asked, and the comparison generated code
  will make.
- **Each region keeps its base, start, top and end** (`space::Region`).
  One survivor space, the past one, holds the last scavenge's survivors;
  the other, the future one, is empty outside a scavenge, which copies into
  it and then makes it the past one (`Space::flip_survivors`). The ranges
  that hold objects are the past survivor space's, eden's and the old
  space's, in address order (`Space::occupied`), and every walk of the
  space reads them, so none can miss a region. Every object is read
  through one slice, `[0, old top)`, which covers the nursery.
- **Every nursery region keeps 264 bytes free past what it fills**
  (`REGION_SLACK`, `8 * 33`), so that the move stress mode's shift of up
  to 256 bytes never pushes an object out of its region (chosen by the
  plan; [OPEN-WORK.md](../OPEN-WORK.md)).
- **The nursery is committed whole when the heap is made**, with the old
  space's first 256 KiB (chosen by the plan); the old space grows as "The
  trigger and the growth policy" says.

### Allocation: eden, its reserve, and what goes old

- **A young allocation bumps eden** (`Space::allocate_young`). Eden asks
  for a scavenge once it holds 7.5 MiB, its 8 MiB less a 512 KiB reserve
  (`EDEN_RESERVE`, chosen by the plan): the reserve takes what a stretch
  of the run loop allocates before its safepoint serves the request, as
  Spur's `scavengeThreshold` lies below eden's end and an allocation past
  it only schedules the scavenge (`ceScheduleScavengeTrampoline`).
- **An allocation that finds eden spent goes old** and asks for the
  scavenge, as Strongtalk's `allocate_tenured` does when a scavenge cannot
  run (`prior-art/strongtalk/vm/memory/universe_more.cpp:55-64`).
- **Large objects are allocated old**: an allocation of 32 KiB or more
  (`LARGE_OBJECT_BYTES`), Primordial Soup's `kLargeAllocationSize`
  (`prior-art/primordialsoup/vm/heap.h:111`), since copying it through the
  survivor spaces would buy nothing (decision 3).
- **Every old allocation outside the image decoder is born remembered.**
  A freshly allocated object is filled before anything else can see it, so
  its initializing stores take no barrier -- the fields `allocate_block`
  writes, a closure's copied values, a self-referential class word -- and
  an old one may be filled with young values. `allocate_block` therefore
  adds every object it allocates old to the remembered set, and the next
  scavenge scans it. The decoder's graph pass allocates old
  (`Heap::set_allocating_old`) and its objects are not remembered: the load
  tenured everything they name before the pass began, which a debug build
  asserts at every fill.

### The scavenger

A Cheney copy of the nursery's survivors (`heap/scavenge.rs`), as Spur's
`doScavenge:` (`cointerp.c:48477`) and Strongtalk's `Universe::scavenge`
(`universe_more.cpp:81-137`) are.

- **The roots** are what a full collection marks from -- the `Activation`
  class, every activation stack entry and every register, the host roots,
  and the step state's `StepRoots` -- plus the remembered set. The whole
  activation stack is a root, so a scavenge costs time in proportion to the
  recursion depth, up to the depth limit's 100,000 activations and its
  1,000 reserve; Spur bounds that with stack pages, and road step 7's
  shared frames revisit it. Each remembered object leaves the set, its bit
  cleared, has its pointer words scanned, and rejoins only if it still
  names a young object.
- **The Cheney loop** then scans what was copied, in the future survivor
  space and in the old space from where each top stood when the scavenge
  began, until neither grew; a promoted object that still names a young
  one joins the remembered set.
- **Forwarders.** A copied object's old place becomes a forwarder: its
  header's format the forwarded one, its class word naming the copy, the
  format road step 5's bulk forward writes too, while it runs
  ([object-model.md](object-model.md), "Planned: `become:` as an eager bulk
  forward"). A reference to a forwarder is followed along the whole chain
  (`evacuate`), and the object at its end is copied only if it is young and
  not already in the future survivor space, as Spur's
  `scavengeReferentsOf:` decides (`cointerp.c:43278`, the
  `isReallyYoungObject:` test at `:43316-43322`). Since the bulk forward
  leaves no forwarder behind, every chain a scavenge meets is one link of
  its own. The copy takes the whole allocation, its overflow word
  included.
- **Where a survivor goes.** To the future survivor space, its age one
  higher, or to the old space, its age 0, when the scavenge tenures
  everything, when its age is at least the tenuring threshold, or when the
  survivor space is full.
- **The precommit.** Before it copies anything a scavenge commits the old
  space for the worst case, every object in the past survivor space and in
  eden promoted, so a promotion never meets a refused commit halfway
  through; a commit the operating system refuses aborts the process as
  running out of memory before the scavenge begins.
- **Afterwards** the weak symbol table is pruned ("Weak references"), the
  survivor spaces flip, eden is empty, the threshold is recomputed, and the
  request is cleared. A rebuilt remembered set past its limit requests a
  scavenge that tenures everything, and an old space past its limit
  requests a full collection.
- **The epoch advances**, as at a move or a compaction, and the scavenger
  rewrites every Rust-side handle it traces into the new epoch, old ones
  included, after checking its heap and its epoch as `Forwarder` does.
- **A scavenge does not replace resolved futures**, which stays the full
  collection's work ("A resolved future is traced as its value"; chosen by
  the plan).

`World::scavenge` checks that the step state holds no handle outside its
roots (`World::check_quiescent_state`), lends the heap and `StepRoots` side
by side, and times the scavenge as `World::collect` times a collection;
`--collector-statistics` reports the scavenges and their pauses beside the
full collections ("The gates the collector needs").

### The age table and the tenuring threshold

An object's age is the header's four age bits, 27-30, so at most 15
(`MAX_AGE`): the scavenges it has survived in a survivor space. A promoted
object's age is 0, and a full collection clears every survivor's.

Each scavenge counts, in its age table, the words it copies into the
survivor space by the age each object arrives with, one higher than the
age it had -- Strongtalk's age table (`ageTable.cpp`). Afterwards the
threshold becomes the lowest age at which the words of the survivors of
that age and younger exceed half a survivor space, less its slack; never
below 1, and 15 when no age does (`scavenge::tenuring_threshold`). The next
scavenge promotes an object whose age is at least the threshold. The first
threshold is 15, the widest the age bits hold, so nothing is tenured by
age before a scavenge has measured the survivors. That is HotSpot's
`ageTable::compute_tenuring_threshold` with `TargetSurvivorRatio=50` and
`MaxTenuringThreshold=15` (decision 3). The age table is Strongtalk's,
which HotSpot took over; Strongtalk's own `ageTable::tenuring_threshold`
(`prior-art/strongtalk/vm/memory/ageTable.cpp:37-46`) answers one age less
and so tenures the age that would just have fitted as well. HotSpot's rule
is the one decision 3 states, and the scavenger's own test pins it.

### The write barrier

An old object that comes to name a young one sets its remembered bit and
joins the remembered set (`Heap::write_barrier`, then `Heap::remember`),
per object, as Spur's `storePointer:ofObject:withValue:`
(`cointerp.c:60688`; `remember:`, `:42843`) and Primordial Soup's
`HeapObject::Store` (`prior-art/primordialsoup/vm/object.h:257-273`)
remember, rather than Strongtalk's cards: simpler, and cheaper to emit. The
test is two comparisons against `NURSERY_END` -- the holder is old, the
stored word names a young object -- and the barrier is forced inline into
each store accessor; `remember` is out of line, and a holder whose bit is
set already returns at once.

The barrier sits in `heap.rs`'s store accessors and nowhere else, never in
`vm.rs`, so the interpreter, the reference lowering and a JIT share one
rule. Every store into an existing object goes through one of them:

- `Heap::set_pointer_word`: an instance's fields, an array's elements and a
  class object's fields;
- `Heap::object_set_class`: the class word;
- `Heap::frame_set_local`: a divorced frame's register, which
  `StoreOuterLocal` writes through its frame;
- `Heap::pop_activation`: the divorce, which copies a retaining
  activation's registers into its frame object and remembers an old frame
  that one of them makes name a young object ("Frames, the activation
  stack and the marriage").

`allocate_block` remembers every old object it allocates outside the
decoder, as above. A married frame's registers live on the activation
stack, which is a root, and take no barrier; nor does a store into a young
object. The collector's own rewrites -- the scavenge, the compaction, the
move -- keep the remembered set themselves; the replacement of resolved
futures, which may store a young value into an old object, runs only
before a full collection, which empties the nursery and the set; and the
decoder's fills store old values into old objects. The debug check of the
stored Rust-side value's
heap id and epoch, which every store makes (`Heap::store`; "The object
space"), holds here too. A store that writes a word into an old object any
other way leaves the young object it names unreachable to the next
scavenge, silently; that is the invariant `CLAUDE.md` states, and the
scavenge stress mode below is what finds a breach.

### The remembered set and its limit

`Heap::remembered` holds the offset of every old object that may name a
young one, each once, its header's remembered bit, bit 31, set; a young
object never carries the bit. A scavenge rebuilds it, a full collection
empties it and clears every bit, and a move relocates every offset through
the same shift, since no tracer reaches a raw offset. Past 8,192 entries
(`REMEMBERED_SET_LIMIT`), Primordial Soup's `kRememberedSetOverflowSize`
(`heap.h:112`, `heap.cc:279-283`), the barrier requests a scavenge that
tenures everything, at the next safepoint, and the set keeps growing until
then; a scavenge whose rebuilt set is past the limit requests one as well.
A scavenge that tenures everything empties the nursery, and the set with
it.

### The full collection over both generations

The full collection marks both generations from the roots and compacts
them into the old space (decision 2): one forwarding table covers both, the
old space's survivors slide down to its bottom, the nursery's follow them
in address order, and every handle is rewritten in one pass ("Compaction
and forwarding"). The nursery is empty afterwards, the remembered set with
it, and a pending scavenge is served too. A full collection is requested
when the old space reaches its limit -- by an allocation that goes old, or
by a scavenge that promoted -- and the safepoint serves it before a pending
scavenge.

### An image load tenures once

Every load bootstraps, which builds the whole shipped world, before it
decodes the graph, so every object an image can name is young when the
graph pass starts; the first phase, which reads everything up to the
graph and resolves the shipped names, allocates none. One scavenge that
tenures everything, between two entries
(`Interpreter::tenure_now`), makes the shipped world old, and the graph pass then
allocates old, its stores old-to-old (decision 4;
[image-format.md](image-format.md), "The image under generations").

### Finding a missed barrier: the scavenge stress mode

`NS_NEXT_STRESS_SCAVENGE`, the fifth stress mode, scavenges at the
safepoints its schedule picks -- every safepoint while the traced size is
within `STRESS_MOVE_BUDGET`, as the move and the collect modes are paced
(`StressSchedule`, `interpreter/safepoint.rs`) -- and verifies the
remembered set before each of them (`Heap::verify_remembered_set`): a walk
of the old space asserting that the set holds no offset twice, that every
entry is the header of an old object carrying the remembered bit, that
every old object carrying the bit is an entry, and that every old object
naming a young one carries the bit.
The verification runs only under the stress mode, since it walks the old
space at every scavenge (chosen by the plan). It is Strongtalk's
`VerifyBeforeScavenge` (`universe_more.cpp:98`) and HotSpot's
`VerifyBeforeGC`.

Every scavenge in a debug build then fills the nursery memory it vacated --
the past survivor space's and eden's occupied ranges -- with a poison word
whose format is the free one, which no object has (`header::POISON`), and
`Heap::block` refuses a free or a forwarded header in a debug build, so a
word that missed the barrier and still names a reclaimed young object
fails at its first read instead of answering whatever is allocated there
next, as HotSpot's `ZapUnusedHeapArea` does (decision 1). Under the stress
mode the whole suite runs at the highest scavenge frequency there is.

### Decisions of road step 4

Decided with the user on 2026-10-04:

1. **A missed write barrier is found by verification and poisoning, not by
   page protection.** The verification before every stress scavenge
   (Strongtalk's `VerifyBeforeScavenge`, HotSpot's `VerifyBeforeGC`) finds
   the old object that missed the barrier before the scavenge that would
   lose its young object; the poison (HotSpot's `ZapUnusedHeapArea`) finds
   a word that still names vacated nursery memory at its first read. No
   page is protected: `Space::words` lends a safe slice over `[0, old top)`,
   which covers the nursery, and a `&[u64]` over no-access pages would be
   undefined behaviour. Rejected: alternating eden between two regions and
   making the vacated one no-access, for that reason; the decision of
   2026-10-03 that nothing is protected stands ("The object space").
2. **The full collection marks both generations and compacts them into the
   old space**, as Strongtalk's full collection does -- the old generation
   compacted first, then the new one into it
   (`prior-art/strongtalk/vm/memory/markSweep.cpp:255`, `:265-266`) -- and as
   HotSpot's Serial and Parallel collectors do. One forwarding table covers
   both generations, so every handle is rewritten once. Rejected: Spur's
   mark, then scavenge-and-tenure, then compact the old space (`fullGC`,
   `cointerp.c:50168`), which relocates twice per full collection; and
   tenure everything, then mark and compact the old space, which keeps
   young garbage reachable only from dead remembered objects alive until
   the next full collection.
3. **The nursery's sizes follow HotSpot's ratio**: eden 8 MiB and two
   survivor spaces of 1 MiB each (`SurvivorRatio=8`), 10 MiB of nursery
   from the reservation's base. Objects of 32 KiB and more are allocated
   old (Primordial Soup's `kLargeAllocationSize`). A remembered set past
   8,192 entries requests a scavenge that tenures everything (Soup's
   `kRememberedSetOverflowSize`). The tenuring threshold adapts by
   Strongtalk's age table, which HotSpot took over: it starts at 15, the
   widest the header's four age bits hold, so nothing is tenured by age at
   first, and after each scavenge it is the lowest age at which the
   survivors of that age and younger exceed half a survivor space
   (HotSpot's `TargetSurvivorRatio=50`, `MaxTenuringThreshold=15`). The
   sizes are measured in [execution.md](execution.md), "What generations
   moved"; what a smaller eden would trade is open
   ([OPEN-WORK.md](../OPEN-WORK.md)).
4. **An image load tenures once, between the shipped-name section and the
   graph pass.** Spur, Primordial Soup, Strongtalk and HotSpot's CDS all
   load into a heap that holds nothing young; here the bootstrap's objects,
   the whole shipped world among them, are young when the graph pass
   starts, and one tenure between two entries makes them old. The decoder
   then allocates its graph old, as Soup's `AllocateSnapshot` does
   (`prior-art/primordialsoup/vm/heap.cc:228`). A second tenure right after
   the bootstrap would be the same one: the first phase, which decodes up to
   the graph and resolves the shipped names, allocates nothing.

The plan also chose constants for the user to confirm, each one line to
change and each stated where it is built above: eden's 512 KiB reserve,
the eden-spent rule, `REGION_SLACK`, the old space's first commit past the
nursery, a scavenge that leaves resolved futures alone, and the
remembered-set verification under the stress mode only
([OPEN-WORK.md](../OPEN-WORK.md)).

## Planned: inline allocation

Would change: the instructions that allocate take their memory by a bump in
eden instead of a call into the collector's allocation path
(`Heap::allocate_block`). Road step 6, and
the groundwork the JIT tiers take over, as Cogit's `genPrimitiveNew` and
`genCreateFullClosure:…` do in machine code (`cogitX64SysV.c:16464`,
`:19026`).

- **What bumps**: `new` of a class whose instances fall below the
  large-object threshold, `MakeClosure`, a boxed
  `Float`, and `Array new:` below the large-object threshold; a larger
  `Array new:` allocates old, as every large object does ("Generations").
- **What it builds on is built** ("Generations", "Allocation: eden, its
  reserve, and what goes old"): eden keeps a 512 KiB reserve past its
  scavenge threshold, so an allocation that crosses the threshold still
  succeeds and only requests a safepoint, as Spur schedules a scavenge
  rather than failing (`ceScheduleScavengeTrampoline`), and a collection
  therefore still happens only at a safepoint of the run loop ("Where a
  collection can happen: only at a safepoint in the run loop"); an
  allocation that finds eden spent goes old, born remembered, and requests
  the scavenge, as Strongtalk's `allocate_tenured` does when a scavenge
  cannot run (`prior-art/strongtalk/vm/memory/universe_more.cpp:55-64`).
  Between two safepoints the allocations are bounded by a stretch's
  straight-line instructions times the large-object threshold, but no
  fixed reserve fits every body, which is why the spent case goes old
  rather than fails. The inlined bump keeps both rules, and generated code
  will allocate the same way.
- **The bump path lives in `heap.rs`**; an instruction arm calls it and does
  nothing else with it, since one more calling arm measures about 3% on the
  send-heavy sources ([execution.md](execution.md), "Where the 3% goes").

## Decisions

- **Identity hashes come from a counter, not an address, and are assigned on
  first request**, so identity survives moving and snapshotting — except, for
  now, the hash of an object an image names by tag rather than by record
  ([image-format.md](image-format.md), "Identity hashes are preserved
  verbatim").
- **A handle is a space-relative offset, not a globally unique name**, and a
  `debug_assertions`-only origin guard says so at every accessor rather than
  leaving it to convention. It exists because the failure it catches is silent:
  a handle from another heap is in range and names a valid object, so the read
  succeeds with the wrong answer. It costs nothing in a release build, where the
  origin field does not exist. An `ActivationReference` adds a generation to the same
  guard, for the same reason: a stack index is reused once its entry is popped.
- **Activations live on a stack, not in the object space**, so that what nothing
  captures is not retained. Only a reified frame reaches the space, and it is
  the one activation form anything may store.
- **A moving collector, not a conservative one**, because `become:` and instance
  migration are requirements. Conservative scanning with pinning was considered
  and rejected on the same grounds.
- **Iterative `Drop` rather than a depth ceiling** for `MixinApplication`'s
  chain, because that depth is bounded by nothing the front end enforces, so a
  ceiling would exclude valid programs. Where a depth *is* independently
  bounded — the `Mixin` declaration tree, capped by the parser — a ceiling is
  the right answer and `image/mixins.rs` uses one. Objects and activations need
  neither: an object's words hold offsets, so there is nothing to unwind.
- **A collection happens only at a safepoint in the run loop, or between
  entries where only roots hold values** (as `Interpreter::collect_now` does
  for tests), never inside an allocation (decided 2026-09-13), so no
  allocating Rust path roots anything and a JIT keeps its freedom across
  allocations. Cog, Nova, CPython and piccolo put it there; V8 and HotSpot's
  runtime pay for the alternative with handles and verifiers.
- **No Rust frame is live below a safepoint**: nothing re-enters the run loop
  from Rust, so there is nothing to root but what the host keeps. Chosen over
  rooting those frames with Nova's `Scoped` values, because the re-entries were
  to go anyway, and have (step 2a).
- **The root discipline is enforced by the type system**, not by convention: a
  generative lifetime brand, gc-arena's rather than Nova's scopes, with no
  public `unbind`. The ergonomic cost is accepted rather than traded away.
- **The brand needs no cast** (decided 2026-09-14): the step state rests as a
  `Mutator<'static>` and is handed to a closure generic over `'gc` as it is,
  and the crate forbids `unsafe` code. A safe conversion per payload was
  rejected for its reallocations, and an audited cast, chosen in its place,
  turned out to add nothing the higher-ranked closure does not already give.
- **The roots' completeness is checked by moving everything**, in a debug-build
  stress mode decided and built before the collector existed, because the
  brand cannot see a handle stored somewhere the tracer does not know about.
- **The stress mode moves on a budget paced by heap size**, not at literally
  every safepoint (decided with the user on 2026-09-13): at every safepoint
  while the traced size is within `STRESS_MOVE_BUDGET`, and above it once per
  size-over-budget safepoints. A move at every safepoint is quadratic in a deep
  recursion, and a fixed interval, as HotSpot, SpiderMonkey and V8 use, would
  take coverage from small programs to spare large ones.
- **A host keeps values across runs only as `Root`s**, slots it shares with the
  heap's root table (gc-arena's `DynamicRootSet`), so the tracer finds and
  rewrites every value a host holds and releasing one needs no heap.
- **A closure's state is inline in the closure object**, not behind an `Rc`
  of its own, and everything else that needs a closure names the object: a
  handle behind a shared `Rc` cannot be rewritten in place. Only its code,
  which holds no handle, stays shared, named by a code id into the heap's
  table of compiled code. It also made `bench_block.ns`
  about 4% faster, since `MakeClosure` no longer allocates an `Rc` and a popped
  block activation drops none.
- **Every object is one block of words, traced by its format, and a reified
  frame is an object** (road step 3a, decided with the user on 2026-09-29 and
  2026-10-03, built on 2026-10-03; [object-model.md](object-model.md), "The
  object layout"): an object is the header word, the class word and the
  fields, there is no separate store for frames, and one mark, one
  compaction and one forwarding table cover everything.
- **Mark-compact, stop-the-world** (decided with the user on
  2026-09-20 over the index arena, kept for the object space). Under
  generations it is the full collection, over both generations
  ("Generations"). The choice rested on what the step is for — the heap
  shrinks and the pages go back — on rewriting being O(1) per handle
  through a forwarding table, and on the machinery already built and under
  test; over the space's variably sized objects a sliding compaction also
  leaves none of the fragmentation a free list would have to manage. Nova
  compacts; Strongtalk, Spur and HotSpot compact or evacuate their old
  generation, and HotSpot removed the one free-list old space it had.
- **MMTk is ruled out by what it would replace**, not by a comparison of
  collectors. Its handles are raw addresses, so adopting it would replace the
  checked access, the origin guards, the epoch check and the brand's proof along
  with the collector. Owning the memory is not what costs them: the object
  space owns its memory and keeps all four, because its handles are byte
  offsets read through bounds-checked slices ("The object space").
- **`unsafe` is acceptable on a performance-critical path where the abstraction
  over it is safe again** (decided with the user on 2026-09-21), so an option is
  not dismissed for needing `unsafe` alone -- the argument has to be about what
  the option dismantles.
- **The `unsafe` the object space needs lives in a crate of its own**,
  `crates/ns-memory` (decided with the user on 2026-09-29), and
  `ns-runtime` keeps `#![forbid(unsafe_code)]`: a crate boundary is one the compiler checks,
  where a crate-level `deny` with a scoped `allow` admits the next `allow`
  without anyone noticing. The executable memory and the entry into generated
  code of the JIT tiers are to go to the same crate. Its operating-system calls
  come from `windows-sys` and `libc`, declaration-only crates, rather than
  hand-written declarations whose constants differ per platform and would be
  wrong only on the platforms nobody here runs.
- **The canonical symbol table is the system's only weak structure.** §5.1.6
  survives it because `Mutator::new_symbol` is the only way to make a symbol
  object, so one that is collected was held by nothing.
- **The application cache stays strong until live editing** (decided with
  the user on 2026-10-03). Its key was an address, which needs the strong
  reference to keep it unique; since road step 3a it is a pair of the heap's
  ids, so a weak cache is decidable, but nothing drops a mixin before live
  editing, so the ids are minted and none is reclaimed
  ([object-model.md](object-model.md), "An application has two ids").
- **A collection is requested by comparing the bytes the objects occupy
  against a limit** -- eden's against the scavenge threshold, the old
  space's against a limit reset to `max(512 KiB, live bytes * 2)` after
  each full collection -- rather than by counting allocations: the region's
  top is already in hand where an allocation happens, so the hot path pays
  one predictable branch and carries no new state.
- **Collecting at every safepoint is a stress mode and a gate of its own**,
  `NS_NEXT_STRESS_COLLECT`, kept beside the move stress rather than replacing
  it, because moving everything without liveness is what separates a missing
  root from a mistake in marking; scavenging at every safepoint,
  `NS_NEXT_STRESS_SCAVENGE`, is another, because a scavenge reads only the
  young objects and the remembered ones and so is the one collection a
  missed barrier breaks.
- **Fields go inline after the header word and the class word, and the
  objects into a space the heap owns, not payloads into a region beside the
  arena** (decided with the user on 2026-09-29; the fields inline built by
  road step 3a, the space by road step 3b). An allocator
  swap measured what the allocator calls cost ("What owning the memory
  gave"); what the address-based engines gain by
  owning their memory -- one load per field, a nursery where death is free,
  allocation a JIT can inline -- needs the fields inline, which a payload region
  would not give; the space gives the one load, and the nursery and the
  inline allocation build on it (road steps 4 and 6). The memory layer, the eight-byte value, the object space,
  generations, mirrors and atomic installation over an eager `become:`, and
  inline allocation are steps 1 to 6 of the road in
  [OPEN-WORK.md](../OPEN-WORK.md), "The road to a high-performance VM".
- **One reservation per heap, contiguous and committed as it grows**
  (built: `Reservation`, which holds each heap's object space), because
  a handle is an offset from a fixed base, so the base cannot move and the
  space cannot be scattered; for Windows and POSIX from the start (decided
  with the user on 2026-09-29).
- **The forwarding table is a live-word bitmap with a prefix count per 64
  words** (decided with the user on 2026-10-03, built by road step 3b;
  "Compaction and forwarding"): one bit per word of the space for every word
  of a surviving object, and per chunk of 64 words the live words before
  it, so a survivor's new offset is one table entry, a mask and a
  `popcount` away -- O(1) per handle, one cache line per lookup, about 3% of
  the space while a collection runs. Rejected: a sorted list of old and new
  offsets with a binary search per handle, O(log n) dependent loads, the
  cost Nova's `CompactionList` pays
  (`prior-art/nova/nova_vm/src/heap/heap_bits.rs:1345`, `:1457`); and a dense
  table indexed by offset over 16, one load but half the space again in
  memory and a cache miss per handle on any heap larger than the cache.
- **No page is protected yet; a debug build's no-access protection of
  vacated pages waits for generated code** (decided with the user on
  2026-10-03). The accessors lend only `[0, old top)`, so a stale offset at or
  above the top already finds no word; the protection pays when code
  dereferences addresses itself, which is road step 8's baseline JIT
  ([OPEN-WORK.md](../OPEN-WORK.md)).
- **A missed write barrier is found by verification and poisoning, not by
  page protection** (decided with the user on 2026-10-04): the scavenge
  stress mode verifies the remembered set before each scavenge it runs, as
  Strongtalk's `VerifyBeforeScavenge` and HotSpot's `VerifyBeforeGC` do,
  and every debug-build scavenge poisons the nursery it vacated, as
  HotSpot's `ZapUnusedHeapArea` does, because the one slice the heap reads
  through covers the nursery and a slice over no-access pages would be
  undefined behaviour ("Generations", "Decisions of road step 4").
- **The full collection marks both generations and compacts them into the
  old space** (decided with the user on 2026-10-04), as Strongtalk's and
  HotSpot's Serial and Parallel collectors do: one forwarding table, one
  relocation per survivor. Spur's mark, scavenge-and-tenure, then compact
  relocates twice, and tenuring everything before a mark-compact keeps
  young garbage alive that only dead remembered objects reached.
- **The nursery's sizes follow HotSpot's ratio** (decided with the user on
  2026-10-04): eden 8 MiB, two survivor spaces of 1 MiB; objects of 32 KiB
  and more allocated old and a remembered set past 8,192 entries tenuring
  everything, both Primordial Soup's limits; and a tenuring threshold that
  adapts by Strongtalk's age table under HotSpot's rule, starting at 15.
- **An image load tenures once, between the shipped-name section and the
  graph pass** (decided with the user on 2026-10-04), so that the graph pass
  allocates old into a heap that holds nothing young, as every reference
  loads; nothing between the bootstrap and the graph pass allocates, so the
  one tenure covers the bootstrap's objects.
- **A refused commit or an exhausted reservation aborts the process**, as a
  failed `Vec` push does, through `std::alloc::handle_alloc_error` (decided
  with the user on 2026-10-03). Allocation stays infallible, so no hot path
  carries a `Result`; a Newspeak-level out-of-memory error is open
  ([OPEN-WORK.md](../OPEN-WORK.md)).
