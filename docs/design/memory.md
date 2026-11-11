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
> safepoint is a compile error (step 2c). The canonical symbol table, the
> table of the symbols a program made, which take no number and die with
> their objects, a
> `WeakArray`'s slots and the fields of an `Ephemeron` whose key nothing
> else reaches are weak, and the table of reified mixins is weak in a full
> collection, which a scavenge traces strongly; everything else the
> collector traces is strong ("Weak references"). Beside the space, a full
> collection gives back the per-heap ids nothing live names any more --
> applications, guard ids, mixin trees, compiled bodies, cache ranges --
> gathered while marking ("Marking"; [object-model.md](object-model.md),
> "An application has two ids"). Every such table has a growth limit
> that requests a full collection, so that a program that mints rows
> without allocating much still collects, and after every full collection
> the reservations beside the space -- the activation stack, the cells,
> the two id tables of machine code, the dispatching rows and the code
> zone -- give back what a
> peak committed, by a band and a damping ("The trigger and the growth
> policy", "Giving committed memory back").
> Allocation is inline (road step 6, "Inline allocation"): a young object
> whose header holds its word count is one comparison against a limit
> below eden's scavenge threshold, a bump of the cursor `ns-memory`'s
> reservation holds, and its header and class word written; every other
> allocation takes the cold path, `Heap::allocate_block`. Machine code
> bumps the same cursor inline and makes the write barrier's decision
> inline, calling a leaf only for what the cold path and `remember` do
> ("The fast path", "The write barrier").
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the
> handle types and their brand, every accessor and allocation, the write
> barrier and the remembered set, the host root table, the move, the
> triggers, the scavenge, marking, compaction and forwarding),
> `heap/space.rs` (the object space: the reservation, its four regions,
> eden's fast limit and fast bump, the cold bumps, the overflow word and
> the walk), `heap/scavenge.rs` (the
> scavenger and the tenuring threshold), `heap/compaction.rs` (the
> live-word bitmap a collection forwards through, and the move's shift),
> `heap/object.rs` (the block of words an object is, `pointer_ranges`,
> which names its pointer words by its format, `strong_pointer_ranges`,
> those of them that keep an object alive, `trace_block`, and
> `realign`, which moves a `ByteArray`'s data to its block's alignment),
> `heap/header.rs` (the header word),
> `heap/records.rs` (the seam's value types), `heap/tables.rs` (the per-heap
> tables of mixins, applications and code, and a body's `CodeIdCache`),
> `heap/mark.rs` (the marker's work list), `heap/limits.rs` (the growth
> limits of the tables beside the space, and the band and damping by
> which each reservation beside it gives memory back), `heap/symbols.rs`
> (the weak table of the symbols a program makes), `heap/machine_entries.rs`
> (the machine entry table, whose rows a full collection's free of a code
> id clears and whose committed rows it gives back),
> `heap/optimization_budgets.rs` (each body's optimization budget by its
> code id, which a full collection carries over to the id the body has
> after it and whose rows past the code table it gives back;
> [execution.md](execution.md), "The optimizing tier's foundation"),
> `heap/optimized_copies.rs` (each method body's dispatching rows by its
> code id, whose blocks past the code table a full collection gives
> back; [execution.md](execution.md), "Speculation"),
> `heap/world.rs` (the step
> state at rest, the entry that lends it out, `StepRoots`, `World::collect`
> and `World::scavenge`),
> `interpreter/mod.rs` (`Mutator`, and `Interpreter::enter` with the
> doctests that pin the brand), `trace.rs` (every root the collector
> traces, marking, forwarding and scavenging alike, in every build),
> `interpreter/safepoint.rs`
> (the safepoint, `Interpreter::tenure_now`, `Interpreter::collect_now` and
> the switches of the memory-facing stress modes: moves, caches, collection,
> scavenges, every instruction wide, suspensions and the code zone's
> compaction; the JIT's, the exits', the optimizer's and the
> deoptimizations' are in `jit/mod.rs`, which also
> reads the zone's to force the JIT on),
> `vm.rs` (one entry per stretch between two safepoints, and the poll),
> `heap/value.rs` (`Value`, and `Word`, what a heap object stores),
> `mixin.rs` (layout, and the one remaining iterative
> `Drop`), and the memory layer's `crates/ns-memory/src/lib.rs`
> (`Reservation`, and the bump cursor it holds, `Reservation::bump`),
> `page_map.rs`, `code_zone.rs` (the JIT's code zone and the entry into
> machine code) and `platform/{mod,windows,posix}.rs`.

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
ActivationReference { offset: u32 }          // likewise, a frame's word offset on the activation stack

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
compares a generation, because a stack offset is reused as soon as its frame
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
reservation and the activation stack's in one call each.
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
below lists. A send's arguments are read in its slots, on the activation
stack, while it is decided and pushed ([execution.md](execution.md), "No
re-entry from Rust"), so the step state keeps no argument buffer that
would have to be empty there.

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
compare with zero of the request word the heap owns
(`Heap::safepoint_requested`), a word of its machine words, which machine
code's poll reads as well and which therefore lives apart from the heap's
own bytes ([execution.md](execution.md), "The machine words"). The
machine words are `AtomicU64`s behind an `Arc`, so that an attached
debugger's agent can arm the request from its own thread; the poll's load
is `Relaxed`, the plain load it was before, so the poll costs what it
did. The request word is a set of bits, each set by a `fetch_or` and
cleared by a `fetch_and` so that no writer erases another's: the VM's own
request (`REAL_REQUEST`, which a collection's, a scavenge's and a full
code zone's trigger set through `Heap::request_safepoint`), a stress
mode's hold (`STRESS_HOLD`), and the debugger's pause and commands
(`DEBUGGER_PAUSE`, `DEBUGGER_COMMANDS`; [debugger.md](debugger.md), "The
poll words as atomics"). The poll takes the safepoint for any of them;
every bit but the hold's lowers the depth limit machine code's prologues
compare with to 0 while it stands (`LOWERS_DEPTH_LIMIT`,
`Heap::synchronize_depth_limit`; [execution.md](execution.md), "Polls and
the depth check"). The same area holds the other words machine code reads
between two leaf calls: the depth count and its limit, the activation
stack's ordinary end, each immediate's class and guard, the generation
counter of a debug build, the receiver and class handed between machine
code and a leaf, and the application id table's address and bound ("The
application id table" below). Beside it, and for the same reason, the
entry block machine code is entered with (`ns_memory::EntryBlock`) is
`Cell`s behind an `Rc` of its own, held by the JIT's state outside the
mutator: its lasting words -- the object space's, the activation stack's
and the machine words' addresses, eden's bump cursor's address, the
stubs, the cells, the global cache's mask -- are written when
`JitState::mark_block_stale` says what they name changed (a zone made
or replaced, the global cache resized), not at every entry, and only a
debug build's check at every entry
catches a missed mark ([execution.md](execution.md), "The entry at a
send"). The machine words' one handle-like word is an immediate's
class, a stored word the collector does not trace, so
`Mutator::rewrite_machine_classes` writes them afresh after every
scavenge, compaction, move and bulk forward; the hand-over words are read
only right after they are written, and no collection runs between. When the flag is
set the stretch writes its frame back and leaves its entry, and `run` calls
`Interpreter::safepoint` (`interpreter/safepoint.rs`), `#[cold]` and out of
line, so `vm.rs` holds the compare and the call and nothing of what a
safepoint does. The safepoint serves an attached debugger first -- the
commands it queued, then a pause, which stops the program there
([debugger.md](debugger.md), "Serving at the safepoint") -- and then a
request -- a full collection if one
is pending, which empties the nursery too, and otherwise a pending scavenge
-- and clears its own bit and the stress hold in the same call, leaving
the debugger's bits, which only their service clears, so that no compiled push the
next turn makes finds the request's depth limit of 0 and is refused
(`Heap::synchronize_depth_limit`), unless serving it raised another
request -- the compaction of the code zone a collection's freed code
asks for, the collection a scavenge that leaves the old space past its
limit asks for -- which the next poll serves, and except under a stress
mode below, which keeps it set. The collector's triggers set it in a release build too:
`allocate_block`, which every allocation reaching eden's scavenge
threshold takes, since the fast bump's limit lies below it ("Inline
allocation"), compares eden's occupied bytes against the scavenge
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
on the send path: a request lowers the limit that `Mutator::push_above` and
`Mutator::push_on_slots` already compare the activation depth against, so a send pays nothing it does not pay
anyway.

### No Rust frame is live below a safepoint

Rust enters the run loop only as the host, its outermost caller
(`Interpreter::run_from_host`). The nested-class accessor, which once ran
Newspeak code in a nested run of the loop, is a trampoline step with a
continuation (step 2a, built); `do:`, which did too, is a Newspeak loop, as
`whileTrue:` is; and instantiation is compiled code that runs in the loop
like a method ([execution.md](execution.md), "No re-entry from Rust"). A Rust frame below a
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
builtin classes and the application caches, and
every method a step uses — heap access, allocation,
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

`ActivationReference` stays unbranded: the collector moves no stack frame, and
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
the other. Marking and the scavenger read it by way of
`strong_pointer_ranges`, which leaves out only the weak words of a
`WeakArray` and of an `Ephemeron` whose key is not yet known to survive
("Weak references"). A
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
- The activation stack, every tagged word of `[0, top)`: every frame's
  header -- its receiver, `current_class`, the closure object or class its
  kind's operand names, the frame object it is married to, and numbers
  the tracer passes over -- and its registers, with no stack map. The
  words above the top are not traced, and
  no push or pop writes a scratch register: a push writes its frame's
  header and named locals only, a pop nothing, and a push laid on its
  sender's slots never lowers the top, so a sender's registers past a
  shorter callee stay below it ([execution.md](execution.md),
  "Representation"). A word below the top may so be one a callee or a
  popped frame left, and the pre-pass below nils each before the trace
  reads it, so that it neither keeps an object alive nor names one a
  collection moved meanwhile. The trace is `Heap::trace_stack`, in
  `heap.rs`, where `ActivationStack`'s fields are private.

  Every trace of the stack -- the scavenge, marking, and every forward
  (`forward_everything`: a compaction's, a stress move's, the bulk
  forward's) -- walks the frames from the top by their caller words and,
  in a pre-pass over each frame before it traces the frame's words,
  writes `dead_register`'s word into every register the frame does not
  read before writing it. That is
  what `bytecode::leftovers` finds tainted before the program counter the
  frame's header stores ([compiler.md](compiler.md), "What a callee
  leaves"): for the top frame the instruction it runs next, which the run
  loop writes back as it leaves for the safepoint; for every other the
  instruction it stopped at, which stored its own index before it handed
  control on -- a send, which the frame resumes after, an instruction
  that met an unresolved future, which it runs again, or an instruction
  that raised an error, below whose `signal` the frame stays while the
  handler runs (`tainted_at`'s doc comment names each write). A frame
  whose callee lies on its slots stopped at that send, which taints every
  register from its slot end, so it loses as well every register from
  there to its own window's end. A
  word inside the window of a frame above is left to that frame, a word at
  or past the top to none, and a frame's arguments and header are never
  written: a callee's arguments are its sender's slots. Each body keeps
  the sets the pre-pass asked of it, by program counter
  (`leftovers::TaintedSets`), so a frame costs a lookup and its writes;
  the analysis runs once per body and program counter, at the first
  collection that finds a frame there, after a `check` of the body, since
  the shipped code is decoded unverified and an answer for a body `check`
  refuses would nil live registers. The stack is walked once: a frame
  whose set is kept is nilled as the walk meets it and then traced --
  its words but those of the windows above, which the frames above
  traced and no frame below writes -- and one whose set is missing is
  deferred with the windows above it that reach into its own; after the
  walk the missing sets are filled body by body, each body's from one
  pass, so a collection analyses a body once however many frames run
  it, and the deferred frames are nilled against the windows recorded
  for them and traced. Last, as a net, a word below the top in no
  frame's window would be traced as it lies; none is, since the bottom
  frame starts at 0 and the top is always some live frame's window end
  -- a push raises it to its frame's, a pop gives back the top its frame
  was pushed at -- and a debug build refuses one. A deferred frame costs
  the walk a `DeferredFrame` of 72 bytes, its body and program counter
  among the missing ones, 16, and at most one recorded claim, 16, since
  at most one merged window above reaches into a frame -- one not laid
  on its sender's slots starts at the top, at or past every window end
  below it, so what reaches in lies on the slots, merged: at most 104
  bytes, freed when the trace ends, no more than the 13 header words the
  frame has of its own on the stack. The first collection of a
  101,000-frame recursion, every frame deferred, holds about 10.5 MB of
  it for the length of the trace. The walk keeps the set it looked up
  last, so a frame of the same body at the same program counter as the
  frame above it -- a deep recursion's frames -- costs the reads of its
  header, its writes and its trace, no lookup. Nilling and tracing in
  one walk loads each frame's header lines once per trace: a separate
  pre-pass, a walk of its own before a linear trace of `[0, top)`,
  loaded them twice and cost a 99,000-frame recursion under about 30
  scavenges 3.4% of its run, which the one walk removes. It is the only
  guard: no push or pop writes the registers it nils. A full collection
  runs it twice, in its marking's trace and in its forwarding's, which
  is idempotent on purpose. A frame object traced as
  it lies takes no such register either: a suspension copies the frame's
  registers less those the pre-pass would nil, a divorce its named ones
  only. In a debug build the word it writes is `Word::DEAD_REGISTER`,
  which every tracer passes over and an instruction's read refuses
  ([execution.md](execution.md), "Representation").
- The side table of continuations (`Heap`'s `continuations`): each running
  activation's continuation other than `Deliver`, which its stack frame
  names by row (a frame that delivers names none), and the state it
  carries — a `NestedClassCreation`'s
  enclosing object and declaring class, an `Unwind`'s target frame and the value or error it carries,
  a `VmErrorSignalled` error's receiver, and a `Future` forcing's future and
  that of every forcing chained behind it, and a `ReturnToSuspended`'s
  suspended sender with the continuation it wraps
  ([execution.md](execution.md), "Representation"). It is a root of
  marking, of every move and of the scavenge, as the stack is.
- The side table of suspended activations (`Heap`'s `suspensions`): each
  row's frame and the state its continuation carries, as a running
  activation's ([execution.md](execution.md), "Suspending and resuming"). It is a root
  of marking, of every move and of the scavenge, because a suspended frame
  belongs to a run that has not finished, whether or not anything else
  still reaches it: a suspended chain nothing else names would otherwise
  be collected while its rows remained. The full collection's reclaim
  takes each row's entered level beside the stack's (`Heap::stack_roots`),
  so an application stays alive while an activation entered at it is
  suspended, as it does while one runs.
- Every object in the space, by its format: its class word; an instance's or
  an array's fields; a `WeakArray`'s slots and an `Ephemeron`'s fields,
  every one forwarded but traced for liveness only as "Weak references"
  says; a closure's captured frame and its copied values, never its code id
  or feedback base; a class object's enclosing object, declaring class,
  superclass, for a metaclass its class (`this_class`), and its value-class
  refusal, never its builtin tag, applied clause or application, which are
  numbers; a frame object's ten fixed fields -- sender, home, receiver,
  current class, the kind's operand among them, the rest numbers or
  constants the tracer passes over -- and its registers. A married frame's
  registers are its stack frame's, traced with the stack, so its own are
  skipped; the `Format::Frame` arm skips them only for a married word of 0
  or more, a stack offset. A suspended frame's registers are the only copy of its
  values, so they are traced like a returned frame's. The digits of a
  `LargeInteger`, a boxed `Float`'s bits, a string's code points and a
  `ByteArray`'s reserve and bytes are never handed to the tracer, since they
  would read as handles.
- The host root table.
- The kernel-private `Activation` class, every frame object's class, which
  the heap holds (`Heap::activation_class`).
- The heap's table of reified mixins (`Heap`'s `mixin_objects`), the one
  kernel-private `Mixin` object of each mixin a mirror has reached
  ([liveness-and-reflection.md](liveness-and-reflection.md), "A mixin is
  reified as a kernel-private object"): forwarded by every move and traced
  by a scavenge, which reclaims no id, but weak in a full collection,
  which prunes it ("Weak references" below) -- an object nothing else
  reaches can be compared with nothing, so `==` stays the mixin's identity
  for every comparison a program can make.
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
that captured it, through the stack frame it is married to, or through the
side-table row of its suspension. A send's arguments are read in its
slots, so nothing beside the stack holds them ("Where a collection can
happen" above). An instantiation keeps nothing
outside the activation stack: its factory and initializers are activations
like any method's ([execution.md](execution.md), "No re-entry from Rust"). A nested
class being created is marked in its own cache slot by a primitive (`false`),
while the enclosing object stays in the continuation of the clause's
activation, in the side table of continuations, which is traced anyway.

**A handle lives only where the tracer can rewrite it in place**: in an
object's words, in a word of a stack frame, in a continuation that one
activation's row of the side tables owns, or in the step state's own roots
— never behind a
shared `Rc`, which the tracer could reach only through a shared reference.
So a closure's handles live in the closure object's own words, and its code,
which holds no handle, is named by a code id into the heap's table of
compiled code, shared with the `MakeClosure` instruction that made it; and
everything else that needs a closure — `FrameKind::Closure` on each of its
activations, `Step::Block` — names the closure object, whose
captured frame and copied values the tracer then rewrites once, in its
words. A continuation's state is boxed in it (`Box<NestedClassCreation>`,
`Box<PendingUnwind>`), owned by exactly one activation.

The inline caches ([dispatch.md](dispatch.md), "3b — The caches") add
nothing to this list, by design, and are not traced: an entry holds a
dispatch id and a target — an `Rc` to a method or a mixin, a field index, a
primitive, a level count, a family's `feedback_base` — and never a handle,
the defining class included, which a hit recomputes from the receiver. The
polymorphic pool and the global cache hold the same entries, and the family
map holds `Rc`s to code, which hold no handle. The table lives in the step
state, per heap, so nothing in it is ever reached from another heap, and a
move leaves every entry valid. The numbers the caches add to traced records
are not handles either, and the tracers say so: a stack frame's feedback
base, a `SmallInteger` word the stack's trace passes over, and a closure's feedback-base field
(`trace_block`), and the dispatch id every object carries in its header
word, which is what a guard reads ([dispatch.md](dispatch.md), "The dispatch
id"). The ids a cache guards on are per heap and never serialized. The
header's id is a number, not a handle: the tracer does not visit the header
word, and a move carries it along with the rest of the object.

**A resolved future is traced as its value.** Before it marks, a collection
(`World::collect`) rewrites every object field that holds a resolved future
of a simultaneous slot clause to hold its value instead
(`Heap::replace_resolved_futures`, with the test
`interpreter::future::resolved_futures` answers): an instance's and an
array's fields -- an indirection vector is an `Array` --, a weak array's
words and an ephemeron's fields, and a closure's
copied values. A class object holds no `Value` a future could stand in, a
string or symbol none at all, and a frame object is exempt as a register
file is, below. The
marker then finds a future that only object fields reached unreachable, and
the compaction reclaims it. A resolved future's value is never itself a
future (its `resolve` forces a future its computation answered before it
records the value, and one whose fields crafted code made so is malformed
and left in place), so one pass leaves none behind.

Registers and host roots are exempt -- a reified frame's, the activation
stack's, and every `Root`. `FutureImplementation`'s own `resolve` and
`doesNotUnderstand:`, whose activations forced it and are still returning
their answers, run with the future as `self` -- a register that must keep
naming an instance of the class whose method runs -- and a host that holds the future holds that
object and may ask for its state; rewriting either would change what they
read. A future held there
survives the collection and is replaced by the first one after it lets go,
so a resolved future lives at most until then. The walk is gated on
`Mutator::futures_made`, so a program that never made a future pays nothing
for it, and the move stress mode (`World::move_everything`) does not
replace: moving is not collecting ([object-model.md](object-model.md),
"Simultaneous slots"). A scavenge does not replace either: the walk reads
every object of both generations, which a scavenge exists not to do, so a
resolved future stays until the next full collection. Only where nothing
else keeps one -- in a weak word or an ephemeron's key -- does a scavenge
judge it by its value and rewrite the word to it, with the same test
([core-library.md](core-library.md), "The heap and the collectors").

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
never pushes an object out of it. A shift changes an object's address
modulo 32, so after copying a region the move walks it and realigns every
`ByteArray`'s data ("Every mover realigns a `ByteArray`'s data"); the
varied shifts put each at all four positions over a run. The remembered set holds raw offsets that
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
epoch: no stack frame moves, and its generation already covers an offset reused
after a pop. V8's `--stress-compaction`, SpiderMonkey's compacting GC zeal mode
and HotSpot's `ScavengeALot` exist for the same reason. Here the stress mode
also comes first: the tracing and forwarding the collector will need are built,
and held to the whole suite, before the collector is.

**The schedule is budgeted, not every safepoint.** A move costs work
proportional to what it traces — the objects, the activation stack's words,
the canonical symbol table and the host roots
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
it, and a boxed continuation such as `Box<NestedClassCreation>` can change
its lifetime in safe Rust only by reallocating: at every push and pop of a
stack frame that carries one, which, when this was decided, was every
iteration of a `whileTrue:` or `do:` over blocks the compiler did not
inline, both then Rust loops. Rejected for that cost, with the user, on
2026-09-13. Nothing is converted at a push, a pop or
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
| a `ByteArray` of n bytes | 2 + 3 + ⌈n/8⌉ | 40 + 8⌈n/8⌉ |

Each adds 8 bytes, its overflow word, once its words after the header pass
254: an `Array` of 254 elements or more, a frame of 244 registers or more,
a `ByteArray` of 2,001 bytes or more. A `ByteArray`'s three reserve words
let its data begin at a 32-byte address wherever the block lies ("Every
mover realigns a `ByteArray`'s data").
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
which runs between two entries. Before it, only the binary's own code
runs: the loader checks every shipped name against the binary's
declarations, refusing an unknown one before it builds anything, builds
the modules and classes the section names that the boot did not, and
resolves each name by table lookup. Nothing allocates between the tenure
and the graph pass, which allocates every placeholder old and never polls
a safepoint, so nothing collects while the placeholders exist and a
rejected image cannot give them back part-way through ("Generations"). It is
not retained past that: `image::load` owns its `Interpreter` in a local, so
every `Err` path drops it and the space with it -- the cost is a transient
peak during loading, not a leak for the process's life.

**The decoder reads the graph section twice**
([image-format.md](image-format.md), "Two passes"). The first pass reads
every record as the second will and allocates nothing in the object space:
beside transient, bounded buffers, it interns the mixin each reified
mixin's record names, and resolving one that names a class side may
synthesize that class side, a Rust value of the heap's mixin table, at
most one per instance-side mixin, which lives until a full collection
finds its module's tree dead or the interpreter the load returns or drops
goes; it sizes each record's object from the record's own bytes,
refuses what the second pass would refuse about counts, ids, a symbol or a
mixin named twice and a `FutureImplementation` record of another field count, and refuses before
anything is allocated when the objects would take more than eight words a
byte of the input left. Then each record's placeholder is allocated at the
size the first pass read, and the second pass fills each in place, every
value stored as it is read and a string packed straight from the buffer, so
no record is ever built beside its placeholder. Since every placeholder is
sized from its own bytes, each byte belongs to exactly one record and one
value, and the ratios below take a maximum rather than a sum. Against the
bytes of the graph section and, for symbols' text, the symbol section, and
for declarations' texts the text table (the derivation is `decode_value`'s
doc comment, `image/decode.rs`, and for the text table `texts::decode`'s,
`image/texts.rs`):

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
| A declaration's text, in an image with source: the 16 bytes of an empty text without holes become a 72-byte `Rc<DeclarationText>`, its 8-byte slot in the decoded table and a 16-byte entry of the check that each text has one owner; the last two are dropped before the graph pass, so 4.5x is held through it; a hole and a text's bytes cost what they take on the wire | ≤ 6x of the text table | ≤ 6x of the text table |

The other record kinds stay below the metaclass record: a class record is at
least 19 bytes for the same 80 (4.2x; 5.5x and 5.9x with the decoder's
tables), a level a `<:` clause added at least 23 for 88, its one field more
the class its clause named (3.8x; 4.9x and 5.2x), a body-less declaration's
class object at least 25 for 104, the level's record and two presence flags
for its declaration's scope (4.2x; 5.1x and 5.4x), a frame record at least 21
for 96 (4.6x; 5.7x and 6.5x), an
instance, array, string or symbol record at least 11 for 16 (1.5x), a
closure record at least 17 for 40 (2.4x), a byte-array record of `n` bytes
`14 + n` for `8 × (5 + ⌈n/8⌉)`, its alignment reserve not on the wire, at
most 40 for 14 (2.9x; 4.0x and 4.6x with the decoder's tables), a reified
mixin's record at least 13 for 24 (1.8x). A
placeholder is filled in place, so no record's fields are laid out beside
it.

**The record figures count a class tag as two bytes**, as the derivation
in `decode_value`'s doc comment does; every class tag the encoder writes
is five, a tag byte and a `u32` (`CLASS_RECORD`, `CLASS_SHIPPED`), and the
byte-array record alone is counted at that. So every other record is at
least three bytes longer per class it names than counted, each record's
ratio above is an upper bound, and so is the 10.7x: a metaclass record is
at least 15 bytes for its 80, 5.3x, and 6.4x and 6.9x with the decoder's
tables, below the 8x of a value's word, which then sets the release
build's figure. The bounds stay safe; tightening them in the doc comment,
here and in `CLAUDE.md` together is open ([OPEN-WORK.md](../OPEN-WORK.md)).

**So the transient peak of a rejected load is about 10.7x of the graph
section's bytes in a release build (a graph of metaclass records) and about
16x in a debug one (a graph that is all root list), plus at most 4x of the
symbol section's bytes and, in an image with source, at most 6x of the
text table's.** The texts are decoded and held to the rows before the
graph pass, and a text belongs to one declaration, so the module texts
assembled from them are at most the sum of their bytes, and are assembled
only once the load has succeeded ([image-format.md](image-format.md),
"Source"). A fraction of two integers beyond 61 bits but
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
many entries, a set of indices and a `Vec` of resolved names of 32 bytes (40
in a debug build, whose handle is two words), about 7.7x of the section's
bytes (8.5x) for the moment it runs and at most about 5 MiB, all of it before
any placeholder exists. The check the loader runs before it builds the
modules and classes a section names (`check_names`) reserves a set of as many
names, as `resolve` does, and one root mixin per shipped module; what the
builds allocate is the binary's own modules and classes, bounded by the
binary rather than by the input, and is tenured before the graph pass. A name
the loading world does not have and a name given twice are refused, so a
section that resolves lists at most as many names as the shipped world has, a
few hundred; what the graph pass holds beside its placeholders -- the names
and their resolution, a second resolution in the graph's own entry with the
set dropped again -- is therefore bounded by the shipped world, a few KiB,
not by the input. The figures above stand: about 10.7x of the graph section's
bytes in a release build and 16x in a debug one, plus at most 4x of the
symbol section's and 6x of the text table's, plus that constant.

**The leftover analysis adds a fixed term, not a factor.** `verify_code_table`
runs it on every decoded body ([image-format.md](image-format.md),
"Verification"), and it is bounded before it allocates: at most 258 bitsets
of at most 8 KiB, 2.02 MiB, in a release build, and 386, 3.02 MiB, in a
debug one, which also keeps each open loop's head state
([compiler.md](compiler.md), "What a callee leaves", "The bound"). It holds
them for one body at a time, freed before the next, and all of it before
the graph pass: `decode_described` verifies the code table before
`decode_graph_phase` reads the graph section (`image/mod.rs`), so no bitset
lives beside a placeholder. So the peak of a rejected load is the figures
above, plus at most 2.02 MiB (3.02 MiB in a debug build) while the code
table is verified.

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
and the growth policy" below); the leftover analysis's list of that body's
loops, one 12-byte `Region` per backward jump, and in a debug build a
16-byte slot per loop for its head's state, of one body at a time and
smaller than its decoded `Vec<Instruction>`; the text table's `Vec`, reserved for the
count the table claims, at most 8 bytes for every 16 that follow the count
and dropped before the graph pass, the refusal coming as soon as the
records run out; and what the first pass keeps
of the reified mixins' records -- the mixin each names, a set of their ids,
both dropped before the placeholders exist, and a class side resolving one
may synthesize, a Rust value, at most one per instance-side mixin,
entered in the heap's mixin table and so living until a full collection
finds its module's tree dead or the interpreter the load returns or drops
goes. Each stays proportional to the input.

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
  O(1) per handle, one cache line a lookup, about 3% of the committed
  space, and a collection writes only the chunks of what is occupied
  ("Compaction and forwarding" below). Nova, the one engine
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
pushes every unmarked object its strong pointer words name, read through
`strong_pointer_ranges` -- `pointer_ranges`, the one enumeration of an
object's pointer words, which forwarding's `trace_block` walks too ("What
the collector traces"), less the weak words, whose objects
`Heap::finish_marking` settles once the work list is empty ("Weak
references"). The bit
is set when an object is visited, not when it is pushed, so an object
reached twice before its visit is popped twice and visited once, which also
ends the traversal on a cycle. The traversal is iterative because the
graph's depth is the program's to choose — the reasoning that already made
`MixinApplication`'s `Drop` iterative. Sharing one edge enumeration between
marking and forwarding is what keeps the two from drifting: a field a later
change adds is named in exactly one place, and the compiler refuses that
place until it is named.

**Marking gathers the ids the objects name.** A class object names its
application and its applied clause's mixin, a closure its body and its
feedback range, a frame its body, each as a `SmallInteger` no pointer range
reaches. `drain_marking` already matches on the visited object's format,
so one arm of that match records those rows in the marker's bit sets
(`heap/reclaim.rs`'s `GatheredIds`), the ephemeron fixpoint's visits
included, and `World::collect` hands them to `Mutator::reclaim_ids` after
the prune, while the mark bits still say what lives
([object-model.md](object-model.md), "An application has two ids"). A
second walk of the heap would cost what the marking already reads.

The roots are the ones a move forwards -- the heap's own, which
`Heap::begin_marking` pushes (the `Activation` class, the activation
stack's words, the host roots), and the step state's,
`StepRoots` (the builtin classes, the `platform` singleton, the VM mirror,
the kernel, the shipped world and the depth limit's reserve) -- with the canonical symbol
table and the table of reified mixins removed, since those are weak. The remembered set is not a root here:
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
(`summarize`). The table is summarized over the chunks of the three
occupied ranges, the old space's first, then the past survivor space's and
eden's, the gaps between them skipped, so a survivor's new position is
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
mark bit and age are cleared as it lands, and a `ByteArray`'s data are
moved to its new block's alignment ("Every mover realigns a `ByteArray`'s
data"). The remembered bits are cleared
first, at the offsets the remembered set holds, and the set emptied: once
everything is old nothing needs remembering. The new top is `NURSERY_END`
plus the live words, the nursery is empty, and a pending scavenge request
is served by the collection with it. The third walk is the existing
`Forwarder` over everything that holds a handle -- the roots and every
object by its format (`forward_everything`) -- each offset looked up in the
table. The table takes two `u64`s per 64 words of the space below the old
space's top, about 3% (decided with the user on 2026-10-03; "Decisions"
below). Why the full collection compacts both generations into the old
space, and what was rejected, is decision 2 under "Generations".

The table covers what is occupied, not what is reserved. It is indexed
from word 0, so a lookup is one index and needs no range check per
forwarded handle; but its span includes the whole nursery's reservation,
10 MiB, about 20,000 chunks, while what the nursery holds at a collection
is only the past survivor space's and eden's filled prefixes. A table
allocated, zeroed and summarized over its whole span would make every
collection cost the reservation however little lives: under the collect
stress mode, which collects at every safepoint of a program with a hundred
live objects, three quarters of the time would go to it. So a collection
writes and reads only the chunks that cover the occupied ranges: `set_live`
marks inside them, and `summarize` fills in their prefixes, the old space's
first, then the past survivor space's and eden's, and leaves the gaps
between them untouched -- a chunk in a gap holds no live word, adds
nothing to a prefix and is never asked for a destination. The buffer
outlives the collection: `World::collect` hands the `Forwarder` back
(`Heap::recycle_forwarding`) once it has forwarded the interpreter's
roots, and the table clears the chunks it covered as it goes
(`IdleTable`). Every chunk outside the occupied ranges is therefore clear
whenever a collection begins -- including a chunk a longer eden covered
last time, whose live bits would otherwise count as live words and leave
holes in the old space, and would make a word in a gap read as live to the
debug build's check that every forwarded handle names a survivor. A table
not handed back takes its buffer with it, and the next collection starts
from an empty, and so clear, one. The buffer is kept to the chunks of the
committed memory, so its footprint follows the space a collection gives
back ("Giving committed memory back") rather than the highest top the
space ever had. When the old space's top has grown past it, it grows to
the chunks of the committed memory, which grows by doubling, so an old
top that rises at every collection copies the buffer only as often as the
space commits more.

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
every allocation that can reach a trigger enters the space -- the fast
bump stops 8 bytes short of eden's threshold and never allocates old
("Inline allocation") -- and which holds the region's new top already.
The fast bump itself tests nothing but its limit. A young allocation compares
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

**Every side table has a growth limit** (`heap/limits.rs`'s
`GrowthLimit`), of the collection limit's shape: after every full
collection a table's limit becomes `max(floor, 2 × size)`, `size` the
table's size after the collection reclaimed what it could
(`World::collect` resets them after `reclaim_ids`, beside
`reset_collection_limits`). The space's own limit asks for a collection
only when objects grow, so a program that edits methods or runs fresh
families without promoting much would otherwise grow these tables
between collections without bound. The floors lie above what a small
program mints, so it never collects for a table: `run_goldens.rs` and
`run_benchmarks.rs` hold every golden and benchmark program to no table
collection, through the count that `ns-next run --collector-statistics`
and `bench --collector-statistics` print, except a golden whose `.toml`
says which rows it keeps live past a floor (`reaches_a_table_limit`) --
`a_chain_deeper_than_the_depth_limit_overflows.ns`, whose 102,001
classes are as many live applications. Such a program collects for the
table a logarithmic number of times as it grows, as one whose live
objects grow collects for the space:

| Table | Size measured | Floor |
|---|---|---|
| code ids | `Tables.codes` less `free_codes` | `CODE_ID_FLOOR = 16_384` |
| mixin rows | `Tables.mixins` less `free_mixins` | `MIXIN_ROW_FLOOR = 4_096` |
| application rows | `Tables.applications` less `free_applications` | `APPLICATION_ROW_FLOOR = 4_096` |
| sites | `InlineCaches.sites` less the free ranges' sites | `SITE_FLOOR = 262_144`, 8 MiB of cells |
| program-made symbols | `ProgramSymbols`' live entries | `PROGRAM_SYMBOL_FLOOR = 65_536` |

A table's size is its rows in use -- its length less its free rows, the
sites less those the free ranges hold -- never its length alone: under a
churn whose live row is the table's last, no truncation shortens it, so
a limit on the length would let the table double at every cycle, its
length growing with what was minted rather than what is live. A freed
row is minted again before the table grows, so its length stays within
the high mark of its rows in use. Every mint, a freed row's included,
compares the rows in use with the table's limit: a code id in
`Heap::register_code_uncached`, behind `Heap::code_id`'s cache, a mixin
row in `push_mixin_row`, an application row in `register_application`,
a program-made symbol in `Heap::record_program_symbol`, and a new range
in `InlineCaches::fresh_range`. A reached limit calls
`Heap::request_table_collection`, which, like `request_collection`, only
sets flags, so a mint inside a leaf or while an object is half-wired may
call it. The heap's limits are its `TableLimits`; the sites' lies with
the sites, since `InlineCaches` has no heap: `fresh_range` records the
reached limit, `InlineCaches::wants_collection` answers it, and the
mutator that asked for the range requests the collection --
`Mutator::feedback_base_for`, through which every caller of `base_for`
goes, machine code's leaves included, and `fresh_feedback_range` for a
closure an image decoded. A family that already has its range pays
nothing for it: `feedback_base_for` mints, and asks, only on its cold
path. The collection is served at the next safepoint, as the heap's own
request is. `CollectorStatistics.table_collections` counts the
collections a table requested and the space's limit did not;
`Interpreter::table_sizes` reads every table's length and what the
reservations beside the space have committed, and
`Interpreter::set_table_floors` lowers the floors for a test
(`tests/collector.rs`'s churns of method edits). HotSpot requests a
collection when class metadata reach a high-water mark, and resizes the
mark after the collection from what is still in use
(`prior-art/jdk/src/hotspot/share/memory/metaspace.cpp:272-300`,
`MetaspaceGC::compute_new_size`, `:424-490`); V8 starts marking when the
memory its objects hold outside the heap passes a limit, and raises the
limit to what is held then (`Heap::UpdateExternalMemory`,
`prior-art/v8/src/heap/heap.cc:6915-6968`).

**No time-based watchdog.** No collection is started by the clock: every
one follows from what the program allocated or minted, so a run is
deterministic, and the growth limits bound every table without one.
V8's memory reducer, which starts marking from a timer when the mutator
seems idle and again after a watchdog delay
(`prior-art/v8/src/heap/memory-reducer.cc:41-42`, `:56-90`), is
rejected for that reason.

#### Giving committed memory back

The reservations beside the space give back what a peak committed.
Decided with the user on 2026-10-30, one of the six decisions that make
everything that grows reclaimable (plan
`docs/superpowers/plans/2026-10-30-0103-jit-closures.md`, "Part 4d").

**Committed memory is given back by a band and a damping**
(`heap::limits::Shrink { threshold, headroom }`, `ShrinkDamping`). At the
end of every full collection (`World::shrink_committed_memory`), and after
every compaction of the code zone, each reservation beside the space is
asked whether it lies in its band, and nothing decommits anywhere else, so
a high mark that rises and falls within one collection's cycle never does.

**The band** (`Shrink::band`): more than the threshold committed above the
high mark, and more than twice the end the reservation would keep, the
high mark plus its headroom rounded up to a page. Its first condition is
Spur's `attemptToShrink` after a full collection, which shrinks only when
the free old space exceeds `shrinkThreshold` and keeps `growHeadroom`
(`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:50168`,
`:50244-50249`; its defaults of 32 MiB and 16 MiB, `:51088-51094`). Spur
grows additively, by the larger of what it needs and its headroom
(`cointerp.c:26019`, `:50328-50329`), and its pair is stable because its
threshold is at least the headroom plus one growth step. The stack, the
cells and the two id tables grow by doubling what they have committed, so
their step from a kept end is that end, and one regrowth from it would
cross a threshold of twice the headroom whenever the high mark is above 0:
a sawtooth back to the same peak would shrink and grow at every
collection. The second condition -- a shrink keeps less than half of what
is committed -- restates Spur's invariant for a doubling growth, the band
HotSpot keeps between `MinHeapFreeRatio` and `MaxHeapFreeRatio`
(`prior-art/jdk/src/hotspot/share/gc/serial/tenuredGeneration.cpp:153-224`)
in bytes rather than ratios. The code zone commits only what an install
needs, so the second condition only makes its shrink rarer.

**The damping** (`ShrinkDamping`, one per reservation, the cells' two
regions sharing theirs): a collection in the band gives back a factor's
percent of what lies above the band's end, rounded down to a page, the
factor 0 the first time, then 10, 40 and 100, and a collection out of the
band sets it back to 0. It is HotSpot's `ShrinkHeapInSteps`, its
`_shrink_factor` read and reset before each resizing and advanced only by
a shrink (`tenuredGeneration.cpp:145-151, 218-231`;
`prior-art/jdk/src/hotspot/share/memory/metaspace.cpp:425-427, 503-522`),
whose comment gives the reason: a program that collects between phases
would otherwise give back at once what its next phase grows again. It
counts collections, never time; the share is multiplied before it is
divided, where HotSpot divides first, so that 100% reaches the band's end
exactly. The code zone's damping counts its shrinks instead, since the zone is
asked after each of its compactions as well as at the end of a full
collection: a safepoint that serves both a compaction and a collection
advances it twice, and the second ask finds the zone's growth flag
already taken by the first (`JitState::zone_damping`).

**Growth is demand** (decided with the user on 2026-10-31). A reservation
that grew since the previous collection is out of the band at this one:
it does not shrink, and its damping starts again at 0. HotSpot resets its
factor wherever the generation does not want to shrink -- it reads and
zeroes it before each resizing, and its expanding branch returns with it
0 (`tenuredGeneration.cpp:146-151, 181-193`) -- and a generation sees its
demand in what is used after a collection. A collection here runs at a
low point -- the stack's top between two recursions, the tables just
truncated -- and does not see the peak between two, so growth since the
last one stands for it: a flag each reservation sets in its cold growth
path and its shrink takes. Growth past what is committed alone does not
show a peak that fits what the damping left committed, and a peak
returning every cycle would thrash still, at every partial step that
cut into it (16 decommits in 30 cycles of a 6 MiB recursion, measured).
So the stack, whose push already compares with an ordinary end in the
cold path, lowers that end to the band's end at every collection in its
band, whatever the damping gives back (`FrameStack::shrink`), and a push
past it is a growth too, which raises the end again without a commit
(`FrameStack::grow_to`) -- machine code's push past it calls the growth
leaf, which does the same and lets the push go on
(`jit::leaves::grow_stack`). The tables grow only by minting rows
between two collections, so their high mark at a collection is their
peak.

The guarantee: of the collections in a row that a reservation's band says
shrink at and that follow no growth, the first gives back nothing, the
second 10% of what lies above the band's end, the third 40% of what is
left above it, and the fourth and every later one all of it; one
collection out of the band, or after a growth, in between starts the
count again. So a program whose peak returns between every two
collections keeps what it committed, whatever the peak's depth, and one
that stops peaking gives it back over the ramp. After a shrink kept `end`
for a high mark `h`, a cycle whose peak lies within one doubling of `end`
never puts the reservation back in the band at a collection whose high
mark is `h` or above. Pinned by `tests/collector.rs`:
`a_sawtooth_of_recursions_does_not_thrash` (collections at a top above 0,
the stack decommitted once in a hundred cycles; it fails without the
band's second condition), `a_recurring_peak_beyond_a_doubling_does_not_thrash`
(a 6 MiB recursion between collections at a top near 0, at most two
decommits in thirty cycles, then given back once it stops; 29 without
the growth, 16 with growth past the committed end alone),
`a_shrink_is_damped_over_successive_collections` and
`a_returning_peak_resets_the_damping`.

The pairs are constants named per reservation in `heap/limits.rs`, and a
test sets them all at once, the damping with them
(`Interpreter::set_shrink_pairs`, `ShrinkPairs::damped`). Under the
collect and the zone stress modes every pair is one page of threshold and
one of headroom, undamped -- HotSpot without `ShrinkHeapInSteps`, its
factor 100 throughout (`tenuredGeneration.cpp:294-295`) --
(`Interpreter::shrink_pairs_for_stress`), so that every collection and
compaction in the band gives back nearly everything above its high mark
and the stress runs check that each bound is lowered before its pages go:

| Reservation | Threshold / headroom | High mark | What the decommit lowers first |
|---|---|---|---|
| the cells (1 GiB reserved) | 32 MiB / 16 MiB, Spur's | the end of `sites` after the sites' slide ([dispatch.md](dispatch.md), "Code families"), and the polymorphic pool's length; the global rows never shrink | each region's committed end, which only Rust reads: machine code reads a site's cell only for a live range and a block's only for one a site names |
| the activation stack | 4 MiB / 2 MiB | the stack's top; a 100,000-deep recursion commits about 16 MiB | the committed end and the ordinary end, with its machine word `OrdinaryStackEnd` (`FrameStack::shrink`) |
| the machine entry table | 1 MiB / 512 KiB | the code table's length after its truncation | the committed rows and their machine word `MachineEntryRows`, which the closure stubs compare an id with |
| the optimization budget rows | the machine entry table's pair | the code table's length after its truncation | nothing: every budget word was pointed at a row of an id the table holds before ([execution.md](execution.md), "The optimizing tier's foundation", "The budget") |
| the dispatching rows (48 MiB reserved) | the machine entry table's pair (`OPTIMIZED_COPIES_SHRINK`) | the blocks of the code table's ids after its truncation | nothing: every body's dispatching word was pointed at a block of an id the table holds before, which a debug build checks ([execution.md](execution.md), "Speculation", "The dispatching entry") |
| the application id table | 1 MiB / 512 KiB | the rows recorded after the application rows' truncation | nothing: machine code's bound, `ApplicationIdRows`, is the rows recorded, which the truncation lowered |
| the code zone (16 MiB by default) | 2 MiB / 1 MiB | the zone's next offset, after its compaction slid the survivors down ([execution.md](execution.md), "The code zone") | the writer's committed end (`CodeWriter::decommit_above`) |

Every bound machine code compares against is lowered before a page goes,
so machine code reads only committed memory: an id past the lowered
machine entry rows reads as no machine entry, and the interpreter calls
the block. The code table's length counts every body with machine code,
not only the bodies an activation runs: a full collection frees the id
of a body nothing runs, and the budget words' pointing that follows
mints one again for each body whose code lives
(`jit::point_budget_words_after_collection`), so that a compiled body
keeps a code id, and its rows in the three tables, for as long as its
code lives. The stack shrinks between entries, when no frame machine
code pushed lies above its top. A page of a table given back reads zeros
when it is committed again, and each table writes what it reads there
first, as it does for a page committed fresh. On Windows the code zone
is made of chunks of 1 MiB, each a pagefile-backed section of its own
mapped into both views in place of a placeholder, and it commits and
gives back whole chunks: a chunk given back has both its views unmapped
back to placeholders and its section closed, which returns its pages and
their commit charge, as ZGC maps and unmaps its heap
(`prior-art/jdk/src/hotspot/os/windows/gc/z/zMapper_windows.cpp:62-75`,
`:258-312`; HotSpot's placeholders,
`prior-art/jdk/src/hotspot/os/windows/os_windows.cpp:3573-3618`); the
zone needs Windows 10 version 1803 for it, and refuses to be made
without it (`CodeZone::reserve`, `MemoryError::Unsupported`). Linux
punches a hole into the zone's memory object, Apple AArch64 frees its
`MAP_JIT` pages with `MADV_FREE`, and a POSIX shared memory object
elsewhere keeps its pages (`platform::decommit_code`). A zone page given
back reads zeros on Linux and on Windows, where a chunk mapped again is a
fresh section, and stops or zeros on Apple AArch64, where its contents
are undefined until written and the compaction wrote stops over it
first; either way no code runs from it before an install writes it
again. `CollectorStatistics.decommits` and `decommitted_bytes` count only
what the system took back: a POSIX shared memory object's pages and a
hole a file system refuses count nothing. `TableSizes.zone_committed_bytes`
is what the zone's writer counts committed, the pages it commits again
before it writes past them, which for a POSIX shared memory object may be
less than the system still holds.

### Weak references

In the space, four things are weak: the canonical symbol table, the table
of reified mixins, a `WeakArray`'s slots, and an `Ephemeron`'s fields while
its key is reached by nothing but ephemerons. Beside it, the per-heap tables
of mixins, applications and compiled code, and the caches keyed by them,
hold an entry only while something live names its id.

**A `WeakArray` and an `Ephemeron` have formats of their own**, codes 10
and 11 (`heap/header.rs`): `WeakPointers`, whose fields are a `WeakArray`'s
slots, and `Ephemeron`, whose first field is the key and whose others, the
value and the finalizer, are reached only through a live key
([core-library.md](core-library.md), "Weak references", has the classes,
their protocol and the image's records). Their words are stored words
like any pointer object's: `pointer_ranges` names every one of them, so
forwarding, the moves and the remembered set's verification read them all,
and a weak word follows its object while the object lives. Marking and the
scavenger trace `strong_pointer_ranges` instead, the class word alone of a
`WeakPointers` object and of an `Ephemeron` whose key is not yet known to
survive. Each collector lists those objects as it meets them and, once
its traversal has run dry, settles them in Primordial Soup's order
(`vm/heap.cc:627-682`): it traces each listed ephemeron whose key has
since been reached, runs the traversal again, and checks again, with the
ephemerons it listed anew, until the traversal has nothing left to visit.
The first two checks scan every unresolved ephemeron; a fixpoint still
running after them indexes those left unresolved by key
(`heap/pending.rs`), and from then on looks each object it reaches up in
the index once and traces the ephemerons keyed by it, recording what it
reaches only while the index holds an unresolved ephemeron -- so a weak
map's fixpoint, which ends within the two scans, indexes nothing, and a
chain of ephemerons costs its length. It sets every field of an
ephemeron left unresolved to `nil` (it is mourned: its key was reached
by nothing but
ephemerons); and sets to `nil` every word of a listed weak object whose
object was not reached. A full collection does so in
`Heap::finish_marking`, between `drain_marking` and `prune_canonical_symbols`, before compaction, whose
forwarding table answers only marked objects; a scavenge in
`Scavenger::finish`, before the nursery is vacated, a young key surviving
when it was copied, forwarding each surviving young referent's word to
its copy, and remembering an old weak object or ephemeron that still names
a young object afterwards, once. Writing `nil` stores no young object, so
neither pass needs the write barrier; a program's store into a weak slot
takes it like every store (`Heap::weak_array_set_element`). The finalizer
is held and never run until the actors' event loop gives it a turn to run
in ([OPEN-WORK.md](../OPEN-WORK.md)).

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
`Mutator::new_symbol` and `Mutator::program_symbol` are the only ways to
make one. If a symbol object is collected then nothing held it — anything that did would have been a root and
kept it live — so the next `new_symbol` mints a fresh one whose identity is
again unique, and no live comparison can tell the difference. An identity hash
belongs to the object rather than to the symbol; a program that keeps the
number `identityHash` answered instead of the object has no guarantee today
either, and Squeak's weak symbol table behaves the same way.

**A program-made symbol takes no number and dies with its object.** A
symbol a program makes from a string -- `asSymbol` and its kin -- does not
intern its text, so a run that makes a million distinct symbols and keeps
none leaves the interner as it was
(`tests/builtins_text.rs`'s
`a_million_program_made_symbols_leave_the_interner_as_it_was`). The
interner (`symbol.rs`) is append-only: `Symbol::intern` leaks one text per
new spelling, per thread, for good, so a weak table of numbered symbols
would let the object die but never its number or its text. Newspeak's own
tests require a program-made symbol to be collectable
(`testSymbolTableWeakness1` and `2`,
`prior-art/newspeak/KernelWeakTests.ns:228-243`;
`goldens/runtime/symbols/program_symbols_are_weak.ns`, where the second
answers with the Ephemeron's refusal of a value as its key).

- **Why no number.** Recycling the number and the text cannot be built
  without `unsafe`: `Symbol::as_str` answers a `&'static str` that
  `intern` leaked, and freeing it would take `Box::from_raw`, which
  `ns-runtime` forbids, or a non-`'static` text API at every caller of
  `as_str`; the interner is one per thread, shared by every interpreter on
  it, so one heap's collection cannot know whether another heap still
  holds a number; and a `Symbol` is a `Copy` `u32` held by compiled code,
  cache keys, return sites, frame headers and every Rust table, each of
  which a recycled number would need scrubbed. A `Symbol` object already
  holds its own text (`Heap::allocate_symbol` is `allocate_string` with
  the `Symbol` class), so a program-made symbol whose text nothing has
  interned needs no number at all: it is just that object, and no number
  exists to recycle. Sends, caches and machine code see only permanent
  numbers, where reference-counted numbers would cost on every cache
  fill, install and text read; the user chose this mechanism as the
  faster of the two.
- **Permanent and program-made.** A symbol is permanent when its text
  was interned -- by the compiler, the resolver, the image decoder or
  the runtime's own constants -- which is every `Symbol` value Rust
  holds; the interner stays append-only, bounded by the source compiled
  and the images loaded, as HotSpot keeps the symbols it never frees at
  `PERM_REFCOUNT` (`prior-art/jdk/src/hotspot/share/oops/symbol.hpp:99-102`,
  `:167-169`). A program-made symbol is a `Symbol` object whose text
  nothing has interned; compiled code, a cache key, a return site, a
  frame header and an image record can never hold one. A run never
  interns a text a program chose, or a host that keeps one interpreter
  would grow the interner with every run: the error a run ends with,
  whose `HostError` names its selector as a `Symbol`, reports a
  program-made selector as `doesNotUnderstand:`'s, whether the kernel's
  `defaultAction` ends the run (`Activation`'s termination) or the depth
  limit's report does, its text in the error's message
  (`tests/exceptions.rs`'s
  `an_unhandled_unnamed_selector_is_reported_as_does_not_understand_on_both_paths`).
- **The weak table** (`heap::symbols::ProgramSymbols`), per heap, beside
  `canonical_symbols`: the program-made symbols keyed by a hash of their
  text and compared by text, so it keeps no copy of the text, with an
  inverse from an object's offset to its hash, as `symbol_of` is the
  canonical table's. The hash is SipHash under a key drawn per heap
  (`std::hash::RandomState`), since a program chooses the texts and could
  otherwise collide them on purpose into one bucket every `asSymbol`
  scans; no output depends on the draw, since a lookup compares texts and
  no answer depends on the order of a bucket. It is weak
  as the canonical table is: never a root, pruned with
  `prune_canonical_symbols`, forwarded by a scavenge's weak pass
  (`Heap::forward_weak_program_symbols`) beside `forward_weak_symbols`,
  forwarded by every compaction and move, and counted by its live entries
  against its growth limit ("The trigger and the growth policy") at each
  mint (`Heap::record_program_symbol`).
- **Making one** (`Mutator::program_symbol`): a text the interner already
  holds (`Symbol::lookup`, a probe that inserts nothing) answers the
  canonical object, `new_symbol` of its number; any other answers the
  weak table's object for the text, or a new one recorded there.
- **One text, one object.** When a text a program-made symbol spells is
  interned afterwards -- a method of that name compiled, an image
  loaded -- `new_symbol` adopts the existing object as the number's
  canonical object instead of allocating another
  (`Heap::adopt_program_symbol`), and the prune adopts every surviving
  program-made symbol whose text has since been interned, so its
  `symbol_value` is one probe again. The image decoder refuses a record
  whose text a living program-made symbol spells, as it refuses a second
  record of one text (`Heap::has_symbol_object`): the record is an object
  of its own the decoder already built, and `load` decodes into an
  interpreter that has run nothing of a program's, so no such symbol
  lives there. So §5.1.6 holds across both tables: equal symbols are one
  object while either table holds it
  (`goldens/runtime/symbols/program_symbol_identity.ns`;
  `tests/mixin.rs`'s
  `a_program_made_symbol_finds_a_method_installed_after_it`).
- **Reading one.** `Heap::symbol_value` answers a program-made symbol's
  number only once its text is interned, `None` before. A caller that
  only asks whether an object is a Symbol asks `is_symbol`; one that
  looks a name up treats `None` as "nothing has this name", since every
  selector, slot name and class name is interned: `Message>>sendTo:`
  reaches `doesNotUnderstand:` with the symbol object in the `Message`
  (`Mutator::decide_unnamed_send`), and a mirror's lookup by name
  declines to its not-found result. A reader of the text reads the
  object's own (`Heap::symbol_text`). A symbol's `hash` is its text's,
  told apart from a String's of the same text, so it does not change when
  the canonical table adopts the object: the interner stores it with each
  text it interns (`Symbol::text_hash`), so an interned symbol's `hash` is
  one probe and one read, and only a program-made symbol whose text
  nothing has interned computes it from its text. No leaf of machine code reads a
  symbol object's number: the mirror leaves and the stubs read the
  selector of the send, always a permanent number, and `sendTo:` is no
  leaf.
- **The image format is unchanged.** A symbol's record carries its text,
  never a number, so no tag is added, and the decoder interns every text
  of an image's symbol table: a program-made symbol an image carries
  becomes permanent at load, bounded by the images loaded
  ([OPEN-WORK.md](../OPEN-WORK.md); `tests/image.rs`'s
  `an_image_keeps_a_program_made_symbol`).

**The table of reified mixins is weak** (`mixin_objects`,
`Heap::prune_mixin_objects`), as the symbol table is, for the same
reason: as a root it would keep every mixin a mirror ever reached, and with
it the mixin's id. It is not a root for marking; after marking and before
compaction, beside `prune_canonical_symbols`, an entry whose object is
unmarked becomes `None`. While anything reaches the object it is marked
and stays the one object of its mixin, so `==` stays the mixin's identity;
once nothing does, nothing can compare it, and the next reflection makes a
fresh one. A marked entry keeps its mixin's id alive. A scavenge traces the
table strongly, since a scavenge reclaims no id
(`goldens/library/mirror/a_reachable_reified_mixin_keeps_its_identity.ns`).

**The application cache and the id tables are weak.** The cache's key is a
pair of the heap's ids -- the mixin's `MixinId` and the superclass
application's `ApplicationIndex` (`heap/tables.rs`) -- not two addresses,
so a freed `Rc<Mixin>` handing its address to a newly allocated one cannot
make the new mixin hit the old entry and be given a **different, perfectly
valid** application, the failure class the handle invariants exist to
prevent. A full collection reclaims every application, every module's tree
of mixins, every compiled body and every cache range nothing live names
any more (`Mutator::reclaim_ids`), and drops every application-cache entry
whose application or superclass key it freed, so the cache's size follows
the live applications, edits or not ([object-model.md](object-model.md),
"An application has two ids", which has the reuse rule of each id). The
heap's tables still map a mixin's and a body's address to its id
(`Rc::as_ptr`), which is sound because each table holds the strong `Rc`
that keeps the address valid and unique for as long as the entry lives,
and a version or a body leaves its map when the table lets go of it. The
same per-mixin id serves dispatch's open option of guarding a private
lexical site on N's mixin ([dispatch.md](dispatch.md), "Decisions").

The cache holds no handle at all — a pair of numbers for a key, an `Rc` for a
value — so it is neither a root nor a forwarding target.

An inline-cache entry holds no handle by construction, so
it survives a collection untouched and simply misses when its guard no longer
matches. Compiled code holds none either: `Literal::Symbol` is an interner
index, not an object.

### Frames, the activation stack and the marriage

The activation stack is a root, and nothing there is reclaimed: a frame is
popped when its activation returns, so the stack is bounded by the deepest the
program has been. Marking traces every word of every live frame, frame by
frame as `Heap::trace_stack` walks the stack -- the header's receiver, `current_class`, operand and `reified` words among
them, every other header word a `SmallInteger` or nil, then the registers --
and every row of the side table of continuations, which a frame whose
continuation is not `Deliver` names by row.

A frame object is collected exactly as every other object is, with the same
mark bit and the same forwarding table: there is no separate store for
frames.

**A married frame cannot be collected, and that needs no rule of its own.**
The stack's trace reads its frames' `reified` words, so a frame whose activation is still
running is reachable from the stack and is marked. Leaving that to the root set
rather than to a special case is what keeps a later change from silently
freeing a running activation's frame.

The marriage survives compaction in both directions: the frame object's
married field holds a *stack* offset as a `SmallInteger`, and no stack frame
moves, and the stack frame's `reified` word is forwarded like any other
word. While it is married the frame's own registers are nil and skipped by
the trace, since the live ones are the stack's. The divorce on pop writes the program
counter and the uncontinuable flag, clears the marriage, and copies the
registers into the frame object only when the activation retains them.

**A frame can be tenured while it is married.** A scavenge copies a young
married frame like any other object -- its registers skipped, since they
are the stack's -- and the stack frame's `reified` word is rewritten to the
copy, in the survivor space or the old space; the married field still holds
the stack offset. While it is married an old frame needs no remembering: its
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

**A suspended frame is a frame in a third state**, its married word a
negative `SmallInteger` naming its row in the heap's side table
([execution.md](execution.md), "Suspending and resuming"). The side table is
a root and each row holds its frame, so a suspended frame is marked,
forwarded and scavenged whatever else reaches it, and its registers, the
only copy of its values, are traced. The suspension is a store into an
existing object and takes the barrier's rule as the divorce does: it copies
every register and the sender into the frame and remembers an old frame
that now names a young object (`Heap::suspend_activations`); the resume
copies the registers back onto the stack and nils the frame's own, so a
married frame's registers are skipped again. A `frame_set_local` into a
suspended frame, a handler leaf's write, goes through the barrier like any
other. Suspended activations still count against the depth limit, in the
heap's one running count, which suspending and resuming leave alone.
Pinned by `heap.rs`'s
`a_suspended_frame_survives_a_move_a_scavenge_and_a_collection_with_its_registers`,
`a_young_value_written_into_a_suspended_old_frame_is_remembered` and
`the_reclaim_keeps_the_entered_level_of_a_suspended_activation`.

The stack is frames of tagged words in one reservation, every word of
`[0, top)` of which the collector traces, with no stack map, walked
frame by frame (`Heap::trace_stack`; [execution.md](execution.md),
"Representation"), pinned by `heap.rs`'s
`every_word_of_the_stack_is_a_tagged_value` and
`a_collection_after_a_deep_pop_reads_nothing_above_the_top`.

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

There are thirteen stress modes, each a gate of its own at the end of a batch,
each set by an environment variable to anything but empty or `0`, all of
them debug builds only: `NS_NEXT_STRESS_MOVES` ("The move stress mode"),
`NS_NEXT_STRESS_CACHES` ([dispatch.md](dispatch.md), "Invalidation"),
`NS_NEXT_STRESS_COLLECT`, `NS_NEXT_STRESS_SCAVENGE` ("Finding a missed
barrier: the scavenge stress mode"), `NS_NEXT_STRESS_SUSPEND`,
`NS_NEXT_STRESS_ZONE`, which compacts the code zone, freeing every body
no activation runs, every other compaction keeping each body that has
optimized code ([execution.md](execution.md), "The code zone"),
`NS_NEXT_STRESS_PAUSE`, which stops the run as a debugger's pause does,
reads its whole stack and every value one level deep and continues
([debugger.md](debugger.md), "The pause stress mode"),
each of these seven keeping the safepoint flag set while it is on,
`NS_NEXT_STRESS_WIDE`, which emits every instruction in a wide form
([execution.md](execution.md), "The compact instruction encoding"),
`NS_NEXT_STRESS_JIT`, which compiles every body at its first entry and
enters its machine code at every push, even when that exits at once,
`NS_NEXT_STRESS_EXIT`, which makes every poll in machine code -- every
backward jump it takes -- take its exit, as if a safepoint were
requested, `NS_NEXT_STRESS_OPTIMIZE`, which arms every body's
optimization budget at 0, so that the next safepoint compiles its
optimized code once its baseline code first spends (under it the
back-off after an abandonment or a trap is void, the budget times 4 to
the version still 0),
`NS_NEXT_STRESS_DEOPT`, which implies it and makes optimized code leave
at every n-th interrupt point it passes, n drawn afresh at each safepoint
([execution.md](execution.md), "The optimizing tier's foundation",
"Verification"), and `NS_NEXT_STRESS_INVALIDATE`, which implies the
optimize stress mode too and makes optimized code leave for a
safepoint at every tenth interrupt point it passes, which frees every
body's copies -- a body freed often sitting out ever more rounds --
before it installs what was requested since
([execution.md](execution.md), "Speculation", "Verification").

`NS_NEXT_STRESS_COLLECT=1` runs a full collection at every safepoint rather
than at a limit, paced by heap size like `STRESS_MOVE_BUDGET`
(`StressSchedule`, `interpreter/safepoint.rs`), since collecting at
literally every safepoint is quadratic in a deep recursion. Under it every
golden and every corpus module runs at the highest collection frequency there
is, so a missed root kills its object at the earliest possible moment and the
recorded outputs catch the difference. `NS_NEXT_STRESS_SCAVENGE=1` does the
same with a scavenge, after a check of the remembered set, and is what finds
a store that missed the write barrier.

`NS_NEXT_STRESS_SUSPEND=1` suspends every activation of the run into its
frame at the safepoints its own schedule picks, paced the same way, and
resumes the top one, so every program runs on from suspended frames, the
path a deoptimized frame will take ([execution.md](execution.md),
"Suspending and resuming"). For the collector it moves the run's values
off the stack into frame objects, where every scavenge and collection the
run makes finds them only because the side table is a root and a
suspended frame's registers are traced; beyond the collector, it checks
everything that walks from an activation to its sender.

False retention is caught by counting rather than by observing: a
`debug_assertions`-only `collect_now`, beside `move_now`, and a
`heap_statistics` answering the objects lying in the space and how many of
them are young, the bytes they occupy and the bytes committed for it. An
object in the space is counted until a collection reclaims it, so between
two scavenges the count includes eden's dead objects; a test that counts
collects first. A program that discards n closures is back at its starting
size after a collection; one that keeps n closures keeps n frames.
`ns-next run` and `bench` take `--collector-statistics`, which prints the
scavenges and their total and longest pause, the full collections and
theirs, the objects lying in the space, and the collections a table's
growth limit requested (`CollectorStatistics.table_collections`, "The
trigger and the growth policy"), on one line to stderr after the run
(`crates/ns-cli/src/entry.rs`).

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
against the Windows heap:

| Source | mimalloc | A bump allocator that never frees |
|---|---|---|
| `bench_loop.ns` | level | +6.8% |
| `bench_send.ns` | level | +2.6% |
| `bench_block.ns` | **−15.6%** | −10.3% |
| `bench_megamorphic.ns` | level | +6.6% |
| A scratch source churning short-lived lists, instances and strings | **−8.9%** | −5.6% |

An xperf profile of the Windows heap put 16.9% of `bench_block.ns`'s
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
  road step 6 does in the interpreter ("Inline allocation").
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
place** -- a frame object is allocated with its code's full register file
but the header's numbers,
and an image's frame record carries exactly that many registers
([image-format.md](image-format.md), "Verification") -- so the object
space needs no growth-in-place case. What changes an
object's size is reflective and rare -- edit-and-continue adding locals to a
live activation, a MirrorBuilder adding or removing slots of live instances
(requirements 2 and 3 of
[liveness-and-reflection.md](liveness-and-reflection.md)) -- and takes a
new object: for instances, road step 5b2a's atomic install forwards the old
ones to their replacements in one bulk forward
([liveness-and-reflection.md](liveness-and-reflection.md), "A
MirrorBuilder and an atomic install"): a walk of every
root the collector traces (`trace.rs`) and every heap object that rewrites
every reference ([object-model.md](object-model.md), "`become:` is an eager
bulk forward"); for a frame, the activation will be divorced into the
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
object space" below), eden's top is that reservation's bump cursor
("Inline allocation"), and the heap's activation stack is a second
`Reservation`, the frames of the running activations
([execution.md](execution.md), "Representation"); the heap's application
id table is a third and the inline caches' cells a fourth, both read by
machine code ("The cells' reservation", "The application id table"
below). `Reservation` is not
`Send` or `Sync`, since it holds a `NonNull`, and nothing needs it to be:
both belong to one `Heap`,
and an interpreter, which holds `Rc`s throughout, is not `Send` either.

- **One contiguous reservation per space**: `Reservation::reserve` rounds the
  size up to the platform's reservation granularity and commits nothing,
  and refuses a size past `isize::MAX` bytes as `InvalidRange`, the most a
  slice may span, so `prefix` and `accessible` below need no comparison
  against it. The heap holds its object space in one, reserved once at a
  generous size --
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
  The heap's activation stack is the second: 16 GiB on a 64-bit target
  and 256 MiB otherwise (`STACK_RESERVATION`, `heap/frames.rs`),
  committed from its base as the stack deepens and given back above its
  top and a headroom after a full collection ("Giving committed memory
  back"). Where the operating system limits address
  space, either reservation falls back, halving the request down to no
  less than 1 GiB (`reserve_falling_back`, `heap/space.rs`; a request
  already below that is tried alone).
- **Operations**: `reserve`, `commit` a range, `decommit` a range when a space
  shrinks, `protect` a range as no-access or read-write, and lend each
  accessible, committed range as its own `&[u64]` (`words`) and `&mut [u64]`
  (`words_mut`, and `split_words_mut` for several disjoint ranges at once),
  and the committed read-write range at the base up to an end, `0..end`, in
  O(1) (`prefix`, `prefix_mut`): the reservation keeps the end of that range
  as a field, refreshed whenever its page map changes, so lending it is one
  comparison rather than a walk of the map. `accessible` and
  `accessible_mut` lend that whole range with no check at all, its end
  being the field -- kept in words beside the byte end, so that lending
  it takes no shift either -- so what indexes the slice makes the
  access's one bounds check, against the committed end: the activation
  stack's register access, which the run loop makes on nearly every
  instruction, where the shift cost `bench_loop.ns` about 2%. Dropping
  the reservation releases it. Ranges are byte offsets from the
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
  regions and the object space needs no other; `split_words_mut` is unused
  by the heap. The activation stack re-borrows its whole accessible range
  (`FrameStack::word`, `words`) and indexes it by a word's offset, so
  only an offset at or past the committed end finds no word; that an
  offset lies below the top, in a live frame, is the stack's to keep,
  which a debug build checks ([execution.md](execution.md),
  "Representation").
- **A bump cursor**: the reservation holds one cursor, a top and a limit
  in byte offsets, and eden's young allocations take their memory from
  it ("Inline allocation"). Its two words lie in an allocation of their
  own, `Cell`s behind an `Rc` (`SharedCursor`), the top word then the
  limit word, whose address `bump_cursor_address` exposes: machine code
  bumps the cursor in place between two calls into Rust, while its
  caller holds the reservation's owner by `&mut`, so the words cannot lie
  in the reservation's own bytes, for the reason the heap's machine words
  lie apart ([execution.md](execution.md), "The machine words"); machine
  code moves the top only by a bump that ends within the limit, as
  `bump` does. `set_bump` places it and refuses
  an unaligned offset, a top outside the reservation or a limit past the
  committed read-write prefix; a limit below the top is allowed and
  refuses every bump. `bump` is one comparison of the words asked for
  against what lies between the top and the limit, and lends exactly
  those words, committed and initialized, as a `&mut [u64]`;
  `bump_array::<COUNT>` does the same for a fixed count and lends a
  `&mut [u64; COUNT]`, so a caller's stores carry no index check;
  `bump_up_to` is the cold bump of any size up to a ceiling inside the
  prefix, and lends nothing. Every change of the page map clamps the
  limit to the prefix (`record`), so no `decommit` or `protect` leaves a
  bump able to lend memory that is no longer committed. The `unsafe` the
  lending takes stays here, beside `words_mut` and `prefix_mut`.
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
- **Executable memory for the JIT tiers** is in the same crate, with its
  `unsafe` entry into generated code (`code_zone.rs`): a code zone of at
  most 128 MiB mapped twice, one view executable and one writable
  (`MAP_JIT` with its per-thread switch on Apple AArch64), the one writer
  of code, which commits pages as code is installed and flushes the
  instruction cache over what it wrote and gives pages back above an
  end (`decommit_above`, "Giving committed memory back"), the entry, the block the entry
  hands machine code (`EntryBlock`, `Cell`s behind an `Rc`, kept from
  one entry to the next), and the shim through which machine code calls
  Rust ([execution.md](execution.md), "The baseline JIT's foundation",
  "The entry at a send", "Executable memory"). The x64 Windows path
  runs in every gate, the Linux path, x64's and AArch64's, in the gate
  groups `linux` and `aarch64` at the end of a batch, AArch64's under
  QEMU; the BSD and Apple AArch64 paths are checked by clippy alone (the
  `memory` gate). The bounds of the running thread's
  native stack are read there too (`native_stack.rs`), for the entry's
  check of its room ([execution.md](execution.md), "The native stack
  carries the return addresses").

### The cells' reservation

The inline caches' cells -- every site's, every polymorphic block slot's,
and the global cache's mirror ([dispatch.md](dispatch.md), "The cells
beside the entries") -- lie in one `Reservation` of exactly 1 GiB, owned by
the `InlineCaches` (`inline_cache/cells.rs`), outside the mutator, since
machine code reads them between two leaf calls ([execution.md](execution.md),
"The machine words"): the global mirror's 64 MiB at its base, room for
2^20 rows of 64 bytes; the site cells' 192 MiB, 6,291,456 cells of 32
bytes (`MAXIMUM_SITES`); the block cells' 768 MiB, one block of four per
site. A cell's address is fixed for the reservation's life -- the region's
base plus its index times its size -- while the `Vec`s of entries beside
it move as they grow. Each region is committed as it grows, doubling what
it has and at least 64 KiB, or exactly what it needs where the system
refuses more, and `heap::out_of_memory` past that; a page newly committed
reads 0, so every cell a table grows by is written vacant explicitly. After
a full collection the site and block regions give their pages back above
the table's end, the pool's length and a headroom (`Cells::shrink`;
"Giving committed memory back"); the global mirror never shrinks. The
addresses come from `Reservation::base_address`, and every slice Rust
borrows of it is derived afresh, as the object space's are. A cell names
no handle, so nothing traces it.

### The application id table

Machine code reads what it needs of an application by its index
(`heap/application_ids.rs`): one `Reservation` of 2^22 rows of four words,
its address and the number of rows recorded in the machine words
(`ApplicationIds`, `ApplicationIdRows`). Row `index` holds, of application
`index`:

1. the bits of its instances `DispatchId`;
2. its instances' header word, which `AllocateInstance`'s template
   writes, or 0 where `allocate_for_factory` makes no ordinary instance
   of the application's count: an instantiation refusal, or a count that
   needs an overflow word (`application_ids::Row::instance_header`);
3. its own slot base (`MixinApplication::own_slot_base`);
4. its flags, bit 0 set when its instances' slots take §3.1.1's
   check of what a value's slot may hold (`inherits_value`).

The megamorphic stub reads the id at a `super` or private lexical site,
whose guard is S's or N's application word while the global cache is
keyed by dispatch id (decided on 2026-10-29, so that no site kind is left
to the Rust probe). The own-slot instructions read the slot base of the
level their frame was entered at, as `Heap::activation_slot_base` does,
and `InitializeSlot` the flags of its receiver's class's application,
whose `inherits_value` `check_value_slot` reads -- the receiver's, not the
entered level's, since a `<:` clause can apply `Value` below the level
(execution.md, "The baseline JIT's foundation", "Own slots in machine
code"). It has three writers:
`Heap::register_application` records an application's row when it is
registered, `Heap::free_application` records zeros -- the id
`DispatchId::NONE` -- when a full collection reclaims the application,
and `Heap::truncate_application_rows` takes the bound back with the table
of applications, after which the end of a full collection gives the
pages past the bound and a headroom back (`ApplicationIds::shrink`;
"Giving committed memory back"). An application's row never changes after it is built,
and a class object's application word, which `class_set_application`
changes, is read afresh by machine code, so nothing else writes it. A
word that is no `SmallInteger`, or an index at or past the bound, names
no application: the stub goes to the miss leaf, an own-slot instruction
exits and `InitializeSlot` takes the generic leaf, each of which
decides.

### The allocator of the Rust side

Everything Rust allocates -- the syntax tree, the resolver's and the
compiler's tables, the compiled code, the image decoder's buffers, the
analyzer's documents -- goes through the global allocator; the Newspeak
heap is the reservation above and never does. Both binaries, `ns-next` and
`ns-next-analyzer`, set it to mimalloc (`#[global_allocator]` in each
`main.rs`; the dependency in the workspace manifest). A library crate sets
none: a global allocator is the whole program's, so one set in
`ns-syntax` or `ns-runtime` would be imposed on every program linking it,
the test binaries included, which run on the platform's allocator.

Rust's default on Windows is the process heap (`HeapAlloc`), which the
front end, allocating and freeing a node at a time, pays for heavily.
Measured on 2026-10-17 on the measuring workstation (Windows 11, release
builds with the alignment pin, three builds a side): `parse` over the 211
corpus modules (`cargo bench -p ns-syntax --bench corpus`) takes 92-96 ms
on the process heap, 54 ms under mimalloc, 56 ms under snmalloc, 53 ms
under rpmalloc and 168 ms under the segment heap a manifest can select;
`ns-next-analyzer parse-corpus corpus/modules` 164 ms against 77 ms; a boot
from source (`hello.ns` under `NS_NEXT_NO_PRIMITIVE_METHODS=1`) 14.9 ms
against 11.8 ms; a boot from the embedded shipped code 4.9 ms against 5.0
ms, the one case mimalloc measures slower. Of the benchmarks, under
mimalloc: `bench_block` -11 %, Bounce -8 %, Storage -7 %, CD and DeltaBlue
-5 %, Json and Havlak -5 %, `bench_megamorphic` and DeltaBlueTon80 -3 %,
List +3 %, the others within their spread.

It costs memory: mimalloc keeps the segments it took resident rather than
handing them back at once. Peak working set
(`PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize`, read after the process
exits; same builds) of `hello.ns`, `bench_loop.ns` and `bench_send.ns`
under `bench --iterations 10` 6.0 MB against 7.3 MB, `bench_block.ns`
13.9 MB against 15.1 MB, Richards 30.7 MB against 31.2 MB, `run-corpus
corpus/modules` 6.3 MB against 7.5 MB, `ns-next-analyzer parse-corpus`
19.5 MB against 28.7 MB; the peak commit charge (`PeakPagefileUsage`)
rises by about 7.5 MB in every `ns-next` case and by 21 MB under
`parse-corpus`. A megabyte or so of resident memory and some reserved
commit per process against a fifth of a boot from source and half the
analyzer's parse: a trade made for these two binaries, which run one
program or one command and exit, or serve one editor.

mimalloc over snmalloc and rpmalloc, which measured the same: it is the
most used of the three. It is C (compiled as C++ under MSVC) built by
`cc`, so the build needs a C compiler, which every target's Rust toolchain
here already has beside its linker, and it links `advapi32` and a few
other system libraries on Windows. The corpus benchmark runs on the
platform's allocator, so that it still shows what the front end allocates.

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
  found eden spent -- eden's top being the bump cursor of the reservation
  (`Space::bump_object` on the fast path, "Inline allocation";
  `Space::allocate_young` and `allocate_old` through `Heap::allocate_block`
  on the cold one; "Generations") -- as Soup's `Heap::Allocate`
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
  registers inline. Married and divorced frames, and suspended ones, keep
  their meaning ("Frames, the activation stack and the marriage"); a frame
  object is allocated with its code's full register count but the header's
  numbers, so none ever
  grows, and whether a divorce copies the registers stays decided per
  activation from the block's `outer_reach`. The activation stack's layout
  stays as it is until road step 7a lays frames out as words in one
  reservation.
- **The full collector is mark and sliding compaction** over the variably
  sized objects of both generations, after Spur's planning compactor,
  which slides marked objects
  down and plans their new addresses before moving any (`compact`,
  `planCompactSavingForwarders`,
  `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:61596`, `:62399`).
  The mark bit is in the header ("Marking"); the forwarding addresses live
  in a side table, a live-word bitmap with a prefix count per 64 words,
  whose buffer is kept between collections and whose chunks a collection
  writes only where the space is occupied, where Spur saves each object's first field to make
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
  stress mode"). The other five stress modes are in "The gates the
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
3a, 3b and 4 are done: the crate list names `crates/ns-memory`, the one
crate with `unsafe`, and the heap's space in its reservation, a nursery of
eden and two survivor spaces below the old space, a Cheney scavenger beside
the mark-and-slide collector, which compacts both generations, and the
barrier in `heap.rs`'s store accessors; "the crate forbids unsafe code"
names `ns-runtime`; "An image is untrusted input" states the figures of
"The amplification bound of a rejected load" above, and that the decoder
allocates only after a tenure between entries, the platform-path sends may
collect at their safepoints like any run, and the graph pass allocates old,
after the last tenure, and never polls a safepoint, so the peak argument
stands; the handle paragraphs describe a 64-bit byte offset and name no
frame arena; "A cache records what the lookup decided" reads the builtin
tag as a field of the class object, which `act` still reads afresh on every
send; the end-of-batch gates name thirteen stress modes, `SCAVENGE` among them;
and "Every store into an existing object goes through `heap.rs`'s store
accessors and their write barrier" is an invariant of its own; and "The
VM mirror is a capability" says that the MirrorBuilder refuses every edit
to a shipped mixin, in `asBuilder` and again in `prepare_edit`, and
resolves an edit with `resolve`, never `resolve_kernel`, which seals the
mirror's class (road step 5b2a). No program reaches `become:` already, since it is a
leaf on the mirror, and the leaf refuses every shipped instance, builtin
instance and withheld value itself ([object-model.md](object-model.md),
"`become:` is an eager bulk forward", "What the forward refuses"). "Decided
per activation from the block's `outer_reach`" stands, since the divorce
keeps that rule.

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

- **A young allocation bumps eden** (`Space::bump_object` while eden is
  below its threshold, `Space::allocate_young` past it; "Inline
  allocation"). Eden asks
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
  class, every word of the activation stack, the side tables of
  continuations and of suspended activations, the host roots, and the step
  state's
  `StepRoots` -- plus the remembered set. The whole
  activation stack is a root, so a scavenge costs time in proportion to the
  recursion depth, up to the depth limit's 100,000 activations and its
  1,000 reserve; Spur bounds that with stack pages, and road step 7a's
  frames as words revisit it. Each remembered object leaves the set, its bit
  cleared, has its strong pointer words scanned, and rejoins only if it
  still names a young object, a weak word included once the weak objects
  are settled.
- **The Cheney loop** then scans the strong words
  (`strong_pointer_ranges`) of what was copied, in the future survivor
  space and in the old space from where each top stood when the scavenge
  began, until neither grew; a promoted object that still names a young
  one joins the remembered set.
- **Forwarders.** A copied object's old place becomes a forwarder: its
  header's format the forwarded one, its class word naming the copy, the
  format the bulk forward writes too, with the guard `FORWARDED`
  ([object-model.md](object-model.md), "`become:` is an eager bulk
  forward"). A reference to a forwarder is followed along the whole chain
  (`evacuate`), and the object at its end is copied only if it is young and
  not already in the future survivor space, as Spur's
  `scavengeReferentsOf:` decides (`cointerp.c:43278`, the
  `isReallyYoungObject:` test at `:43316-43322`). A young object is copied
  once per scavenge, and nothing names a bulk forward's forwarder once the
  forward ends, so every chain a scavenge meets is one link of its own. A
  bulk forward's old forwarder that is remembered -- its class word names
  a young replacement -- stays in the set until its target is tenured or a
  full collection runs; its class word is the only word a scavenge reads
  of it, which keeps the replacement alive at most as long as it is
  remembered. The copy takes the whole allocation, its overflow word
  included, and moves a `ByteArray`'s data to the copy's alignment
  ("Every mover realigns a `ByteArray`'s data" below).
- **Where a survivor goes.** To the future survivor space, its age one
  higher, or to the old space, its age 0, when the scavenge tenures
  everything, when its age is at least the tenuring threshold, or when the
  survivor space is full.
- **The precommit.** Before it copies anything a scavenge commits the old
  space for the worst case, every object in the past survivor space and in
  eden promoted, so a promotion never meets a refused commit halfway
  through; a commit the operating system refuses aborts the process as
  running out of memory before the scavenge begins.
- **Then the weak objects are settled** (`Scavenger::finish`): the
  ephemerons whose key was copied meanwhile are scanned and the Cheney
  loop runs again, until no copy waits to be scanned -- the first two
  checks scanning every unresolved ephemeron, the rest of the fixpoint
  indexing those still unresolved by the young objects whose copy would
  resolve them and looking each object it copies up once, recorded only
  while the index holds an unresolved ephemeron; the ephemerons left
  unresolved are mourned; and every young word of a weak object is
  forwarded to the copy or set to `nil` ("Weak references").
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

`World::scavenge` lends the heap and `StepRoots` side
by side, and times the scavenge as `World::collect` times a collection;
`--collector-statistics` reports the scavenges and their pauses beside the
full collections and the table collections ("The gates the collector
needs").

### Every mover realigns a `ByteArray`'s data

A `ByteArray` is an object of the format `AlignedBytes`
([object-model.md](object-model.md), "The object layout"): its header,
its class word, three reserve words, then its data words, the data
beginning at the first 32-byte address at or after the first field word
-- 0, 8, 16 or 24 bytes in -- so that a later leaf over a whole typed array
can load it with AVX2's width and no load crosses a cache line
([core-library.md](core-library.md), "`ByteArray` in the heap, the movers
and the image"). Where the data begin is computed from the header's word
index, never stored (`heap/object.rs`'s `aligned_data_start`, the first
field index rounded up to a multiple of four; the space's base is
page-aligned, so a word index that is a multiple of four is a 32-byte
address), so no header bit records it and the object's size never changes
when it moves: the compaction's forwarding counts words.

The price is that **every mover that copies an `AlignedBytes` object to
another address moves its data within the new block to the new block's
aligned position** (`object::realign`, a `copy_within`, since the old and
the new offset can overlap): the scavenger's `copy` (`heap/scavenge.rs`),
which knows the old and the new header index; the compaction's slide
(`heap.rs`), likewise, the slide copying downward so the words inside the
new block are all in place when it realigns; and the move stress mode
(`Heap::move_everything`), which shifts a whole region by one distance and
then walks it, realigning each `AlignedBytes` object from its old header
index, the new one less the shift. A mover that skips it leaves the data
at the old offset within the block, and every read answers wrong bytes at
exit status 0 -- an invariant of `CLAUDE.md`'s. It is the first format
that needs a per-object fix-up in the movers; slack inside the object
touches the fewest invariants, where an aligned bump with `Free` fillers
would break the compaction's dense forwarding. The image decoder places a
`ByteArray`'s data at its own placeholder's alignment as it fills it.

The heap's readers `debug_assert!` the alignment, and
`an_aligned_byte_array_keeps_its_bytes_through_every_mover_at_every_position`
(`heap.rs`) holds the bytes through a scavenge, a compaction and the move
stress mode at each of the four positions; the move stress mode's varied
shifts reach all four in the suite too. Bytes are no handles: the tracer
never sees them, and storing a byte takes no write barrier.

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

The barrier sits in `heap.rs`'s store accessors, never in `vm.rs`, so the
interpreter and the reference lowering share one rule, and machine code's
inline barrier, below, makes its comparisons and calls its `remember`.
Every store the interpreter makes into an existing object goes through one
of them:

- `Heap::set_pointer_word`: an instance's fields, an array's elements and a
  class object's fields;
- `Heap::object_set_class`: the class word;
- `Heap::frame_set_local`: a divorced frame's register, which
  `StoreOuterLocal` writes through its frame;
- `Heap::pop_activation`: the divorce, which copies a retaining
  activation's registers into its frame object and remembers an old frame
  that one of them makes name a young object ("Frames, the activation
  stack and the marriage").

Machine code makes the same decision inline. A store into an object it
did not allocate in the same template -- `StoreVector`'s, a setter
stub's, a private or `super` setter's, an Array's `at:put:` stub's,
`InitializeSlot`'s and `InitializeSlotProvisionally`'s, and an outer
local's in a divorced frame -- takes
`MacroAssembler::store_with_barrier`: the word, then the same two
comparisons against `NURSERY_END` behind an early out for a value that
names no object, and one for a holder whose remembered bit is set
already, which `remember` would answer at once; only past all four does
it call `Leaf::Remember`, which is `remember` itself
([execution.md](execution.md), "The baseline JIT's foundation"). Its
initializing stores -- `MakeArray`'s elements, a closure's copied
values, the fields of a frame object a `MakeClosure` reifies -- take
none, for the reason `allocate_block`'s callers take none: the object is
young, or old and born remembered, since only `Leaf::Allocate` and the
generic leaf place one old, through `allocate_block`. An outer local in
a married frame lies on the activation stack and takes none either.

`allocate_block` remembers every old object it allocates outside the
decoder, as above. A married frame's registers live on the activation
stack, which is a root, and take no barrier; nor does a store into a young
object. The collector's own rewrites -- the scavenge, the compaction, the
move -- keep the remembered set themselves, and so does the bulk forward
(`Heap::forward_identity`), which writes words without the barrier beside
them: it rewrites words in place as a compaction does, and after each
holder's rewrite remembers every old one that names a young object now
(`IdentityForward::holder_done`) ([object-model.md](object-model.md),
"`become:` is an eager bulk forward"); the replacement of resolved futures,
which may store a young value into an old object, runs only before a full
collection, which empties the nursery and the set; and the decoder's fills
store old values into old objects. The debug check of the stored Rust-side
value's heap id and epoch, which every store makes (`Heap::store`; "The
object space"), holds here too. A store that writes a word into an old
object any other way leaves the young object it names unreachable to the
next scavenge, silently; that is the invariant `CLAUDE.md` states, and the
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

Every load bootstraps, which builds `Kernel` and `PlatformFactory`, and
then builds the shipped modules and classes the image names that the boot
did not (`Interpreter::build_shipped_names`), all before it decodes the
graph, so every object an image can name exists, young, when the graph
pass starts; the first phase, which reads everything up to the graph,
allocates none, and the builds after it allocate as a program's first
demand does. One scavenge that tenures everything, between two entries and
after the builds (`Interpreter::tenure_now`), makes the shipped world old,
and the graph pass then allocates old, its stores old-to-old (decision 4;
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
naming a young one carries the bit -- by any word `pointer_ranges` names,
a weak one included, since the next scavenge must forward or clear it.
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
   load into a heap that holds nothing young; here the bootstrap's objects
   and those of the shipped modules and classes the loader builds for the
   image's names are young when the graph pass starts, and one tenure
   between two entries, after those builds, makes them old. The decoder
   then allocates its graph old, as Soup's `AllocateSnapshot` does
   (`prior-art/primordialsoup/vm/heap.cc:228`). One tenure suffices because
   it comes last: the first phase, which decodes up to the graph, allocates
   nothing, the builds all run before the tenure, and nothing between the
   tenure and the graph pass allocates.

The plan also chose constants for the user to confirm, each one line to
change and each stated where it is built above: eden's 512 KiB reserve,
the eden-spent rule, `REGION_SLACK`, the old space's first commit past the
nursery, a scavenge that leaves resolved futures alone, and the
remembered-set verification under the stress mode only
([OPEN-WORK.md](../OPEN-WORK.md)).

## Inline allocation

Road step 6 (decided with the user on 2026-10-22): a young object is
allocated by one comparison and a bump, its header precomputed, and only
what that cannot serve takes the general path, `Heap::allocate_block`,
which is `#[cold]` and `#[inline(never)]`. Every system of the prior art
has this fast path -- one compare of the new top against a limit, the
header precomputed, the identity hash left 0 until first requested:
Cog's interpreter compares `freeStart + numBytes` against
`scavengeThreshold`
(`prior-art/opensmalltalk-vm/src/spur64.cog/gcc3x-cointerp.c:51349-51358`)
and its `ee` allocators for closures, floats and contexts leave the body
to the caller's stores (`:48869-48918`); Strongtalk bumps eden with one
compare (`prior-art/strongtalk/vm/memory/space.hpp:143-152`); Primordial
Soup checks the size and the top
(`prior-art/primordialsoup/vm/heap.h:373-387`). Strongtalk and Soup
collect inside the allocator when the bump fails, which ns-next does not
("Where a collection can happen: only at a safepoint in the run loop");
Cog's shape, in which an allocation only requests a collection, is the
one followed here. It is also the shape the JIT tiers take over, as
Cogit's `genPrimitiveNew` and `genCreateFullClosure:…` do in machine code
(`cogitX64SysV.c:16464`, `:19026`).

### The fast path

- **Eden's top is the reservation's bump cursor** ("The memory layer"),
  not a second copy of it: the space keeps eden's start and its threshold
  (`Space`'s `eden_start` and `young_threshold`) and reads the top from
  the cursor.
- **The fast limit lies 8 bytes below the scavenge threshold**:
  `min(eden_start + threshold - 8, NURSERY_END - REGION_SLACK)`, placed by
  `Space::place_eden_top`. An allocation that reaches the threshold
  therefore always takes the cold path, which requests the scavenge, and
  the fast path requests nothing; and the limit never lies within
  `REGION_SLACK` of eden's end, which the move stress mode needs ("The
  layout and its constants"). While the image decoder allocates old
  (`Heap::set_allocating_old`) there is no threshold and the limit is
  `eden_start`, so every fast bump fails and the decoder allocates old
  and not remembered ("An image load tenures once"). The limit is placed
  again by everything that changes eden's start, its top or the
  threshold: `Space::set_young_threshold` (from `set_allocating_old`, and
  from the threshold a debug-build test sets), every placement of eden's
  extent (`Space::set_region_extent`, which the move stress mode's shift
  uses), the survivors' flip after a scavenge and the nursery emptied by a
  full collection.
- **What takes the fast path**: every young object whose header holds its
  word count -- at most 254 words after the header, so no overflow word --
  while the bump stays within the limit. `Space::bump_object` bumps the
  cursor, writes the header word and the class word, and lends the fields
  to the caller; `bump_object_array::<BLOCK_WORDS>` does the same for a
  fixed size and lends the block as an array. Every typed allocator tries
  it first -- `allocate_object`, behind which every kind without a fast
  path of its own lies, `allocate_instance_with`, a closure, a frame and a
  float -- and calls `allocate_block` when it answers `None`.
- **The cold path is `allocate_block`, which decides everything else**:
  bumping into eden's reserve past the threshold and requesting a
  scavenge, eden spent going old born remembered and requesting the
  scavenge, a large object going old, an overflow word, the old space's
  collection trigger ("Allocation: eden, its reserve, and what goes
  old"). It bumps the same cursor up to a ceiling `REGION_SLACK` short of
  eden's end (`Reservation::bump_up_to`).
- **Why the limit lies below the threshold**: a fast path that bumped
  into the reserve would need a second comparison and the request inline,
  and inlining `allocate_block` itself would keep its branches and could
  not precompute the header.
- **Machine code bumps the same cursor, against the same limit.** The
  cursor's two words, the top then the limit, lie in an allocation of
  their own (`ns_memory::SharedCursor`, "The memory layer"), whose
  address the entry block names (`BlockWord::EdenCursor`), so machine
  code reads and writes them between two calls into Rust as the
  interpreter's fast path does (`MacroAssembler::bump_allocate`).
  `MakeVector`, `MakeArray`, `AllocateInstance`, `MakeClosure`, the
  frame object a twin's `MakeClosure` reifies and `MakeFuture` write
  the header and the class word from the machine words, where the
  interpreter's precomputed headers are copied
  ([execution.md](execution.md), "The machine words"), and fill or store
  every field before the next instruction, as the fast path does. Past
  the limit, or for a block at or past the large-object threshold, they
  call `Leaf::Allocate`, which places the object as `allocate_block`
  does and never collects, or the generic leaf, which runs the
  interpreter's own allocation; either requests the scavenge, which runs
  at the next poll's exit. The objects machine code places are counted in
  the machine word `MachineAllocations` and folded into the heap's counts
  before a scavenge or a compaction recounts them, so the heap's
  statistics stay the interpreter's. Cog's machine code carries on past
  its threshold into the slack and calls a trampoline that only schedules
  the scavenge (`cogitX64SysV.c:19053-19077`); here the limit already
  lies below the threshold, so the leaf is the interpreter's own
  decision, and no word of machine code holds the threshold.

### The headers precomputed

- **An ordinary instance's header is on its `MixinApplication`**
  (`instance_header`, an `InstanceHeader`): its guard, `FixedPointers`
  and its word count, computed by `Heap::register_application` from the
  row's `dispatch_ids` and `slot_count`, which never change after it (an
  edit makes a new row); `None` when the count does not fit the header.
  The class word comes from the class object at every allocation, since a
  class object is per enclosing instance. The factory body's allocation
  (`instantiate::allocate_for_factory`) reads the header beside the
  application's refusal and slot count and allocates through
  `Heap::allocate_instance_with` ([object-model.md](object-model.md),
  "Instantiation").
- **A closure's and a frame's guard are on the heap** (`BuiltinGuards`,
  set by `Heap::set_builtin_guards` where
  `Mutator::refresh_immediate_dispatch_ids` runs: once when the builtin
  classes are built and again when the kernel gives them their
  applications). **A boxed `Float`'s** is the guard an immediate float
  dispatches on (`FLOAT_DISPATCH_INDEX` of the immediate dispatch ids),
  since both are instances of `Float`. Their word counts come from the
  object: a closure's fixed fields and its copied values, a frame's fixed
  fields and its body's register count, a float's two words. A debug
  build asserts that each precomputed guard is the one
  `Heap::header_dispatch_id` would have looked up.
- **What the fast path writes**: the header word and the class word,
  always. An instance, an `Array` and every object made through
  `allocate_object` are nil-filled. A closure, a frame and a float store
  every field explicitly and are not filled first, as Cog's `ee`
  allocators do; a debug build then asserts that no word of the new
  object is still `header::POISON`, which a scavenge leaves in the
  nursery it vacated. No safepoint and no read lies between the bump and
  the last store. The identity hash stays 0 until something asks for it.
  The `heap` module stays the one writer of a header: `allocate_block` on
  the cold path, `Space::bump_object` and `bump_object_array` on the fast
  one.

### A closure's code id without the map

A closure, a frame object and a stack frame store their body's code id, which
`Heap::register_code` finds in a map keyed by the body's `Rc` pointer. A
body carries a one-entry cache in front of the map (`CompiledCode`'s
`code_id_cache`, a `CodeIdCache`): the id it was last answered and the
code-table epoch of the heap that answered it. Every heap's epoch is
unique in the process (a process-wide `AtomicU64`,
`next_code_table_epoch`), and a heap renews its own whenever a full
collection frees a code row (`Heap::free_unnamed_codes`). A hit of the
current epoch skips the map; a miss runs it and refills the cache. An id
from another heap, or one since freed, is therefore never answered -- the
rule that a handle means nothing against a heap that did not mint it
("A handle is space-relative"), applied to ids -- and a debug build
asserts that a hit names the body's own row. A copy of a body is another
key to the map, so a cloned `CompiledCode` starts with an empty cache,
and the cache is on no wire. It holds an id, never a handle, so a reader
of compiled code still holds no handle ([compiler.md](compiler.md),
"Readers of compiled code take the mixin").

### Arrays without a `Vec`

`Array new:`'s leaf allocates its array nil-filled
(`Heap::allocate_nil_array`), with nothing built beside it; `MakeVector`
does the same, and `MakeArray` copies its registers straight into the
fresh array's elements (`Heap::allocate_array_from_registers`). A length
the program chose is asked of `Heap::array_fits` first: one below the
large-object threshold is allocated young, where eden is committed, and
needs no probe; a larger one is asked of `Heap::fields_fit`, as a
`ByteArray`'s and a `WeakArray`'s size are, and refused with the same
error ("… cannot allocate an Array of … elements"). That probe cannot
tell whether an overcommitting system will later grant the pages, and it
compares the object against the old space's size rather than its free
room ([OPEN-WORK.md](../OPEN-WORK.md)).

### What the path keeps

- **Born remembered**: only `allocate_block` allocates old, so every old
  allocation outside the decoder is still remembered; the fast path makes
  young objects only, whose initializing stores need no barrier ("The
  write barrier").
- **Collection only at a safepoint**: the fast path requests nothing and
  never collects; the request is the cold path's.
- **Poison**: the header and the class word are written before anything
  reads the object, and the debug check above covers the explicit fills.
- **The origin guard and the move epoch**: a handle is still minted only
  by `object_handle` and `frame_handle`.
- **The move stress mode**: the limit is placed again at every move of
  eden, and `REGION_SLACK` stays free past it ("The move stress mode").
- **An image is untrusted input**: the decoder's graph pass allocates
  only through `allocate_block`, since no fast bump succeeds while it
  allocates old, so the amplification bound stands as derived ("The
  amplification bound of a rejected load").
- **`vm.rs` holds no allocation logic**: its arms call `Mutator`
  functions; `make_array_into` and `make_vector_into` call
  `Mutator::new_array_from_registers` and `new_nil_array`, which build no
  list first.

What it moved is [execution.md](execution.md), "What inline allocation
moved".

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
  guard, for the same reason: a stack offset is reused once its frame is popped.
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
  code of the JIT tiers are in the same crate (`code_zone.rs`). Its operating-system calls
  come from `windows-sys` and `libc`, declaration-only crates, rather than
  hand-written declarations whose constants differ per platform and would be
  wrong only on the platforms nobody here runs.
- **The runtime's own weak structures in the space are the canonical
  symbol table and the table of reified mixins** (the latter weak since
  road step 5b2c, 2026-10-20); a program's are `WeakArray`, `Ephemeron`
  and `WeakMap` ([core-library.md](core-library.md), "Weak references").
  §5.1.6 survives the symbol table being weak because
  `Mutator::new_symbol` is the only way to make a symbol object, so one
  that is collected was held by nothing; a mixin's identity survives its
  object being dropped because nothing that could compare the object
  holds it any more, and the next reflection makes a fresh one.
- **A weak word is forwarded like a strong one and traced for liveness
  apart** (built on 2026-10-16): `pointer_ranges` stays the one
  enumeration of pointer words, which forwarding and the moves need whole,
  and `strong_pointer_ranges` leaves the weak words out for marking and
  the scavenger, so the two cannot drift apart. The ephemeron fixpoint,
  mourning and clearing run before compaction and before the nursery is
  vacated, Primordial Soup's order (`vm/heap.cc:627-682`), since a word
  left naming a dead object would be forwarded to garbage or name what is
  allocated next.
- **The ephemeron fixpoint scans its pending ephemerons in its first two
  checks and, if it goes on, indexes those still unresolved by key**,
  where Primordial Soup (`vm/heap.cc:853-911`) and Spur
  (`processEphemerons`) rescan them in every round: only the
  fixpoint's own drains after the switch record what they mark or copy,
  and only while the index holds an unresolved ephemeron; each recorded
  object is looked up once and each indexed ephemeron resolved once
  (`heap/pending.rs`), so a chain of ephemerons each keyed by the previous
  one's value costs its length rather than its square, and the ordinary
  marking and Cheney loop pay nothing for it -- the marking's drain takes
  the recording as a parameter of its type, the scavenger's copy tests one
  flag ([core-library.md](core-library.md), "The heap and the
  collectors"). The two scans are what a weak map's fixpoint needs:
  measured on 2026-10-17 by the `measuring-performance` protocol, three
  pinned release builds a side, twenty runs of a source that fills a
  `WeakMap` with 20,000 held and 20,000 dropped keys and collects once,
  indexing from the first check paused its scavenges 45.0 ms in all and
  its collections 49.9, the two scans 37.8 and 44.3, as the rescan in
  every round did (37.6 and 44.8): every dead-keyed ephemeron entered the
  index before it was mourned. A 4,000-link chain costs 4.4 ms a run
  with the scans, 4.2 without them, and 67.5 with a rescan per round.
- **The application cache is weak, pruned by the ids a full collection
  reclaims** (decided with the user on 2026-10-03, built by road step 5b2c
  on 2026-10-20). Its key was an address, which needs the strong reference
  to keep it unique; since road step 3a it is a pair of the heap's ids, so
  the collection that frees an id can drop the entries naming it
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
  the committed space, kept between collections and written only where the
  space is occupied. Rejected: a sorted list of old and new
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
  loads; the loader's builds of the shipped modules and classes the image
  names run before it and nothing between it and the graph pass allocates,
  so the one tenure covers the bootstrap's objects and the builds'.
- **A refused commit or an exhausted reservation aborts the process**, as a
  failed `Vec` push does, through `std::alloc::handle_alloc_error` (decided
  with the user on 2026-10-03). Allocation stays infallible, so no hot path
  carries a `Result`; a Newspeak-level out-of-memory error is open
  ([OPEN-WORK.md](../OPEN-WORK.md)).
- **A `ByteArray`'s data lie at a 32-byte address, through a reserve of
  three words that every mover realigns** (decided with the user on
  2026-10-12, built on 2026-10-13): AVX2's width, so no 32-byte load
  crosses a cache line, where 16 would let one in four do so and 64 would
  cost seven words an object; computed from the object's address and never
  stored, so the object's size stays the same wherever it moves; slack
  inside the object, where an aligned bump with `Free` fillers would break
  the compaction's dense forwarding ("Every mover realigns a `ByteArray`'s
  data"). A size the program chose is asked first whether the reservation
  and the operating system can hold it (`Heap::byte_array_fits`), so that
  `ByteArray new:` refuses with an `ArgumentError` what would otherwise
  abort the process.
