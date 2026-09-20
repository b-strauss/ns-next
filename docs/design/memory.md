# Memory — object lifetime, identity, collection

> **Status:** partial. Objects live in an arena owned by one `Heap`,
> addressed by 32-bit handles; activations live on an activation stack in
> the same `Heap` and reach the frame arena only when a block captures one.
> Identity, layout ownership and the seam that routes every access through
> `Heap` are built. Nothing reclaims an arena slot: there is no collector.
> The root discipline one will use, in "Checked roots and a collector", is
> built in all three of its parts: nothing re-enters the run loop from Rust,
> so no Rust frame is live below a safepoint (step 2a); the run loop polls a
> safepoint, every handle the collector would trace is enumerated, a host
> keeps values across runs only as `Root`s, and a debug-build stress mode
> moves everything at the safepoints to check all of it (step 2b); and every
> movable handle carries a lifetime brand, so that holding one across a
> safepoint is a compile error (step 2c). Only the collector is not built.
> **Code:** `crates/ns-runtime/src/heap.rs` (the arenas, the activation
> stack, the handle types and their brand, every accessor, the host root
> table, the move), `heap/world.rs` (the step state at rest, and the entry
> that lends it out), `interpreter/mod.rs` (`Mutator`, and
> `Interpreter::enter` with the doctests that pin the brand), `trace.rs`
> (everything the collector traces), `interpreter/safepoint.rs` (the
> safepoint and the two stress modes), `vm.rs` (one entry per instruction, and
> the poll), `types.rs` (`Value`, `Object`, `Frame`), `mixin.rs` (layout,
> and the one remaining iterative `Drop`).

## What exists

**The class owns the slot layout**, and slot access resolves through the
*current* layout, so a layout change plus instance migration is expressible
rather than something the representation forecloses. Layout lives on
`MixinApplication`; see [object-model.md](object-model.md).

**Identity is stable and hashed from a counter.** Every object's
`identity_hash` is drawn from a monotonic counter the first time it is asked
for, never derived from its address. This is required by identity-keyed
collections, by image roots, and by `become:`; an address-derived hash would
break the moment an object moved or an identity was swapped.

**Objects live in an arena and activations on a stack, not in
`Rc<RefCell<_>>`.** One `Heap` owns `objects: Vec<Object>`, the activation
stack every running activation lives on, and `frames` — the arena of *reified*
activations, the ones a block captured or the image decoder rebuilt. An
`ObjectReference` or a `FrameReference` is a 32-bit index into an arena, an `ActivationReference`
into the stack. The representation is described below, and the stack in
[execution.md](execution.md), "Activations". Nothing reclaims an arena slot: a
cycle is no longer the special case that leaks, because everything in an arena
is retained, deliberately, until a collector exists. The stack is the exception:
an entry is popped when its activation returns, and one that was never reified
leaves nothing behind. That is not a problem the image format inherits: the
format is defined against the *logical* value model rather than against this
representation, so replacing it again would not break images.

### The representation: an arena of typed handles

The model is the Nova JavaScript engine's: a `Value` is a Rust enum carrying
either its data inline or a 32-bit index into a typed arena, with no `Rc` and no
`RefCell` anywhere in it. Nova is the useful comparison rather than V8 because it
solves this problem in Rust, where the borrow checker is available as an
enforcement mechanism and a C++ engine's conventions are not.

```rust
Value  = Nil | Boolean(bool) | SmallInteger(i64) | Float(f64) | Object(ObjectReference)   // Copy
ObjectReference        { index: u32 }   // private to heap.rs; 4 bytes in a release build
FrameReference      { index: u32 }   // likewise; ClassReference is an alias for ObjectReference
ActivationReference { index: u32 }   // likewise, into the activation stack

Heap { objects: Vec<Object>, frames: Vec<FrameSlot>, stack: ActivationStack,
       next_identity_hash: u32 }
```

What that removed is not an abstract elegance but part of the measured cost of
the previous model, itemized in [execution.md](execution.md)'s measurements
section: the `Rc` box and its reference-count traffic on every frame link and
every shared handle, and a `RefCell` borrow on every single register read and
write. A register access is now a slice index into an owned `Vec`. Over
`bench_loop.ns` on a release build that measured as **19.4–19.6 ms before,
16.4–16.8 ms after** — about 15%. The rest of the cost of an activation nothing
captures — a `Vec` apiece for its arguments and temporaries, and an arena entry
— went with the activation stack. A captured activation still takes an arena
entry for its reified frame, and two vectors when it copies its registers out
on return — which, since closure conversion, only an activation whose block can
still reach them by reference does. [execution.md](execution.md) has both:
"Lifecycle" for when the registers are copied, "What the activation stack
moved" for the figures.

`ObjectReference`, `FrameReference`, `ActivationReference` and `Value` are `Copy`. A handle has no
destructor and nothing to clone, which is why `Value` could become `Copy` at all,
and it is what makes an arena entry cheap to hand around.

Three properties follow from an index that did not follow from a pointer.

**A slot's index is known before the slot is filled.** Building an object that
names itself — a bootstrap class whose own class is itself — used to need
`Rc::new_uninit` and `Rc::from_raw`, which were the crate's `unsafe`. `Heap`
reserves the index, then writes the record that mentions it, in safe Rust.
The crate now forbids `unsafe` code outright (`#![forbid(unsafe_code)]` in
`lib.rs`; "The brand" below says why it needs none).

**There is no reference chain to unwind.** See "Iterative teardown" below.

**A handle is only meaningful against the heap that minted it.** See "A handle
is arena-relative" below.

An arena is also closer to the shape the image format already writes. Handles are
architecture-neutral by construction, which is requirement 4 in
[liveness-and-reflection.md](liveness-and-reflection.md), and identity hashes
already come from a counter rather than an address, so nothing about identity
depends on where an object sits.

### A handle is arena-relative

An index is meaningful only against the `Heap` that allocated it. That was the
open question the arena had to settle, and this is the answer: not globally
unique, arena-relative.

Reading a handle through the wrong heap is the one new failure mode the arena
introduces, and it is silent — an index minted by another heap is in range and
names a different, perfectly valid object, so the read answers the wrong thing
rather than failing. A `debug_assertions`-only guard makes that explicit rather
than conventional: each `Heap` carries a distinct id, each handle records the id
of the heap that minted it, and every accessor that takes a handle asserts the two
match — 69 sites in `heap.rs`, 16 of them through an `ActivationReference`, whose check
also compares a generation, because a stack index is reused as soon as its entry
is popped. An `ObjectReference` or a `FrameReference` also carries the heap's move epoch, which
the same check compares ("Checking the roots before a collector exists: the
stress mode" below). In a release build neither the fields nor the checks
exist, and a handle is exactly its 32-bit index.

The room the actor milestone needs is still there. Actors are designed to get one
heap each so that pauses stay local
([liveness-and-reflection.md](liveness-and-reflection.md)), which will ask how a
handle encodes *which* heap. A handle is 32 bits and its field is private to
`heap.rs`, so widening it to carry a heap tag is a change to one module — and the
debug guard is already the place that knows a handle has an origin at all.

One deliberate escape exists, and it is opaque rather than a hole.
`Heap::object_identity_key` and `Heap::frame_identity_key` are `pub(crate)` and
return the index as a `usize` for the two places that need a handle as a
`HashMap`/`HashSet` key rather than a comparison: `image/encode.rs` and
`image/decode.rs`, six call sites in all. Every
one stores it, compares it, or looks it up; none does arithmetic on it, and none
assumes it is an index into anything. So the property that matters survives — a
handle's *encoding* is still `heap.rs`'s alone to change — but the narrower
claim that nothing outside the module ever sees the bits is not true, and these
two are where to look first if the encoding changes.

They sit on `Heap`, not on the handle, so that the origin check applies to them
like it does to every accessor. Handed a handle and no heap, the operation has
nothing to check against, and two handles from different arenas then produce the
same key and collide silently in a caller's map. Folding the debug-only `origin`
field into the key instead was tried and reverted: that makes the collision
impossible in exactly the build that would have reported it and leaves it
exactly as possible in the build that ships.

### Iterative teardown

Rust has no iterative drop glue, so releasing a deep reference graph recurses and
a sufficiently deep one aborts the process. Since an image is untrusted input and
its graph depth is attacker-controlled, that is not acceptable.

Under the arena, objects and activations have no such graph: an `Object` holds
handles, a handle holds an index, and dropping the `Heap` drops flat `Vec`s — the
two arenas and the activation stack's two vectors. The hand-written iterative
`Drop` implementations they used to need are gone.

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
walks the graph over an explicit stack, and `metaclass_of` walks its chain
iteratively, because an image controls both depths.

### The seam

`crates/ns-runtime/src/heap.rs` is the only place that knows what a handle is.
Every read and write of an `Object`, a `Frame` or a running activation goes
through a `Heap` accessor, so replacing the store behind them touches that file
rather than the 193 call sites (109 under `src`, 84 in the integration tests) the
workspace held before the seam existed. Replacing `Rc<RefCell<_>>` with the
arena is the evidence that this works: **no call site outside `heap.rs` and
`types.rs` changed** for either the frame conversion or the object one.

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

**The seam is compiler-enforced.** `ObjectReference`, `FrameReference`, `ActivationReference` and the
`ClassReference` alias are declared in `heap.rs` with a **private** inner field, so
reading a handle's index from anywhere else — any other module of `ns-runtime`
included — fails to compile. There is no grep gate to remember and no
visibility to widen: an accessor that is missing has to be added in `heap.rs`,
which is where the store is. The one way out is the pair of
`pub(crate)` identity-key accessors described above, which hand the index over
as an opaque key and are used as one — a named exception rather than an
unenforced boundary, and one the origin guard still covers, since they are
accessors on `Heap` like any other.

Where a site needs several fields of one object, frame or activation at once, a
batched accessor answers them together rather than the caller taking several —
`activation_receiver_and_current_class`, `object_snapshot`,
`frame_register_shape` and `frame_snapshot` are the present examples. This is
not only style: the lexical-send head (`sends::decide_lexical_send` today), on
every implicit-receiver send, once took one `RefCell` borrow yielding both the
receiver and the current class;
splitting that into two separate accessor calls measured as a real cost,
recovered by batching them back into one (see [execution.md](execution.md)'s
measurements section). On the activation stack those are two field reads of one
entry rather than two borrows, so the cost batching removes is smaller than it
was; whether it still pays is a measurement nobody has taken.

### Known rough edges in the seam

Carried deliberately rather than fixed. Recorded so that whoever reshapes this
file next starts from a list rather than from rediscovery.

- **The accessor names grew by accretion.** `class_of_object` reads an *object*
  while `class_application` and its neighbours read a *class* — the same prefix
  for opposite directions. `array_at`/`array_length` and
  `instance_field`/`instance_field_count` are parallel operations under
  divergent names. The activation side is uniform: an accessor through a
  `FrameReference` is `frame_*` and one through an `ActivationReference` is
  `activation_*`. The exceptions are verbs that put the noun second — `set_frame_program_counter`,
  `set_activation_program_counter`, `replace_frame` — and the stack operations themselves,
  `push_activation`, `pop_activation`, `top_activation` and `reify`.
- **The `heap` parameter sits first in some signatures and last in others**,
  across `image/`, `compiler/` and `interpreter/`.
- **Copy-out costs, and the accessor that could now avoid them.**
  `builtins/text.rs`'s string dispatch clones the payload on every send, and
  `object_snapshot`/`frame_snapshot` clone each payload twice per encoded
  record. The arena did not remove these, and was never going to: copy-out is a
  property of an accessor's *return type*, not of the store, and `str_value`
  still returns an owned `String`. What the arena changed is that a *borrowing*
  accessor became expressible at all. Under `Rc<RefCell<_>>` one would have had
  to hand back a `Ref` guard, which is unusable at a site that calls back into
  the interpreter; under the arena an accessor can lend out `&str` tied to the
  `&Heap` borrow, and the borrow checker enforces the rest. Whether to add one
  is a measurement, and neither site named here is on a measured path.

## Checked roots and a collector

**Partial:** the safepoint, the tracing, host roots, the stress mode and the
lifetime brand are built; the collector is not. What the collector would
change: a collection could run — only at a safepoint in the run loop — and move
objects. Every handle the runtime holds is already either where a collector can
find and rewrite it, which the stress mode checks, or a compile error, which
the brand makes it. Designed on 2026-09-13 as step 2 of Milestone 4, in three
parts that each landed green and measured on their own: **2a** removed every
re-entry of the run loop from Rust ([execution.md](execution.md), "No re-entry
from Rust"); **2b** is the safepoint, the enumeration of everything the
collector traces, host roots, and a debug-build stress mode that moves every
object at the safepoints and so checks the enumeration against the whole
suite; **2c** brands every movable handle with a lifetime, so that holding one
across a safepoint is a compile error. The collector itself is step 5, and
until it exists nothing requests a collection in a release build.

**The collector has to move objects.** `become:`-style identity swap and object
forwarding are requirements, not conveniences: adding an instance variable to a
class with live instances allocates new-shape instances, copies fields, and
atomically swaps identity. That rules out a conservative non-moving collector,
which makes `become:` hard. A copying or compacting collector can implement it,
and MMTk supports object forwarding.

A moving collector must not only *find* every root but *update* it, and roots
held in Rust locals across a collection are the hard case: under `Rc` those
references kept their objects alive and never needed updating, and under a
moving collector they are roots the collector cannot see and indices it cannot
rewrite. The design answers that in three layers: where a collection may
happen, what may be on the Rust stack when it does, and what the compiler
checks.

### Where a collection can happen: only at a safepoint in the run loop

An allocation never collects. Once the collector exists, an allocation will
count what it allocates and, past a threshold, *request* a collection, which
runs at the next safepoint of the run loop. `vm::run` polls one at every switch
to another activation — a push, or a caller resumed — and at every backward
jump, so every call and every loop iteration passes one, a loop that makes no
send included; a forward jump does not poll. Between two instructions `run`
holds the running `ActivationReference`, its `Rc<CompiledCode>` and its
`program_counter`, and no handle the collector would move — each instruction
is an entry of its own and the poll is outside every one, so the compiler
refuses anything else ("The brand" below); every live value is in a register
on the activation stack or in one of the places "What the collector traces"
below lists. The step state's reusable argument buffer is empty there — a
send gathers into it, decides and pushes within one instruction, and it is
cleared when it is given back — so it is not a root, and a debug assertion
before every move says so (`World::check_quiescent_state`).

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
straight to a large-object space and a small reserve absorb that, as they do in
Cog and Primordial Soup.

**The poll** (`vm::poll`, inlined into `vm::run`) is one compare of a flag the
heap owns, `safepoint_requested`. When the flag is set it calls
`Interpreter::safepoint` (`interpreter/safepoint.rs`), `#[cold]` and out of
line, so `vm.rs` holds the compare and the call and nothing of what a
safepoint does. The safepoint serves a request by clearing the flag, except
under the stress mode below, which keeps it set. Nothing sets it in a release
build yet: the allocation counter that will is the collector's.

That the slow path serves the request, although nothing in a release build
makes one, is what keeps the poll in the binary. With an empty release slow
path, fat LTO proved the call free of side effects and deleted it — then the
branch, then the flag's load — so the release binary carried no poll at all,
and the first measurement of it measured nothing. Clearing the flag leaves a
compare-and-branch at both poll sites and a cold call in `vm::run`'s release
assembly. The collector's trigger has to keep the same thing in mind: a
measurement of the poll means something only on a binary whose assembly still
contains it, so the price it shows is the price the collector will pay rather
than one the optimizer hid.

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
contains one (`Object`, `ObjectData`, `ClassData`, `ClosureData`, `Frame`,
`FrameKind`, `Continuation` and the state its variants carry, `Step`,
`Signal`, `NewspeakError`, `Heap` itself) — carries a lifetime `'gc`,
invariant (`heap.rs`'s `Brand<'gc>`, a zero-sized `PhantomData<fn(&'gc ()) ->
&'gc ()>`) so that it can be neither shortened nor extended. A release handle
is still exactly its 32-bit index.

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
run `vm::run`, reach the safepoint or start a host's send. Each instruction
the loop executes is one entry, and the poll is outside every one; what the
loop carries from one instruction to the next names no brand — the running
`ActivationReference`, its code and its `program_counter` — and a run ends
inside its last entry, its answer rooted for the host (`vm::run`, and
[execution.md](execution.md), "The VM"). A host makes one entry per read.
`World::enter` is `#[inline(always)]`, and a lifetime is gone before code is
generated.

**Only `heap.rs` changes a brand.** A host root's slot is shared with a host
outside every entry, so it holds a `Value<'static>`, and `Heap::stash` passes
a value through `at_rest` on the way in, `fetch` through `branded` on the way
out — two private functions, each monomorphic in its direction. The split
does not make rebranding impossible inside `heap.rs`: composing the two,
`branded(at_rest(value))`, carries a value from one brand to another. What it
guarantees is that no single call does, so a misuse there has to spell out
both calls and pass visibly through `'static` at the call site. They are the
only conversions of a brand, and there is **no public
`unbind`** — which is where Nova's discipline leaves a hole that its own
documentation closes by convention ("NEVER unbind a `Value` into a local
variable"). `Heap::new` and `Heap::move_everything` are private to the heap
module, so only `World` builds the heap an interpreter has and only `World`
moves it — never while a `Mutator` is lent out, since `enter` holds `&mut
self`.

**Three `compile_fail` doctests pin it**, on `Interpreter::enter`
(`interpreter/mod.rs`), each beside a passing twin that differs from it in
the one line that matters, so that each fails for the reason it names:
answering a handle out of an entry ("lifetime may not live long enough"),
storing one in a host's own field (E0521), and a host's send from inside an
entry, the shape a native re-entering the loop would have (the interpreter is
already borrowed by the entry).

**No branded code can hold a stale handle.** `same` compares two handles with
no heap in reach, so a debug build can check their move epochs only against
each other, and two stale handles from the same old epoch would pass it where
a live handle against a stale one panics. Within one entry nothing moves, and
no handle survives from one entry to the next, so every handle `same` is
handed was minted or read in the entry that compares it. Only a bare `Heap`
in `heap.rs`'s own unit tests can still make a stale handle, and those tests
are about the debug check itself.

`ActivationReference` stays unbranded: the collector moves no stack entry,
and its generation check already covers an index reused after a pop.

**Why no cast is needed.** The heap stores handles in their at-rest form and
a step sees them branded, so something has to connect the two. Converting
each payload safely was rejected for its cost ("Considered and rejected: a
safe conversion per payload" below), and one audited lifetime cast of the
whole step state was chosen in its place, then found unnecessary ("Considered
and rejected: an audited lifetime cast"). The two systems that cast have
reasons that do not apply here. gc-arena's `Gc<'gc, T>` is a real pointer
whose `'gc` must end with the borrow of the arena, so `mutate` has to bring
its root, a generic type projected through `Rootable<'gc>`, down from its
at-rest lifetime to that shorter one, which the compiler cannot check through
the projection. Nova's `Bindable for Box<T>` rebrands a payload that already
exists from one lifetime to another (`prior-art/nova/nova_vm/src/engine/context.rs`).
Here `'gc` names no real reference, only the `PhantomData` in a handle, and
`Mutator` is a plain type with a lifetime parameter: no step state is ever
brought from one brand to another, because the at-rest `'static` is itself
one of the lifetimes the closure is checked for.

The stress mode below keeps running under the brand: the brand covers what
Rust holds, the stress mode what the tracer knows. Its price is ergonomic —
every host and test reads through `enter` — and that price is accepted. What
the whole step measured is in [execution.md](execution.md), "What 2c moved":
level with the commit it started from on every source, every group
overlapping in both sittings. A first measurement had read 1–2.5% slower on
the instantiation and block benchmarks; the brand commit's hot machine code
turned out identical to its parent's, moved into another 64-byte phase by
smaller cold code ahead of it, which is why a measurement now builds every
binary it compares with function starts pinned to 64 bytes — the shipped
build stays unpinned ([execution.md](execution.md), "What removing the
re-entries moved", the protocol).

### Host roots

A host keeps values across runs: `ns-next bench` sends to the same instance 22
times, and `snapshot` encodes after its send. A run may move every object, so
what a host holds across one must be where a move rewrites it: in the heap's
root table, which the tracer walks. A `Root` (`heap.rs`) is an `Rc`-shared
`Cell<Value>` slot the table also holds — gc-arena's `DynamicRootSet` model —
so releasing one needs no access to the heap: once the last clone outside the
table is dropped, the table holds the only reference, and its next sweep drops
the slot. The table is swept at every move, and by `stash` whenever it has
doubled since its last sweep (never below 64 slots), so it stays proportional
to the roots a host still holds in a release build too, where nothing moves.

Inside an entry, `Mutator::stash` roots any value and `Mutator::fetch` reads
one back (`Heap::stash`/`fetch`). Outside every entry, `Interpreter::stash`
roots an immediate a host built itself — `nil`, a Boolean, a `SmallInteger`,
a `Float` — and takes a `Value<'static>`: a host can build an immediate but
never hold an object handle outside an entry, so a `Value<'static>` holds
none. All of them take `&self`, the table sitting behind a `RefCell`, so that
a host can stash an argument inside the call that sends it —
`interpreter.send(&receiver, selector, &[interpreter.stash(value)])` — as
`DynamicRootSet::stash` allows. A value `fetch` answers carries the brand of
the entry it was read in, so it is good until that entry ends, and a host
reads the root again in the next one.

Every host entry takes and answers roots: `Interpreter::send`, `instantiate`,
`load_module` and `platform`, and `image::load`, whose `LoadedImage` carries
the root class and the image's roots as `Root`s. A send or an instantiation
answers a `HostResult`, `Result<Root, HostSignal>`: its value rooted
(`Mutator::to_host`), a Newspeak error as a `HostError` whose receiver is a
`Root`, or a `^` that reached the host as `HostSignal::Return`. `Signal` and
`EvaluationResult` are crate-private, so no plain value leaves a run by any other
way than `fetch`; and since a `Root` can hold any value, `instantiate` refuses
one that holds no class object with a Newspeak error. The codec runs inside
an entry: `image::snapshot` takes a `&Mutator<'gc>` and branded values,
`image::build` a `&Heap<'gc>`, which a host fetches from its roots in the
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

`trace.rs` enumerates it, type by type: a `Trace` impl hands every field that
holds an `ObjectReference`, a `ClassReference` or a `FrameReference` to a `Tracer`, one `&mut` at a
time.

- The bootstrap: `builtin_classes` and `platform_singleton`, which the step
  state holds beside the heap and `World::move_everything` forwards after it.
- The activation stack: each entry's receiver, `current_class`, the
  `ClassReference` or closure object its `FrameKind` names, the frame it is married
  to, and the state its continuation carries — a `WhileLoop`'s condition and
  body, a `DoLoop`'s receiver, block and elements, a `NestedClassCreation`'s
  enclosing object and declaring class, an `Instantiation`'s instance, levels
  with their arguments, and phase; and every register. `StackEntry`'s impl is
  in `heap.rs`, where its fields are private.
- Every arena record: an object's class and payload — an instance's or an
  array's fields, a closure's captured frame and its copied values, a class's enclosing
  object, declaring class, superclass and metaclass — and a frame's receiver,
  registers, sender, home, current class and kind. A married frame's registers
  are its entry's, traced with the stack.
- The host root table.
- The heap's canonical symbol table ([object-model.md](object-model.md),
  "Values and objects"), a strong root until the collector makes it weak, as
  Primordial Soup's `symbolTable` is -- a symbol no program holds any more is
  then free to go, and the next literal of its text allocates a new
  canonical one.

**Every impl names every field and matches every variant**, with no `..` and
no `_` arm, so a field or a variant added later does not compile until someone
decides whether it holds a handle. That is what keeps the enumeration complete
as the types grow; the stress mode checks that the places it is applied to are
complete. Enumerating is `trace.rs`'s and rewriting is `heap.rs`'s
(`Forwarder`), since only `heap.rs` can mint a handle. `trace.rs` exists in a
debug build only, until the collector needs it in every build.

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

**A handle lives only where the tracer can rewrite it in place**: in an arena
record, in a stack entry, in a continuation that one stack entry owns, or in
the step state's own roots — never behind a shared `Rc`, which the tracer could reach
only through a shared reference. So a closure's handles live in the
closure object: `ClosureData` is stored inline in the object's payload,
sharing only `code`, which holds no handle, through its own
`Rc` with the `MakeClosure` instruction that made it; and everything else
that needs a closure — `FrameKind::Closure` on each of its activations,
`WhileLoop.condition`, `DoLoop.block`, `Step::Block` — names the closure object,
whose `captured_frame` and copied values the tracer then rewrites once, in its
record.
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
are not handles either, and their `Trace` impls say so: a stack entry's and a
closure's `feedback_base` (`ClosureData.feedback_base`), a class object's
`ClassData.dispatch_id`, and the dispatch id every object carries in its
header (`Object.dispatch_id`), which is what a guard reads
([dispatch.md](dispatch.md), "The dispatch id"). The ids a cache guards on
are per heap and never serialized. The header's id is a number, not a
handle: the tracer does not visit it, and a move carries it along with the
rest of the object. Its four bytes are paid for by boxing a class object's
data (`ObjectData::Class(Box<ClassData>)`), so an `Object` is still 56 bytes
in a release build and the amplification figures below did not move there.

### Checking the roots before a collector exists: the stress mode

The brand checks what Rust locals hold; it cannot check that the list above
is complete, and nothing checks that at compile time. A debug-build
stress mode checks it by running. With `NS_NEXT_STRESS_MOVES` set to anything
but empty or `0`, every interpreter the process bootstraps — the `ns-next`
binary the CLI tests spawn included — keeps the safepoint flag set and moves
everything at the safepoints its schedule picks, so the whole existing suite,
which pins every program's outcome, runs under it unchanged.

A move (`World::move_everything`: `Heap::move_everything`, then the step
state's own roots) rotates each arena by a shift in `1..len` that varies
with the epoch, so no slot keeps its index, and rewrites every traced handle: a
compacting collection that treats everything as live. Every `ObjectReference` and
`FrameReference` carries, in a debug build and beside the origin it already carries,
the heap's move epoch when it was minted or last forwarded, and every accessor
compares the two, so a handle the tracer missed panics at its first read
("stale handle") instead of perhaps printing a wrong answer. The rotation
checks the forwarding; the epoch checks the completeness. The `Forwarder`
checks each handle it rewrites as well — its origin, and that its epoch is the
one being moved from — because a foreign or stale handle stored in the heap is
a mistake the move would otherwise make permanent. `ActivationReference` carries no
epoch: no stack entry moves, and its generation already covers an index reused
after a pop. V8's `--stress-compaction`, SpiderMonkey's compacting GC zeal mode
and HotSpot's `ScavengeALot` exist for the same reason. Here the stress mode
also comes first: the tracing and forwarding the collector will need are built,
and held to the whole suite, before the collector is.

**The schedule is budgeted, not every safepoint.** A move costs work
proportional to what it traces — both arenas, the activation stack's entries
and registers, and the host roots (`Heap::traced_size`) — so moving at every
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
`run-corpus` — every runnable module from source and as a built binary, under
both lowerings — under the variable before every commit. A release build
contains none of it: `trace.rs`, the epoch, the move, the schedule and the
test hooks (`Interpreter::move_now`, `moves`, `stress_safepoints`,
`set_stress_moves`) are `#[cfg(debug_assertions)]`, and only the poll is
left, whose price is measured in "Where a collection can happen" above.

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

With the arena in place and roots checked, the collector's own root set is the
list in "What the collector traces" above. Most of it is two things: the
bootstrap, standing in for upstream's object store, and the activation stack
([execution.md](execution.md), "Activations") — a contiguous, precisely typed
pair of arrays rather than a pointer-chased chain of individually allocated
frames. The safepoint exists in every build, and the tracing (`trace.rs`) and
the forwarding (`Forwarder`, `Heap::move_everything`) exist in a debug build,
checked by the stress mode. The collector un-gates the tracing and the
forwarding, and adds liveness, reclamation, the allocation counter that
requests a collection, and the spaces — a nursery, a large-object space.

Closure conversion ([execution.md](execution.md), "Closure conversion") was
built directly before the collector for the collector's sake. Before it, a
block that outlived its method kept that method's whole reified frame alive —
every register, including ones the block never reads — so the collector would
have retained more than the program can reach through its variables. A
surviving block now holds what it captured: its copied values, and the
indirection vector of any scope whose written variables it needs, plus its
creator's frame record without registers.

### The interim cost, and the one question still open

- **Neither arena reclaims a slot today**, and that is the interim cost the
  milestone's ordering accepts. It is measurable. Peak working set on a release
  build, before the arena, after it, and after the activation stack; the last two
  columns were measured against each other in one sitting on 2026-09-10, and the
  arena figures reproduced exactly the ones taken when the arena landed:

  | Command | Entry sends | Before the arena | After the arena | After the activation stack |
  |---|---|---|---|---|
  | `run`, `bench_loop.ns` | 1 | 4.6 MB | 4.6 MB | 4.6 MB |
  | `bench --iterations 10`, `bench_loop.ns` | 22 | 4.6 MB | 340.7 MB | 4.6 MB |
  | `bench --iterations 10`, `bench_send.ns` | 22 | — | 324.9 MB | 4.6 MB |
  | `bench --iterations 10`, `bench_block.ns` | 22 | — | 966.9 MB | 446.0 MB |

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

  **What is still unbounded is everything in an arena: every reified frame and
  every object the process has ever created**, not what is live.
  `bench_block.ns` shows it: every iteration creates a block, so every
  iteration reifies one frame and allocates one closure, and nothing gives
  either back. Its peak roughly halved, because the block's own
  activation no longer takes an arena entry, but it still grows with the work
  done. A long-running process that creates blocks is therefore unbounded until
  the collector exists. Closure conversion shrank each reified frame's share —
  a frame of a body nothing reaches by reference is now written back with no
  registers at all — and **measured 2.6% worse on this source, not better**:
  454.7 MB to 466.4 MB, because a block that writes a captured local now
  allocates an indirection-vector `Array` per activation of the body that
  declares it, into the same arena that gives nothing back
  ([execution.md](execution.md), "What closure conversion moved", where that
  explanation is marked as an inference from what changed rather than a
  measured attribution).

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
  `bench_block.ns`, which still grows both arenas every iteration, still has a
  maximum well above its median: 63.8–67.9 ms over medians of 37.6–39.9 ms in
  all six `bench` runs of the 2026-09-11 before-and-after measurement of the
  retirement. `bench` reports only the median, minimum and maximum, so how many
  runs were slow is not measured. Pre-sizing the arenas with `with_capacity` is
  the control that would settle the doubling explanation, and it has not been
  run. This is recorded because a collector's design should know the interim
  may have a latency cost as well as a memory one.

  A corollary the collector's design should know about, stated carefully because
  the careless version is wrong. A **failed** image decode holds its placeholders
  until the load returns. `decode_graph_section` allocates `object_count` objects
  and `frame_count` reified frames up front, because a record must be addressable
  before the records that reference it are read, and only then validates each
  record; nothing reclaims a slot, so a rejected image cannot give them back
  part-way through.

  It is not retained past that. `image::load` owns its `Interpreter` in a local,
  so every `Err` path drops it and both arenas with it — the cost is a transient
  peak during loading, not a leak for the process's life.

  What is worth recording is the *amplification*, and it compounds in a way the
  obvious reading misses. `decode_graph_section` bounds each count with
  `bounded_count`, but both calls divide the **same** `r.remaining()` — only the
  two count words separate them — so one file can max out both at once rather
  than trading one against the other. Against the bytes remaining at the graph
  section, not against the whole file:

  | | `Object` | Frame-arena entry | Combined |
  |---|---|---|---|
  | Record minimum | 6 B | 18 B | — |
  | Release size | 56 B → 9.3x | 136 B → 7.6x | **~16.9x** |
  | Debug size | 80 B → 13.3x | 160 B → 8.9x | ~22.2x |

  The release `Object` size is pinned by a release-build test
  (`an_object_is_the_size_the_amplification_figure_assumes`, `heap.rs`),
  so a field that grows it fails there before this figure goes stale. It is
  56 bytes rather than 48 because `ClosureData` — an `Rc` to its code, a
  `FrameReference`, the boxed slice of its copied values and a
  `FeedbackBase`, 32 bytes — is `ObjectData`'s
  largest payload now that a class object's data is boxed; a placeholder
  record is an empty instance and allocates no box. Closure conversion kept
  that at 32: `copied` replaced the two fields it made redundant, the
  parameter names and the home.

  A frame-arena entry is a `FrameSlot`: the `Frame` (128 B in a release build,
  152 B in a debug one) and the `live` marriage field beside it, 8 bytes more.
  The debug sizes are larger because every handle there carries, besides its
  index, the id of the heap that minted it and the move epoch the stress mode
  checks it against.

  `bounded_count` is what keeps this proportional to the input at all rather
  than to a 32-bit count — without it a crafted `u32::MAX` would allocate
  billions of placeholders. The residual factor is the price of allocating
  before validating, which the format's back-references require.

  This is also *better* than the model it replaced, which is the part that makes
  it an accepted cost rather than a regression. Under `Rc<RefCell<_>>` each
  placeholder was its own heap allocation with the same lifetime, plus a
  refcount and an allocator round trip per record; the arena makes the same
  placeholders two contiguous `Vec` pushes. The collector's interest here is the
  peak, not a lifetime bug.
- **The application cache grows without bound** in the presence of live editing,
  because every re-application mints a fresh entry that nothing evicts. That is
  the same knot as `Mixin`'s lack of interior mutability; see
  [liveness-and-reflection.md](liveness-and-reflection.md). Evicting weakly is
  not available until a mixin carries an id of its own rather than an address —
  see "Weak references" below for why the address is what holds the cache
  strong.

### The shape: mark, compact, forward

Decided with the user on 2026-09-20; designed, not yet built. The collector
marks from the roots, slides the live slots to the front of each arena, and
rewrites every handle through a forwarding table. One space, stop-the-world.
Generations come later, as their own measured step.

Three things the representation settles before any taste enters:

- **MMTk is out.** It is the one Rust framework whose collectors move, but it
  works on raw addresses, mapped spaces and headers reached by pointer
  arithmetic, and `crates/ns-runtime/src/lib.rs` is `#![forbid(unsafe_code)]`.
  Adopting it would replace not only the arena of 32-bit indices but the origin
  guards, the epoch check and the proof behind the brand. That is a different
  runtime, not a choice of collector.
- **Reference counting is out**, because cycles need tracing and `become:`
  needs forwarding.
- **Conservative scanning is out**, decided earlier and on its own grounds
  ("Considered and rejected: conservative stack scanning").

What is left is the choice between a non-moving free list over the slot table,
a semispace copier, and mark-compact — and the two arguments that usually
decide it do not apply here. Every slot is the same size, so a free list has no
fragmentation to solve; and an object's payload (`Vec<Value>`, `String`,
`Box<ClassData>`) is a separate allocation that no collector here moves, so a
copier buys far less locality than it would where payloads sit inline.

Mark-compact was chosen for four reasons:

- **It reaches the goal of the step.** The table shrinks and the pages go back
  to the operating system. A free list holds the table at its historic maximum
  for the life of the process, which is the figure step 5 exists to fix.
- **Rewriting is the cheap part here rather than the expensive one.** Over an
  index arena the forwarding table is a flat `Vec<u32>` over the slot count:
  O(1) per handle, 4 bytes per slot while a collection runs — 32 MB against the
  466 MB `bench_block.ns` peaks at today. In an address-based collector this is
  what costs. Nova, the one engine with the same representation, pays more than
  it needs to: its `CompactionList` stores shift runs and binary-searches per
  handle.
- **A free list's sweep scales with the historic maximum**, while every phase of
  a mark-compact scales with the live set. Where there is much garbage and
  little live data — `bench_block.ns` — mark-compact is the cheaper of the two,
  not the dearer.
- **It uses the machinery that exists and is under test.** `trace.rs`,
  `Forwarder`, the epoch check and the move stress mode were built for exactly
  this shape; under a free list they would stay a debug-only appendage with no
  production role.

The prior art points the same way. Nova (`prior-art/nova/nova_vm/src/heap/`),
the only reference implementation with this representation, is mark-and-sweep
with compaction over per-type vectors and is not generational. Strongtalk's
full collection is a mark-compact despite the name of `markSweep.cpp`
(`prepare_for_compaction`, `compact`), under a scavenged young generation.
Spur compacts its old space. Every current HotSpot collector moves, and the one
free-list old space it had, CMS's, was deprecated in JDK 9 and removed in
JDK 14. Primordial Soup is the one that keeps a free list, in its old space
(`vm/heap.h`, seven size classes) — and it has the fragmentation this design
does not, because its objects are variably sized in address space.

### Marking

Mark bits live in a side table, one `Vec<u64>` per arena, one bit per slot —
1/448 of the object arena — and exist only while a collection runs. Not in the
header: `Object` is pinned at 56 bytes by
`an_object_is_the_size_the_amplification_figure_assumes` (`heap.rs`), and the
image format's amplification figures are computed from that number. Nova's
`heap_bits.rs` keeps its bits beside the data for the same reason.

Marking is a second `Tracer` (`trace.rs`) beside `Forwarder`: it sets the bit
and, where the bit was clear, pushes the index onto an explicit work list. The
traversal is iterative because the graph's depth is the program's to choose —
the reasoning that already made `MixinApplication`'s `Drop` iterative. Sharing
one edge enumeration between marking and forwarding is what keeps the two from
drifting: a field a later change adds is named in exactly one place, and the
compiler refuses that place until it is named.

The roots are the list `Heap::move_everything` and `World::move_everything`
already walk — the activation stack's entries and registers, the builtin
classes, the `platform` singleton, the host roots — with the canonical symbol
table removed from it, since that becomes weak. The difference from a move is
the direction: a move walks every arena record because everything counts as
live, while the collector starts at the roots and reaches a record only along
an edge.

### Compaction and forwarding

Three passes per arena. One over the bits with a running destination index,
which *is* the forwarding table, a `Vec<u32>` over the slot count. One that
slides the live slots forward and truncates the arena to the number of live
ones — the dead `Object`s drop there, and their payloads with them. One with
the existing `Forwarder` over everything that holds a handle, looking up the
table instead of applying a `Rotation`.

A collection does both arenas and one forwarding pass, because they are one
graph: a closure holds a `FrameReference`, a frame holds objects.

The epoch is unchanged, and it is what makes a stale handle loud rather than
silent. A reclaimed slot does not linger to be read the way a free list's
would; it is gone, and the index that takes its place carries a new epoch.
Poisoning a freed slot, which Primordial Soup does with `kUnallocatedByte`, is
not needed here.

The move stress mode stays beside the collector rather than being folded into
it. It moves everything without liveness, so it isolates the completeness of
the root enumeration from the correctness of marking: when the collector's own
gate fails, a green move stress says the fault is in marking. `Forwarder`
therefore takes its mapping as a parameter — a rotation for the stress mode, a
forwarding table for the collector.

### The trigger and the growth policy

A collection happens only at a safepoint of the run loop, as decided on
2026-09-13. An allocation sets the request at most; it never collects.

The test is a comparison, not a counter. `push_object` already holds the
arena's length, so it compares that against a limit and sets the safepoint
request when it is reached; `reify` does the same for the frame arena. One
predictable branch, and no new state on the hot path.

After a collection each arena's limit becomes `max(floor, live * 2)`, so that
between two collections no more is allocated than is live and the marking work
per allocated slot is amortized constant. The floor — 4096 slots — keeps a
small program out of the collector altogether: `bench_loop.ns` and
`bench_send.ns` run in 4.6 MB today and should never collect. Soup's
`GrowthPolicy` and HotSpot's free-ratio control have the same shape.

The image decoder never collects, because it never polls — it does not run in
the run loop. The transient peak described under the decode amplification above
therefore stands unchanged: a rejected image's placeholders are reachable until
validation finishes, so they are live, and the amplification figures stay
valid.

In a release build this gives the safepoint's slow path its first real work.

### Weak references

**The canonical symbol table becomes weak.** It is a strong root today
(`canonical_symbols`), and left that way every symbol object a program ever
makes would outlive the process, since a selector literal makes one. It is not
a root for marking; after marking and before compaction, an entry whose object
is unmarked becomes `None`, and the rest are forwarded like everything else.
Nova has exactly this pair: `shift_strong_u32_index` answers an index,
`shift_weak_u32_index` answers an `Option`.

This does not weaken §5.1.6. Equal symbols are one object as long as
`Mutator::new_symbol` is the only way to make one. If a symbol object is
collected then nothing held it — anything that did would have been a root and
kept it live — so the next `new_symbol` mints a fresh one whose identity is
again unique, and no live comparison can tell the difference. An identity hash
belongs to the object rather than to the symbol; a program that keeps the
number `identityHash` answered instead of the object has no guarantee today
either, and Squeak's weak symbol table behaves the same way.

**The application cache stays strong, and the reason is its key rather than the
collector.** `mixin_key` and `application_key` are `Rc::as_ptr(...) as usize`
(`mixin.rs`), and the strong `Rc` the cache holds is what keeps that address
valid and unique. A weak cache would let a freed `Rc<Mixin>` hand its address
to a newly allocated one, and the new mixin would hit the old entry and be
given a **different, perfectly valid** application, reporting nothing — the
failure class the handle invariants exist to prevent. Weakness becomes
decidable only once a mixin carries an id of its own instead of an address.
That is a change in its own right, and it belongs with the live-editing work
([liveness-and-reflection.md](liveness-and-reflection.md)), which is also where
the cache's unbounded growth belongs. The same per-mixin id is wanted
independently by dispatch's open option of guarding a private lexical site on
N's mixin ([dispatch.md](dispatch.md), "Decisions").

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

The frame arena is collected exactly as the object arena is, with its own bits
and its own forwarding table.

**A married frame cannot be collected, and that needs no rule of its own.**
`StackEntry::trace` follows `reified`, so a frame whose activation is still
running is reachable from the stack and is marked. Leaving that to the root set
rather than to a special case is what keeps a later change from silently
freeing a running activation's frame.

The marriage survives compaction in both directions: `FrameSlot::live` holds a
*stack* index and no stack entry moves, and `StackEntry::reified` is forwarded
like any other handle. The divorce on pop, and the conditional write-back of
registers, are unchanged.

What keeps a frame alive past its return is a closure's `captured_frame`, or
another reified frame's `sender`/`home`. Closure conversion shrank the record —
a body nothing reaches by reference is written back with no registers — but did
not remove it: `ClosureData` still names its captured frame, because the home
of a non-local return hangs on it. A surviving closure therefore retains its
frame and, through that frame's `sender`/`home`, possibly a chain. That is the
semantics rather than a defect, and it is what `bench_block.ns` measures.

### The gates the collector needs

The existing gates cover the completeness of the roots and the rewriting,
because the move stress treats everything as live. The collector's own failures
are different: **premature reclamation** — a missed root, a reachable object
gone, silent wrong output at exit status 0 — and **false retention**, which no
test sees at all and only a measurement shows.

`NS_NEXT_STRESS_COLLECT=1` is the third stress mode, beside the move and the
cache one, and a gate of its own. It collects at every safepoint rather than at
a limit, paced by heap size like `STRESS_MOVE_BUDGET`, since collecting at
literally every safepoint is quadratic in a deep recursion. Under it every
golden and every corpus module runs at the highest collection frequency there
is, so a missed root kills its object at the earliest possible moment and the
recorded outputs catch the difference.

False retention is caught by counting rather than by observing: a
`debug_assertions`-only `collect_now`, beside `move_now`, and a
`heap_statistics` answering live and occupied slots per arena. A program that
discards n closures is back at its starting size afterwards; one that keeps n
closures keeps n frames.

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


## Decisions

- **Identity hashes come from a counter, not an address, and are assigned on
  first request**, so identity survives moving and snapshotting — except, for
  now, the hash of an object an image names by tag rather than by record
  ([image-format.md](image-format.md), "Identity hashes are preserved
  verbatim").
- **A handle is an arena-relative index, not a globally unique name**, and a
  `debug_assertions`-only origin guard says so at every accessor rather than
  leaving it to convention. It exists because the failure it catches is silent:
  a handle from another heap is in range and names a valid object, so the read
  succeeds with the wrong answer. It costs nothing in a release build, where the
  origin field does not exist. An `ActivationReference` adds a generation to the same
  guard, for the same reason: a stack index is reused once its entry is popped.
- **Activations live on a stack, not in the arena**, so that what nothing
  captures is not retained. Only a reified frame reaches the arena, and it is
  the one activation form anything may store.
- **A moving collector, not a conservative one**, because `become:` and instance
  migration are requirements. Conservative scanning with pinning was considered
  and rejected on the same grounds.
- **Iterative `Drop` rather than a depth ceiling** for `MixinApplication`'s
  chain, because that depth is bounded by nothing the front end enforces, so a
  ceiling would exclude valid programs. Where a depth *is* independently
  bounded — the `Mixin` declaration tree, capped by the parser — a ceiling is
  the right answer and `image/mixins.rs` uses one. Objects and activations need
  neither: an arena entry holds indices, so there is nothing to unwind.
- **A collection happens only at a safepoint in the run loop**, never inside an
  allocation (decided 2026-09-13), so no allocating Rust path roots anything
  and a JIT keeps its freedom across allocations. Cog, Nova, CPython and
  piccolo put it there; V8 and HotSpot's runtime pay for the alternative with
  handles and verifiers.
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
  stress mode, before a collector exists, because the brand cannot see a handle
  stored somewhere the tracer does not know about.
- **The stress mode moves on a budget paced by heap size**, not at literally
  every safepoint (decided with the user on 2026-09-13): at every safepoint
  while the traced size is within `STRESS_MOVE_BUDGET`, and above it once per
  size-over-budget safepoints. A move at every safepoint is quadratic in a deep
  recursion, and a fixed interval, as HotSpot, SpiderMonkey and V8 use, would
  take coverage from small programs to spare large ones.
- **A host keeps values across runs only as `Root`s**, slots it shares with the
  heap's root table (gc-arena's `DynamicRootSet`), so the tracer finds and
  rewrites every value a host holds and releasing one needs no heap.
- **A closure's payload is inline in the closure object**, not behind an `Rc`
  of its own, and everything else that needs a closure names the object: a
  handle behind a shared `Rc` cannot be rewritten in place. Only `code`, which
  holds no handle, stays shared. It also made `bench_block.ns`
  about 4% faster, since `MakeClosure` no longer allocates an `Rc` and a popped
  block activation drops none.
- **Mark-compact over the index arena, one space, stop-the-world** (decided with
  the user on 2026-09-20), with generations as a later, separately measured
  step. Over uniform slots a free list has no fragmentation to solve and a
  copier little locality to win, so the choice rested on what the step is for —
  the table shrinks and the pages go back — on rewriting being O(1) per handle
  through a flat forwarding table over an index arena, and on the machinery
  already built and under test. Nova, the one engine with this representation,
  compacts; Strongtalk, Spur and HotSpot compact or evacuate their old
  generation, and HotSpot removed the one free-list old space it had.
- **MMTk is ruled out by `#![forbid(unsafe_code)]`**, not by a comparison of
  collectors. It works on raw addresses, so adopting it would replace the arena,
  the origin guards, the epoch check and the brand's proof along with the
  collector.
- **The canonical symbol table is the system's only weak structure.** §5.1.6
  survives it because `Mutator::new_symbol` is the only way to make a symbol
  object, so one that is collected was held by nothing.
- **The application cache stays strong because its key is an address.**
  `Rc::as_ptr` as a key needs the strong reference to keep it unique; weakly, a
  recycled address would answer an old entry with a different, perfectly valid
  application. It becomes decidable once a mixin carries an id, which the
  live-editing work wants anyway.
- **A collection is requested by comparing an arena's length against a limit**,
  reset to `max(4096, live * 2)` after each collection, rather than by counting
  allocations: the length is already in hand where an allocation happens, so the
  hot path pays one predictable branch and carries no new state.
- **Collecting at every safepoint is a third stress mode and a third gate**,
  `NS_NEXT_STRESS_COLLECT`, kept beside the move stress rather than replacing
  it, because moving everything without liveness is what separates a missing
  root from a mistake in marking.
