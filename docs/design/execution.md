# Execution — activations, the VM

> **Status:** built, including the trampoline; nothing re-enters the run loop
> from Rust, the loop polls a safepoint, it runs eight-byte instructions over
> the running frame's state in locals, each stretch between two safepoints one
> entry of the step state, the sends it executes go through inline caches,
> except the reference lowering's, and the optimized lowering converts
> closures, so only by-reference code keeps a returning frame's registers; one
> unwinder carries every non-local return, exception unwind and termination,
> running `ensure:` blocks as trampoline steps; a running activation can be
> suspended into its frame and resumed from it, lazily, and a stress mode
> runs every program on from suspended frames; every running activation is
> a frame of tagged words in one reservation of the heap's, measured no
> slower than the stack of entries before it (road step 7a, "What frames
> as words moved"), and a send's own callee lies on its argument slots,
> which are the callee's first registers (road step 7a′, "Overlapping
> register windows"). The contract compiled frames will keep with
> interpreted ones is written (road step 7c, "Planned: the
> deoptimization contract"), and the baseline JIT's foundation is built
> on x64: a body that can loop before its first instruction without a
> template runs as machine code from its second entry up to that
> instruction, then the interpreter goes on (road step 8's first two
> stages, "The baseline JIT's foundation").
> **Code:** `crates/ns-runtime/src/heap.rs` (the activation stack, the frame
> objects, the `ActivationReference` and `FrameReference` handles, every read and write of an
> activation, the copied values a closure carries and whether a frame keeps its
> registers, and the side table of suspended activations with
> `suspend_activations` and `resume_activation`), `heap/frames.rs` (the
> reservation the stack's frames lie in, the header's words and the flags
> word), `heap/object.rs` (the
> fixed fields of a frame object and a closure, `frame_field` and
> `closure_field`), `heap/world.rs` (the step state
> at rest, and its entry),
> `types.rs` (`ActivationKind`, `FrameKind`, `Continuation` and the state
> its variants carry), `bytecode/compact.rs` (the eight-byte encoding the
> loop runs),
> `vm.rs` (the run loop and its safepoint poll), `interpreter/` (the
> inline caches, the send decisions, `Step`, every push, the depth limit, the host entry, the
> safepoint's slow path, the indirection vector's reads and writes in
> `interpreter/locals.rs`, and the unwinder and the depth limit's reserve in
> `interpreter/unwind.rs`, and forcing a future in `interpreter/future.rs`),
> `compiler/capture.rs` (the capture analysis) and
> `compiler/lower.rs` (the converted register layout),
> `builtins/block.rs` (`value`'s leaf, which answers a block's activation as
> a step), `jit/` (the baseline JIT: the trigger and the entry in
> `jit/mod.rs`, the compiler in `jit/baseline.rs`, the idioms in
> `jit/macro_assembler.rs`, the stubs in `jit/stubs.rs`, the generic leaf
> in `jit/leaves.rs`), `crates/ns-assembler` (the operations and the x64
> encoder), `crates/ns-memory/src/code_zone.rs` (the code zone and the
> entry into machine code), `crates/ns-cli/src/main.rs` (the interpreter thread and
> its stack), `crates/ns-cli/src/commands/bench.rs` and
> `benchmarks/are-we-fast-yet/` (what "Measurements" times).

This is the consumer side. What produces the bytecode — the instruction set,
lowering, register allocation, inlining — is [compiler.md](compiler.md).

## One engine

The bytecode VM is the only engine. A module's code is compiled when the module
is loaded, or decoded from an artifact, and every Newspeak-level body — method,
block, factory, initializer, superclass clause — runs as compiled code in the run loop,
`vm::run`.

The Milestone 1 tree-walking interpreter, which ran beside the VM as a
differential oracle, was retired on 2026-09-11. A recursive AST evaluator keeps
its computation on the Rust stack, so it could take part in none of what the
trampoline ("The trampoline" below) exists for — suspending and
serializing a running computation, debugger stepping, edit-and-continue on a
live activation — and it constrained the design wherever it still had to agree.
The oracle was also blind to defects in the orchestration the two shared: four
of the six silent defects Milestone 3's review found were identically wrong
under both.

Behaviour is protected by recorded answers, which both lowerings must produce:
every `goldens/**/*.ns` against the exit status, stdout and stderr
recorded in the `.toml` beside it (`crates/ns-cli/tests/run_goldens.rs`), and
`run-corpus` over the runnable corpus. The answers recorded before the
retirement are verdicts the oracle and the VM agreed on. A change that alters
one is a behaviour change, to be justified as one — never recorded because the
code now produces it. The reference lowering ([compiler.md](compiler.md), "The
reference lowering") is the differential oracle that replaced it.

## Activations

A Newspeak-level activation is never represented only by Rust locals or the Rust
call stack. A running activation is a frame of words on one **activation
stack** that the `Heap` owns. A first-class frame -- an object of the frame format in the
`Heap`'s object space, an instance of the kernel-private `Activation`
([object-model.md](object-model.md), "The object layout") -- exists for it only
once something must hold it past the call that runs it: a block being
created, the exception machinery asking for its `Activation` object, an
unwind naming it as its target, the depth limit's reserve, or a suspension,
which moves a running activation off the stack into its frame ("Suspending
and resuming" below).

### The model, and the one place ns-next differs from it

The model is Cog's, and Primordial Soup implements it for Newspeak
(`prior-art/primordialsoup/vm/interpreter.cc`). Frames live on a stack and carry
an empty activation slot. `EnsureActivation` allocates the heap `Activation`
only when something asks for one — `PushClosure`, `thisContext`, an error — and
*marries* it to its frame, so that reading the activation reads the live frame.
`HasLivingFrame` detects an activation whose frame has since returned. Cog does
the same: `marryFrameSP` on closure creation, `isWidowedContext` for the dead
case (`prior-art/opensmalltalk-vm/src/spur64.stack/interp.c`).

One difference decided the shape of this design, and closure conversion has
since closed most of it. Cog's and Primordial Soup's closures **copy** the
values they capture and box the variables they write
(`pushClosureNumArgs:copiedValues:` and `pushRemoteTemp` in Cog; `PushClosure`
and `PushIndirectLocal` in Primordial Soup), so nothing ever reads a dead
frame's temporaries and a widowed context needs none of them. The optimized
lowering now does the same ("Closure conversion" below): a captured variable is
copied into the closure, or lives in its scope's indirection vector, which the
declaring body and every block that needs it share. A counter block returned
from the method that declares `n` still goes on writing `n`
(`goldens/language/block/closure_variables.ns` pins this) — through the vector rather
than through the frame.

Two things still read an enclosing activation's registers **by reference**,
through `captured_frame`: the reference lowering, which keeps the by-reference
model as the oracle the conversion is checked against, and the fallback closure
an inlined `ifTrue:` sends when its receiver is not a Boolean ("Fallback
blocks" below). So a reified frame keeps its registers when its activation
returns only when something can still reach them that way, decided per
activation from the block's own `outer_reach` ("Pop" below).

### Representation

```rust
// heap.rs -- every field private to the module
struct ActivationStack {
    frames: FrameStack,        // heap/frames.rs: one reservation of words, every
                               // live activation a frame, innermost highest
    innermost: u32,            // the innermost frame's offset, or none
    live: usize,               // how many frames: `activation_depth`
    machine_words: MachineWords, // its `DepthCount` word: how many
                               // activations, live or suspended, count
                               // against the depth limit
}

a frame on the stack           // every word tagged
    the arguments              // registers 0 .. argument_count, below the header;
                               // a send's own callee's are its sender's slots
the header                     // heap/frames.rs's frame_header, from the frame's offset
    0 receiver                 // a value
    1 current class            // the class object; nil at top level
    2 operand                  // the closure of a block frame, the class of a
                               // class-carrying kind, the selector of a method
                               // frame as a SmallInteger; nil otherwise
    3 reified                  // the married frame object, or nil
    4 flags                    // SmallInteger: the kind's tag, argument_count,
                               // counted, retains_registers; in a debug
                               // build the generation too
    5 code  6 program counter  7 feedback base          // SmallIntegers
    8 caller                   // the frame below's offset; nil in the bottom one
    9 continuation             // nil for Deliver, else its row in the heap's
                               // side table of continuations
    10 guard  11 entered level // recorded_guard and entered_level, SmallIntegers
    12 register count          // the registers above the header
    then the registers         // copied values, temporaries, vector, scratch:
                               // registers argument_count + 13 on

a frame object                 // heap/object.rs's frame_field; an object of the frame format
    0 sender  1 home  2 receiver  3 current class   // objects or nil
    4 program counter  5 code id  6 kind (its wire tag)   // SmallIntegers; code id nil at top level
    7 the kind's operand       // the closure of a block frame, the class of a class-carrying kind
    8 uncontinuable            // true once its activation has returned
    9 married                  // SmallInteger: >= 0 the stack frame's offset,
                               // < 0 -(row + 1) of a suspended frame's
                               // side-table row; nil once divorced
    10.. the registers         // the arguments, then the registers above the
                               // header: register r is field 10 + r below
                               // argument_count, 10 + r - 13 above the header;
                               // the header's thirteen numbers have none

pub struct ActivationReference {
    offset: u32,               // the word offset of the frame's header
    #[cfg(debug_assertions)] origin: u32,
    #[cfg(debug_assertions)] generation: u32,
}
```

- **The stack belongs to the `Heap`**, because `heap.rs` is already the one place
  that stores activations, and because it is half of the collector's root set
  ([memory.md](memory.md)).
- **One reservation of words holds every running activation**, header and
  registers together: a `Reservation` of `ns-memory`, the heap's second
  beside its object space ([memory.md](memory.md), "The memory layer"),
  16 GiB on a 64-bit target and 256 MiB otherwise (`STACK_RESERVATION`,
  `heap/frames.rs`). Where the operating system limits address space, it
  falls back as the object space's reservation does, halving the request
  down to no less than 1 GiB (`reserve_falling_back`, `heap/space.rs`).
  It is committed from its base as the stack deepens -- doubling what is
  committed, by at least 64 KiB and at most 16 MiB at a time unless a push
  needs more (`FrameStack::grow`) -- and never decommitted while the heap
  lives. A frame is its arguments, the header -- `frame_header::COUNT`
  words, 13 in every build, a debug build's generation riding in the flags
  word -- then the rest of its registers, innermost highest; a send's own
  callee lies on its sender's argument slots, which are its arguments,
  every other frame above its sender's whole window. An
  `ActivationReference` is the word offset of its frame's header, and a
  married frame object's married word holds the same offset.
- **Every word of a frame is a valid tagged value**: a number a
  `SmallInteger`, an absent handle nil. Cog's and Primordial Soup's frames
  are the model: a flags word that reads as a `SmallInteger` (Cog's
  `1 + (numArgs << 8)`,
  `prior-art/opensmalltalk-vm/src/spur64.stack/interp.c:52829-52832`;
  Soup's `(numArgs << 1) | isClosure`,
  `prior-art/primordialsoup/vm/interpreter.cc:46-55`), the code named in
  the frame, the receiver and temporaries tagged. Both still hold raw
  words -- a saved frame pointer and a bytecode pointer -- which Cog's scan
  skips by offset (`interp.c:57557-57596`) and Soup turns into indexes
  around every collection (`interpreter.cc:1875-1905`). Here every link is
  an index, since `ns-runtime` forbids `unsafe`, so nothing needs either:
  **the collector traces every tagged word of the stack**, `[0, top)`, as
  Soup scans `[sp_, stack_base_)`, and the table of continuations beside
  it, with no stack map: it walks the frames by their caller words and
  traces each frame's words as it meets the frame, once it has nilled
  the registers the frame does not read before writing them
  (`Heap::trace_stack`; [memory.md](memory.md), "What the collector
  traces"). A married frame
  object still skips its own registers, which are nil while the stack
  holds the live ones. The flags word packs the kind's tag in bits 0-7,
  the argument count in bits 8-23 -- every argument is a register, so the
  register ceiling, `u16::MAX`, bounds it: `Interpreter::push` refuses a
  longer list of values with the too-large error the only body that
  declares more parameters, the raising one, would raise -- `counted` at
  24, `retains_registers` at 25 and, in a debug build, the 31-bit
  generation in bits 26-56, 57 bits inside a `SmallInteger`'s 61; the
  kind tags are `heap/frames.rs`'s own numbering, never written to an
  image.
- **The code is an id, not an `Rc`**: a return reads the caller's code from
  the heap's code table by the `CodeId` in its code word. A method's
  `MixinMethod` is needed only for its selector and its code, which the
  operand and the code word hold; the push registers a method's body with
  its method, so `reify` rebuilds `FrameKind::Method` from the code row.
- **A push** comes in two kinds. A send's own callee
  (`Heap::push_overlapped_activation`) takes the header and the registers
  above it from the end of the send's argument slots, which are its
  arguments and are copied nowhere, writes every word of the header and
  fills its named locals above it with nil -- those below its body's
  `local_count` -- and no scratch register. Every other push
  (`Heap::push_activation`) -- a host's, a `doesNotUnderstand:` handler's,
  `Message>>sendTo:`'s, a control-flow step's block, an unwind block, the
  initializer a factory enters, a resumed or forcing activation -- takes
  its arguments, the header and the registers above from the top, above
  the sender's whole window: the frame stack answers the frame's words and
  every word below it, and the push copies the arguments into the first
  registers -- out of their window in the frame below, or from the values
  a decision or the host built (`interpreter::Arguments`); one or two word
  by word, more by a loop kept inline -- writes the header and fills the
  named locals with nil, never indexing the whole stack once the frame is
  pushed. No push writes a scratch register: no body reads one before it
  writes it (`bytecode::leftovers`, which both lowerings check and the
  decoder holds an image to; [compiler.md](compiler.md), "What a callee
  leaves"), and the collector's pre-pass nils what the stack held there
  before every trace (`Heap::trace_stack`).
  The frame names its body by its code id, found through the
  body's one-entry cache ([memory.md](memory.md), "A closure's code id
  without the map") and kept by a full collection while the activation
  runs ([object-model.md](object-model.md), "An application has two ids");
  only for code that addresses its own slots
  (`CompiledCode::addresses_own_slots`) does it record the receiver's
  guard and the full-width index of `current_class`'s application, which
  every own-slot instruction checks and takes its slot base from, so any
  other push pays one flag test ([object-model.md](object-model.md), "What
  a slot access checks"). Taking the frame is one comparison of its end
  against what may be used without committing more, and the reservation
  stays committed across pops, so once it has grown to the program's depth
  a push commits nothing. The heap's pushes refuse a frame the
  reservation cannot hold with `PushRefused`, and the public
  `Heap::push_activation` with `StackExhausted`; `Mutator::push_placed`
  raises either as the depth limit's `stack overflow: recursion too deep`
  ("The depth limit" below). Every activation the runtime pushes goes
  through that one function, which `Mutator::push` calls for a push above
  the sender's window. It refuses a push past the depth limit before it
  reaches the heap -- the compare sits in `Mutator::push_above` and
  `Mutator::push_on_slots`, the two placements it chooses between -- so no
  new push site can skip the check. It is called from
  `Mutator::push_step`, for the method, block or body a decision
  answered, and for the initializer a factory entered past its first
  instructions decides (`Mutator::enter_factory_body`). It lends the
  header to the heap's push rather than moving it, and hands it on whole
  only to a refusal's cold path, `Mutator::push_refused`. Moved through
  the push's inlined layers, the header, built by narrow stores, was
  copied into each layer's own place sixteen bytes at a time and so read
  back wide right after it was written, a store-forwarding stall on every
  send. Every word is computed before the frame is taken, so a refused
  push changes nothing; the header is then written in place, every word
  once. The caller word packs the caller's offset with the top the frame
  was pushed at -- its caller's window end or past it -- as a distance
  from the frame's start (`frames::caller_link_word`), so that a pop finds the top
  it gives back without reading the caller's header. A push laid on its
  sender's slots never lowers the top (`FrameStack::push_from`), so the
  top stays at a live window end: the sender's registers past a shorter
  callee's window stay below it, traced, and no word comes below the top
  again that a collection never saw. A pop reads the words it needs and
  sets the top back to the one it was pushed at -- the caller's window
  end, or past it where the caller lies on a longer frame's slots -- or,
  for the bottom frame, to its start, and writes nothing: the caller's
  registers above its slots are its own again, dead, and whatever the
  callee or a frame above it left there the collector's pre-pass nils
  before the next trace, which would otherwise keep alive what nothing
  reaches -- a `WeakArray` would not forget it -- or meet a handle to an
  object a collection moved or reclaimed ([memory.md](memory.md), "What
  the collector traces"). A debug build writes `Word::DEAD_REGISTER` --
  under the tag no value carries, so that it cannot be told from a
  program's number -- into every scratch register a push hands out and
  every register a pop gives back (`FrameStack::give_back_dead_registers`),
  and the pre-pass writes it into what it nils; `Heap::register` and every
  other register read an instruction makes refuse it, the dynamic net
  under the analysis that says nothing reads such a register. Bulk copies
  -- a reification's, a suspension's, a divorce's, a trace -- are no such
  reads: a suspension nils the registers the pre-pass would, a divorce
  every scratch register, and the tracers pass over the word as naming no
  object. The registers above the header are
  recorded in their own word rather than read off the code, and the
  arguments in the flags, since a factory is handed as many arguments as
  its send has keywords whatever its register count. An activation nothing
  reifies leaves nothing behind; only a block the compiler did not inline
  reifies one, since an inlined `ifTrue:`/`and:`/`whileTrue:` argument
  creates no closure.
- **A send's arguments are not copied** when its callee is the one it
  decided: the caller's argument slots are the callee's first registers.
  A send whose window names its sources rather than its slots copies them
  into the slots first, on the push path (`Mutator::push_on_slots`); a
  decision answering without a push reads them in place and writes
  nothing ("Overlapping register windows" below).
- **Register access through an `ActivationReference`** is the stack's word
  at the frame's start -- its offset less its argument count -- plus `r`,
  the register numbering leaving the header's numbers out
  ([compiler.md](compiler.md), "Registers"), with no branch on the
  handle's kind or on an args/temps split, and one bounds check, of
  that index against the reservation's committed end (`FrameStack::word`,
  through `Reservation::accessible`). A debug build also checks that the
  index lies below the top and inside the frame's own window, and names
  no header word: the neighbouring activation's header and registers, and
  the frame's own header, are in bounds of the stack, so reading them
  would not otherwise fail. A release build makes
  that trade knowingly, since the run loop makes this access on nearly
  every instruction: a register index past its window but below the
  committed end reads or writes a stale word of the stack -- never another
  object's memory -- and only one past the committed end panics. Only a
  compiler defect produces such an index; the decoder refuses an
  artifact's register out of its body's range
  ([image-format.md](image-format.md), "Verification"). A frame's offset
  is fixed for its whole life and everything is addressed by index, so the
  reservation committing more moves nothing anyone holds. The activation
  below a frame is its caller word, and the one above is found by walking
  the caller words down from the innermost, since a callee on its
  sender's slots starts inside the sender's window. A frame's position is
  its start, which grows with depth -- a callee on its sender's slots
  starts above the sender's header, any other above its window -- so a
  run's floor and every comparison of positions stay an ordering, and the
  run's base has returned once the top is back at its position
  (`Heap::activation_position`, `Heap::activation_stack_top`).
- **Two handle types, not one tagged handle.** `ActivationReference` names a live
  stack frame; `FrameReference` names a frame object and is the only
  activation handle anything *stores*: a closure's captured frame, a frame's
  home and sender, `Continuation::Unwind`'s target, the suspended sender
  `Continuation::ReturnToSuspended` names, a side-table row's frame and the
  image's frame records take a `FrameReference`, so storing a stack
  handle is a compile error rather than a convention. A tagged `FrameReference` would
  have touched only `heap.rs`, but it would put a branch on every register
  access and leave "a stack handle was stored" to a debug-build assertion — the
  silent failure class the handles' private field was built to exclude.
- **An `ActivationReference` is 4 bytes in a release build.** In a debug build it
  carries `origin`, like every handle, and a `generation` drawn from a counter at
  each push and recorded in the frame's flags word. A handle read
  after its frame was popped and the offset reused would otherwise answer a
  different, perfectly valid activation; the generation check turns that
  into a panic at the read.
  `Interpreter::run_from_host`, the one host entry, debug-asserts that it leaves
  the stack as deep as it found it, because a leaked frame is otherwise silent:
  a later activation would sit on top of a dead one's registers.
- **A reified frame is a frame object**: ten fixed fields, the stack frame
  it is married to among them, and its registers inline. Only the heap module reads
  or writes its words; the image encoder and decoder go through
  `Heap::frame_record` and the decoder's in-place fills, which never touch the
  marriage, so nothing outside the heap module can read or forge it. While
  the married field is set, the frame's own registers stay nil and a
  register read or write through the `FrameReference`
  (`Heap::frame_local`/`frame_set_local`) reaches the stack slot. That
  branch exists only on the path from a block to its method's variables —
  `read_outer_local`/`write_outer_local`, behind the VM's
  `LoadOuterLocal`/`StoreOuterLocal`.
- **The accessor prefix names the handle.** `activation_*` reads or writes a live
  activation through an `ActivationReference`; `frame_*` a reified frame through a
  `FrameReference`.

### Lifecycle

**Reify.** `Heap::reify(activation) -> FrameReference` is idempotent: the first call
allocates the frame object — receiver, `kind` and its operand, `current_class`,
`program_counter`, the code id, and `home` when the activation is a
closure's, with a field for every register of the activation's window but
the header's thirteen words -- its arguments, then its registers above the
header (`Heap::frame_field_of_register`) -- all nil —
marries it to the stack frame, and records it in the frame's `reified`
word; every later call
answers the same handle. Its callers are closure creation, the VM's
`MakeClosure`, for the new closure's captured frame; the `currentActivation`
leaf, whose `Activation` object is the frame object itself, so the same
activation answers the same object every time; the unwinder, for a target it
must name across `ensure:` blocks; and the depth limit's reserve. A closure
stores no `home` of its own: `Heap::closure_home`
derives it from the captured frame — that frame itself when it is a method's or
an initializer's, that frame's own `home` when it is a block's ("The closure
keeps its defining activation"). Every block creation reifies the activation
that creates it, whether or not the block reads one of its variables, because
`captured_frame` is also where `Mutator::push_step` reads the block's receiver
and current class from.

**Pop.** The run loop pops every activation it runs, on *every* exit, error
paths included (`vm::run`), and only the top frame can be popped. Popping
hands back the frame's continuation (`Popped`), which the loop applies to
the activation's value ("The VM" below). If the activation was reified,
its frame object is **divorced**: its `program_counter` takes the stack
frame's last stored one, the
married field becomes nil, `uncontinuable` becomes true and `sender` becomes
nil. Then the stack's top is set back to the top the frame was pushed at,
its caller's window end or past it, the caller's registers above its slots
its own again ("Representation" above).

**The registers are copied into the frame object's registers only when the
frame asks for it**, and then only those below its body's `local_count`
-- the arguments, the copied values, the named temporaries and the
indirection vector's register, all a block reaching the frame by
reference can name -- every scratch register nil, since it may hold a
word a callee left. The frame asks through the `retains_registers` flag
of its header, which
`Heap::retain_registers` sets and which `MakeClosure` sets when the block it
creates can reach this frame by reference — `block.outer_reach > 0`
("Converted representation"). Under the reference lowering that is every
activation that creates a closure, so every frame is divorced with its
registers as it always was; under the optimized lowering it is only an
activation whose fallback path actually created a closure. A frame divorced
without its registers keeps the nil it was allocated with in each, so a
register read answers `nil`; the frame object never grows or shrinks, and an
image carries every register it has ([image-format.md](image-format.md),
"Verification"). A frame that was never
reified leaves nothing behind, so storage for activations nothing captured is
bounded by the deepest the program has been rather than by every activation it
has ever made.

**Sender.** A live activation's sender is the frame below it, its caller
word — structural, not a field anyone passes on, so no call site can forget
it, at a block boundary or anywhere else — unless it returns to a suspended
frame, which its continuation names (`Continuation::ReturnToSuspended`,
"Suspending and resuming" below). `Heap::frame_sender_frame` answers either as a frame, the
`sender` leaf's answer. A suspended frame's sender is its `sender` field, the
one place the runtime writes a sender into a frame. A dead frame's `sender` is
`None`: Smalltalk's semantics for a returned context, and what Primordial Soup
does on return (`zap->set_sender(nil)`) and lazily in `HasLivingFrame`; the
divorce clears it. The image format records one per frame, so beyond a
suspended frame only a frame the decoder rebuilt can have one set.

**Only a `FrameReference` is ever stored.** An `ActivationReference` is
valid until its frame is popped, and the next frame pushed there reuses its
offset. Everything that
outlives a call — a closure, another frame, an image record — holds a `FrameReference`.

### Signatures

- Wherever a function means *the running activation* it takes an
  `ActivationReference`: `vm::run` and the loop's helpers,
  `inline_cache::send_lexical` and `send_super`, `sends::decide_lexical_send`,
  `sends::decide_super_send`, `read_local`/`write_local`,
  `read_outer_local`/`write_outer_local`, `raise_return`.
  `FrameReference` stays for closures, a frame's home, the image, and `frame_at_depth`
  from depth 1 on: the first hop goes from a closure activation to its
  `captured_frame`, the rest follow `FrameReference` links.
- **No send or block invocation takes a sender to record.** `Interpreter::send`,
  `decide_send`, `builtins::execute` and the `builtins/` modules have
  no `sender` parameter. It was removed rather than retyped to an `ActivationReference`: its only
  use was to fill a frame's sender, and once a live activation's sender is the
  frame below it, nothing read it. No native takes an activation handle at all.
  The one function handed the sending activation is `Mutator::push_step`,
  which reads its reified frame for the depth count's exemption and stores
  nothing.
- `vm::run` receives the `ActivationReference` of an activation its caller has just
  pushed — its *base* — and pops it, with everything it pushes above it, on
  every path out.
- The image encoder only ever runs between sends, when nothing is live: its
  one writer, behind `image::package` and `image::snapshot`, refuses to start
  while any activation is on the stack, and `Heap::frame_record` debug-asserts
  that no encoded frame is married, the encoder that none is suspended. The
  decoder builds frames that are never married or suspended, and refuses one
  not marked uncontinuable. The format knows nothing of the stack: a frame
  record describes a reified frame, and nothing else is serialized
  ([image-format.md](image-format.md)).

### What an activation carries

`current_class` is required by §3.6: the current class is the class that actually
**defines** the running method, not the receiver's concrete class, and it is what
`self`, `outer` and `super` sends inside that method resolve relative to. It is
set by the dispatch step that pushes the activation and never derived from the
receiver at read time, which is what makes chained `super` sends through a deep
hierarchy resolve one level at a time.

The registers — arguments first, then temporaries — are addressed as described in
[compiler.md](compiler.md). Every activation is pushed with its body's whole
register file, since the code is read before the push — the initializer's
included — and a live window never grows: a frame below the top could not
grow anyway, since its window ends where the next one's begins. A frame
object never grows either: it is allocated with its code's whole register
file ([object-model.md](object-model.md), "The object layout"). Adding a local slot to a
running method — growing its register file, initializing the new registers to nil
and remapping the `program_counter`, the edit-and-continue requirement — therefore means
taking the activation off the stack first; see "How this meets the liveness
requirements" below, which is not built.

`kind` distinguishes a method, closure, factory, initializer,
superclass-clause and top-level activation; an initializer's names the
level it was entered at and which of its mixin's three bodies it runs
(`Initializer(level, body)`), and a factory's the class object it runs
on, whose metaclass's factory entry holds its body. A stack frame's kind
is an `ActivationKind` (`types.rs`, `Heap::activation_kind`), which names
a method by its selector alone and so holds no `Rc`: the flags word's
kind tag says which kind, and the operand word holds its closure, class
or selector. A frame object and an image hold a `FrameKind`, whose
`Method` holds the `MixinMethod`; `reify` rebuilds it from the tag, the
operand and the method the frame's code row recorded at the push. An
initializer activation must be
able to name the code it is executing so that `program_counter` is interpretable, since the
statement→program-counter table is reached through its kind. A stack frame
also names its code, by its code id, because the loop resumes a caller in it
after a callee returns; `continuation`, which names the `Continuation` the
loop applies to its value when it is popped; and `counted`, whether it counts against the depth limit
("The depth limit" below). A `Continuation::Deliver`, every send's, is never
stored: the frame's continuation word is nil, so a send's return tests one
word and reaches no table. Every other continuation lives in a row of the
heap's side table of continuations (`Heap`'s `continuations`), since most
of them carry Rust state no word can (`NewspeakError`, the boxed
nested-class, unwind and forcing records), minted smallest free row first as `Suspensions`' rows are
and taken out and freed by the pop; the table is a root
([memory.md](memory.md), "What the collector traces"). An activation with a
continuation of its own -- a future's computation, a nested class's clause
-- takes a row and gives it back when it returns, and its row is usually
the last: the last row is
never free, so that pop and that push each touch only the end of the
table's vector, inline, and only a row below the last takes the free list,
out of line. A suspension takes its
activation's continuation out of the table into its suspension row, and a
resume puts it back. No loop has a continuation: `whileTrue:`,
`whileFalse:` and `Array>>do:` are Newspeak loops whose blocks deliver as
any send's callee does ([core-library.md](core-library.md), "Real bodies
for control flow and `do:`").

`program_counter` is an instruction index into the activation's compiled body.

`feedback_base` is where the activation's code family starts in the heap's
inline-cache table ([dispatch.md](dispatch.md), "Code families"): a send's
entry is that base plus its call-site number. `push_step` sets it — from the
cache entry that chose a method, from the closure for a block (a fresh range
for a decoded closure, which has none), from the family map otherwise — and
it is `NOT_ASSIGNED` only for a family whose range
could not be allocated, whose sends then run uncached. It is a number, not a
handle, and it is never serialized.

**The chain of running activations is walkable independently of the Rust
stack**: it is the activation stack, innermost last, and below a frame that
returns to a suspended frame, the suspended frames by their `sender`. That is
what the debugger will reflect over and what the collector scans. An image
serializes reified frames only, and is written when nothing is running
([image-format.md](image-format.md)).

### Suspending and resuming

An activation can leave the stack before it returns and come back to it
later: it is *suspended* into its frame object and *resumed* from it. That is
the landing point of every later deoptimization (road step 7b, "Planned: JIT
tiers"). A frame object is in one of three states, told by its married word
(`frame_field::MARRIED`):

- **married**: a `SmallInteger` of 0 or more, the word offset of the stack
  frame it is married to; its own registers are nil, the live ones being the
  stack's;
- **suspended**: a negative `SmallInteger`, `-(row + 1)`, naming its row in
  the heap's side table of suspensions (`Heap`'s `suspensions`); the frame
  holds every register, its program counter and, in `sender`, the frame of
  the activation it returns to, or nil when it returns to the host;
- **returned**: nil, as the divorce leaves it, `uncontinuable` true.

A married or a suspended frame is running (`Heap::frame_is_running`); only a
married one has a stack frame (`Heap::frame_activation`), so every reader
that asks for the stack frame answers none for a suspended frame, and none
mistakes it for a live one.

**Suspending** (`Heap::suspend_activations(floor)`) reifies every frame at
or above `floor` first, so that each sender is a frame by the time the
activation above it is suspended, then works from the top down. It copies
every register into the frame, whatever `retains_registers` says, since the
frame is now their only copy -- a sender's only below the header of the
callee that lay on its slots, the rest nil, so that no frame holds a word of
another's header, and nil for every register the frame does not read
before it writes it, the set the collector's pre-pass nils before the
program counter the frame stores (`CompiledCode::tainted_before`), which
may hold a word a callee or a popped frame left -- writes the program
counter and the sender; moves the
rest of the header into a side-table row; and sets the stack's top as a
pop does. The slots the callee shared come back in both: the callee never
writes its arguments (§3.6), so the sender's copy holds them as the
callee's does. The row holds what a frame object cannot -- the
activation's `Continuation`,
whether it counts against the depth limit, the level it was entered at and
`retains_registers` -- and the frame itself, which makes the table a root
([memory.md](memory.md), "What the collector traces"); a frame object grows
by nothing for being suspended. An old frame that a suspension makes name a
young value is remembered, as any old holder is, since the stack's roots no
longer cover its registers ([memory.md](memory.md), "Frames, the activation
stack and the marriage").

**Resuming is lazy.** `Heap::resume_activation` pushes one suspended
activation back -- its registers copied back from the frame, which is
married to it again, its row freed -- and nothing else. When its frame
names a sender, its continuation is wrapped in
`Continuation::ReturnToSuspended`, which holds that sender and the
activation's own continuation. When such a frame is popped -- by a return,
in `vm::finish`'s cold `return_to_suspended`, or by the unwinder, through
`Mutator::pop_for_unwind` -- the sender is resumed first and the wrapped
continuation applied or abandoned after it, so from there on everything
proceeds as if the sender had been on the stack all along: the caller a
value is delivered to, the activation a continuation pushes, the instruction
a forced future runs again. A resume costs what is returned into and no
more, as Primordial Soup's `CreateBaseFrame` and `LocalBaseReturn` and Cog's
`makeBaseFrameFor` arrange it. `Deliver` stays `finish`'s first, inline arm;
only a run that was suspended ever reaches the second.

A resume derives two things rather than storing them. The feedback base is
decided again by `push_step`'s rule for the frame's kind
(`Mutator::resume_frame`: a closure's from the closure object, every other
body's from `InlineCaches::base_for` of its code), so the side table holds
no number a reclaim could orphan. The recorded guard starts at none, so the
first own-slot access takes the cold check
([object-model.md](object-model.md), "What a slot access checks"). The code
comes from the frame's code id (`Heap::frame_code`); a top-level frame
records none and cannot be resumed, which nothing needs, since the runtime
never pushes a top-level activation.

**A suspended activation still counts against the depth limit.** The count
is one running total that suspending and resuming leave alone, so the check
on every push stays one load and a resume never checks the limit ("The depth
limit" below).

**Everything that walks from an activation to its sender goes on into
suspended frames**: `Heap::frame_sender_frame`, which the `sender` leaf
answers and the kernel's handler search walks; the handler leaves, which read
and write registers through `frame_local` and `frame_set_local` and the
marker through `Heap::frame_unwind_marker`; the unwinder ("Unwinding"
below); and the depth reserve's walks, which follow `interpreter::unwind`'s
`WalkedActivation` -- the live frames, then, past a frame that returns to
a suspended frame, the suspended frames by their `sender`, reading a
suspended `signal`'s error from its row (`Heap::frame_signalled_vm_error`).
A reader of a live frame's own continuation looks through the wrapper
(`Heap::activation_own_continuation`), and `activation_set_continuation`
replaces only the continuation inside it, so a VM error's `signal`
resumed above a suspended reserve still holds the reserve and still returns
to its suspended sender.

**No activation outlives its run suspended.** Every path out of a run pops
through the `ReturnToSuspended` frames, resuming each sender in turn, so
the side table is empty when a run ends; `Interpreter::run_from_host`
debug-asserts it beside its depth assertion (`World::suspension_count`). That
is why the image encoder, which runs only between runs, never meets a
suspended frame. Runs never nest (nothing re-enters the loop from Rust,
which `run_from_host` debug-asserts; "No re-entry from Rust" below), so a
suspended frame whose `sender` is nil returns to the host, and nothing lies
below the run's floor that a sender walk would miss.

**The suspend stress mode runs every program through it.** With
`NS_NEXT_STRESS_SUSPEND` set to anything but empty or `0`, a debug build
keeps the safepoint flag set, and at the safepoints its schedule picks --
paced by the traced size as the collect and scavenge modes are
(`StressSchedule`, `interpreter/safepoint.rs`; [memory.md](memory.md), "The
move stress mode") -- `Interpreter::safepoint` suspends every activation of
the run, from the run's floor up, and resumes the top one at once, so that
the loop always runs on a stack frame. It takes its turn after the other
stress modes, which leave the stack as they find it. The safepoint answers
whether it rebuilt the stack, and `vm::run` then reads its `FrameRegisters`
again from the top frame; for that the stretch writes the running
activation's program counter into its frame when it leaves for a safepoint,
one store on a cold path. Every golden and corpus module keeps its recorded
outcome under it, so every return, unwind, handler search and depth-limit
path runs from suspended frames, the path a deoptimized frame will take;
`crates/ns-runtime/tests/stress.rs` pins the cases one by one, plain and
stressed, under both lowerings. It is a gate of its own at the end of a
batch, beside the other five ([memory.md](memory.md), "The gates the
collector needs").

## The VM

Every body a program runs hangs off the declaration it belongs to — a method's
off its `MixinMethod`, an initializer's and a superclass clause's off the
`Mixin`, a factory's off its factory entry — so what dispatch finds is what
runs; nothing looks code up beside it.
Everything in `interpreter/` decides *what* to run; `vm.rs` runs it.

`vm::run(interpreter, base)` is the run loop. It is handed an activation its caller
has just pushed — its *base* — and runs it and everything it activates, in a
loop over the running frame's state held in locals (`run_stretch`, a `match`
over the running body's eight-byte `CompactInstruction`s; "The loop over
locals" and "The compact instruction encoding" below). **Each stretch
between two safepoints is one entry** (`Interpreter::enter`, [memory.md](memory.md), "The
brand"): `run_stretch` runs inside it with a `&mut Mutator`, and `run` itself —
the frame it carries from one stretch to the next (`FrameRegisters`) and the
safepoint — is outside every one, so nothing `run` carries can name a brand.
A run ends inside its last
entry: `vm::finish` or `vm::unwind` roots its answer for the host
(`Mutator::to_host`), and `run` answers that `HostResult`, never a branded
value. It never recurses: a send pushes
the callee and switches to it, a return pops the callee and switches back to
the caller. It pops everything it ran, `base` included, on every path out — a
return from `base`, an error, or a non-local return aimed below `base` — so the
stack is exactly as deep afterwards as it was before `base` was pushed. The
depth `base` sits at, the run's *floor*, is how the loop tells its own
activations from its callers'.

- **A send** (`vm::send`) names its arguments where they are in the
  caller's registers, an argument window (`vm::argument_window`; "No
  re-entry from Rust" below), stores the caller's `program_counter`, and asks
  `interpreter/` what the send means: `inline_cache::send_public`,
  `send_lexical` or `send_super`, which answer from the call site's cache or
  run the lookup ([dispatch.md](dispatch.md), "3b — The caches"), or, for
  the reference lowering's `SendInlinable`, `Mutator::decide_send`. Each
  answers a `Step`. `Step::Value` — a native leaf, a slot access, a nested class or
  a level whose initializer runs no code — is written to the send's `destination`,
  and the caller goes on. Anything else is an activation: `Step::Method`,
  `Step::Block`, or one of the bodies an instantiation or a nested-class
  accessor runs, `Step::Factory`, `Step::Initializer` and
  `Step::SuperclassClause`. The three initializer instructions that may
  start an activation -- `InitializeInstance`, `InitializeSuperclass` and
  `InitializeAppliedLevel` -- go through the same `vm::send`, with their
  decision in `interpreter/instantiate.rs` in place of a lookup. A step is
  pushed by `Mutator::push_step`, which checks a method's, block's or
  factory's arity and the depth limit, and the loop switches to the new activation at `program_counter` 0,
  its window filled straight from the caller's registers -- except a
  factory, which `Mutator::push_sent` carries through its first two
  instructions where it pushes it, and which the loop starts where that
  leaves it ([object-model.md](object-model.md), "Instantiation"). A send
  gathers its arguments nowhere and allocates nothing for them.
- **A return** (`vm::finish`) pops the activation and applies its
  continuation. `Continuation::Deliver`, every send's, hands the value to the
  caller: `resume_caller` reads the `program_counter` the caller stored at its send, writes
  the value to that send's `destination` and continues at the next instruction — or,
  when the popped activation was `base`, the loop leaves `run` with it. Every
  other continuation — the nested-class accessor's, an unwind's, a VM
  error's `signal` — is applied by `Mutator::resume`, which
  calls into `interpreter/`; it may answer a value, another
  activation — the next step of a nested class's creation — which
  `Mutator::push_continuation` pushes in the popped activation's place, or
  a signal, an unwind carried on ("The trampoline" below). A future's
  forcing, `Continuation::Future`, is the one `finish` does not hand to
  `resume`: `Mutator::future_computed` records the value, and the loop
  delivers it, forces the next future of a chain, or runs the caller's
  stopped instruction again ("Running an instruction again after forcing a
  future" below). An activation whose sender is suspended carries
  `Continuation::ReturnToSuspended`, `finish`'s second arm, cold and out of
  line: it resumes the sender and then applies the continuation it wraps
  ("Suspending and resuming" above).
- **A signal** — a Newspeak error, a non-local return, or an exception's
  `return:`, `retry` or termination — goes to `vm::unwind`, which hands it
  to the one unwinder, `interpreter::unwind`, and carries out what it
  answers: the target returns or restarts, an `ensure:` or `ifCurtailed:`
  block the unwind runs on its way is pushed, an error's `signal` is
  pushed, or `run` ends with the signal ("Unwinding" below). Every
  activation the unwinder pops hands its continuation to
  `Mutator::abandon`, which undoes what that continuation had begun; the
  pop is `Mutator::pop_for_unwind`, which first resumes a suspended sender
  the popped entry returns to.

Five kinds of body run, and each reaches the loop one way:

1. **Method body** — pushed by `push_step` for a `Step::Method`. A method
   activation catches a `^` that targets it (`Mutator::catches_return`).
2. **Block body** — pushed by `push_step` for a `Step::Block`, which names the
   closure object; the activation runs the code that object's code id
   names. `value` and the control-flow steps — `True`'s and `False`'s
   `^<parameter> value` quick methods — answer the block as a step.
3. **Factory body** — pushed by `push_step` for a `Step::Factory`, which
   `act` answers for a factory entry the lookup found, on the class object
   the factory was sent to; its value is delivered like a method's. A send
   in the loop enters it past its `AllocateInstance` and
   `InitializeInstance` (`Mutator::enter_factory_body`;
   [object-model.md](object-model.md), "Instantiation").
4. **Instance initializer** — pushed by `push_step` for a
   `Step::Initializer`, which the factory body's `InitializeInstance` and an
   initializer's `InitializeSuperclass` and `InitializeAppliedLevel` answer:
   one of the level's mixin's three bodies, entered at that level. Its value
   goes to the instruction that started it, which does not use it; a level
   whose body would run nothing answers `Step::Value`, the instance, and is
   not entered. A user class whose mixin carries no full initializer is
   refused on load (`image::decode::verify_mixins`); a mixin a host built
   by hand has none, and its level is not entered.
5. **Superclass clause** — the nested-class
   accessor's clause is a `Step::SuperclassClause`, continuing with
   `Continuation::NestedClass`. A clause's message is no body of its own:
   its arguments are lowered into the initializer of the declaration whose
   clause it is ([compiler.md](compiler.md), "The three initializer
   bodies").

Rust enters the loop in one place, `Interpreter::run_from_host`, for a host's
`send` or `instantiate`, and only from outside it: it pushes what the decision
answered and runs `vm::run` on it. No decision, native or continuation runs
code from Rust, and a debug build checks that the loop is never entered while
it runs ("No re-entry from Rust" below).

**The safepoint.** `run_stretch` tests one flag the heap owns
(`Heap::safepoint_requested`) at every switch to another activation — a push,
or a caller resumed — and at every backward jump (the `jump_to!` macro every
jumping arm goes through), so every call and every loop iteration passes one,
a loop that makes no send included; a forward jump does not test it. When the
flag is set the stretch writes its frame back to `FrameRegisters` and leaves
its entry (`Leave::Safepoint`), and `run` calls `Interpreter::safepoint`
(`interpreter/safepoint.rs`), out of line, which is where a scavenge or a
collection runs ([memory.md](memory.md), "Where a collection can happen"), and
enters again. A scavenge is requested by an allocation that fills eden to its
threshold or finds eden spent, and by a remembered set past its limit, through
the barrier or after a scavenge; a full collection once the old space's
occupancy reaches its limit, checked by an allocation that goes old and after
every scavenge ([memory.md](memory.md), "Generations"); and the debug-build
move, cache, collection, scavenge and suspend stress modes keep the flag set
while they are on. The stretch also writes the running activation's
`program_counter` into its stack frame as it leaves, so that a suspension at
the safepoint finds it; `Interpreter::safepoint` answers whether the suspend
stress mode rebuilt the stack, and `run` then reads its `FrameRegisters`
afresh from the top frame ("Suspending and resuming" above). That is sound
because of what `run` carries across the safepoint, which the compiler holds
it to, since the stretch's entry has ended before it: `FrameRegisters` — the
running `ActivationReference`, the word offset of its frame on the stack, and
no stack frame moves (a rebuilt stack is read again, above); its
`Rc<CompiledCode>`, which holds no handle; its `program_counter`; its
`FeedbackBase`, a number; and its `RegisterBase`, the word offset of its first
register, which a move does not touch — and the run's floor, a word offset on
the stack. Every value the loop works on is read from
a register or an operand when an instruction needs it and written back before
the next, so at a safepoint every handle is in a word of a stack frame,
in the heap's side table of continuations or of suspended
activations, in a heap object or in a root, where
the tracer rewrites it. A send's arguments are read in its slots, so nothing
outside the stack holds them while it is decided.

**The VM has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch and no error text of its own.** It pushes, pops, switches, writes
registers and reads instructions' operands; which body runs next is always
decided in `interpreter/` or `builtins/`.
Every error a program can observe is produced there. That keeps dispatch in
one place, which the reference lowering and a later JIT will reach unchanged —
and it is why `CLAUDE.md`'s "`vm.rs` holds no dispatch logic" forbids copying
anything out of `interpreter/` into `vm.rs`.

**The inline caches keep it that way** ([dispatch.md](dispatch.md), "3b —
The caches"). `run_stretch` holds the running activation's `feedback_base` in
a local beside its code, a switch's `Handoff` carries it, and a send passes it,
its code's `family_size` and the instruction's `CallSite` to
`interpreter/inline_cache.rs`, which probes the cache there; `MakeClosure`
copies it into the closure. `vm.rs` learns three numbers and no dispatch.
**The stretch is forced inline** (`#[inline(always)]` on `run_stretch` and on
the closure `run` hands to `Interpreter::enter`): every cached send inlines
its hit path into it, and past LLVM's inlining threshold it would become a
function of its own. When each instruction was one entry, that cliff made
every instruction pay a call and return its answer through memory —
6.6–13.6% on every benchmark when it happened ("What the inline caches
moved"). The stretch sits near that threshold, and anything added to a
send's hit path lands in it. `dumpbin /disasm` shows one `ns_runtime::vm::run` and no
`vm::run::closure` or `run_stretch` ("What the loop over locals moved").

Closure invocation is not special-cased into an opcode: `Closure value` is still a
native method, which answers the closure as a step; the closure's activation is
pushed above the one that sent `value` — its sender, by position — and a wrong
argument count is still the same error.

## Where `program_counter` is stored

An activation's `program_counter` is written **only by the instructions that can hand the
activation to an observer**: every send, since a nested activation runs above it;
`MakeClosure`, since the activation escapes into the object graph — the store
comes just before `reify`, which copies it into the frame it creates; and
`NonLocalReturn`, since control leaves with a signal; an instruction that
meets an unresolved future (`vm::force_and_reexecute`), since the forcing
runs above it and the loop runs that instruction again; and every
instruction that raises an error (`vm::raise`), since the raising
activation stays on the stack below the error's `signal` while the
handler runs, and a collection meanwhile nils its dead registers as of
the instruction its header stores ([memory.md](memory.md), "What the
collector traces") -- as of an earlier send it would keep what that
send's callee left in registers the frame wrote before it. The stretch adds one
more, off the hot path: leaving for a safepoint, it stores the running
activation's, since a suspension there copies it into the frame
("Suspending and resuming").

The stores go to the stack frame (`Heap::activation_set_program_counter`), so
they keep the activation's `program_counter` current for as long as it runs; a
frame married to it holds no `program_counter` of its own that anything reads
meanwhile. When the activation returns, the divorce gives the frame the last
`program_counter` stored — often a later send's, not the `MakeClosure` that
reified it — and a suspension gives the frame the same, which a resume gives
back to the entry.

The loop itself reads it back. While a callee runs, the caller's only record
of where it stopped is the `program_counter` its send stored; when the callee
returns, `vm::resume_caller` reads that `program_counter`
(`Heap::activation_program_counter`), writes the value to the destination
register of the send it names, and resumes the caller at the next instruction.

Nothing can read `program_counter` while a straight-line instruction executes, because the
loop never switches activations between them. Keeping the store out of the
straight-line arms takes a write off the hot path for a value nothing observes
there.

Whether a debugger will force `program_counter` at every instruction is an open question for
the milestone that builds one; it is a deliberate trade, not an oversight.

## Unwinding

A closure's `home` is the reified frame of the method, initializer or
superclass-clause activation that lexically encloses it.
It is not a field: `Heap::closure_home` derives it from the closure's
`captured_frame` ("The closure keeps its defining activation").
`Signal::Return` carries a `FrameReference`, its target, and `raise_return` picks
it: for a closure activation, its closure's `home`, while that frame is
running (`Heap::frame_is_running`: married to a stack frame or suspended into
the frame), and a `CannotReturn` if it is dead, either divorced because the
activation returned or built by the image decoder, which never marries or
suspends a frame. For any other activation
— an initializer's `^` from an inlined block — the target is the activation
itself, reified for the purpose.

The one unwinder, `interpreter::unwind`, carries it down the stack;
`vm::unwind` only does what it answers. A target whose frame is married to the
activation on top of the stack, which sits at or above the run's floor and
catches a return (`Mutator::catches_return`: a method, a superclass clause or
its message), ends the unwind: every activation above it has been popped, and
the target returns the value through the ordinary return path, so a reified
frame is divorced exactly as on any return. The unwinder decides that again
before every pop, from the target's frame (`Heap::frame_activation`): a
suspended target has no stack frame yet and gets one only when the pops
reach it, since every pop is `Mutator::pop_for_unwind`, which resumes the
suspended sender of the frame it pops before abandoning that frame's
continuation. So the unwinder resumes a target only when it is suspended, and
only by popping down to it, as `retry` keeps popping down to its `on:do:`
activation ("Suspending and resuming" above). The popped activations'
continuations are not applied, so a `^` out of a superclass clause or an
`ensure:` block abandons the class's creation or the unwind; they go to
`Mutator::abandon` instead, which unmarks a nested class's creation in its
cache slot and drops every other. A return whose target catches none, or has
returned meanwhile, pops every activation of the run, `base` included, in the
same way — running the pending blocks on its way — and leaves `vm::run` with
the signal, for the host that started the run.

**The exceptions' unwinds are the same unwind.** `Exception>>return:` and
`resume:` answer `Signal::Return` aimed at the `on:do:` or the `signal`
activation's frame, and `retry` answers `Signal::Restart`, aimed at the
`on:do:` activation's, which the unwinder carries down exactly as a `^`; at
a restart's target it sets the program counter to 0 and the loop switches
to it, its registers as they are. A restart whose target has returned
meanwhile ends the run with a `CannotReturn`, the error §7.2 calls
`cannotReturn`: the unwind has passed every handler by then. An unhandled exception's
`defaultAction` answers `Signal::Terminate`, an unwind with no target: it
runs every pending block down to the floor and leaves the run with the
error, which the host receives as it received every error before there were
exceptions. A pending unwind is a `PendingUnwind` — its target frame, if
it has one, and whether it returns a value, restarts or terminates — and it
is carried across a block it runs as what it was
([core-library.md](core-library.md), "Exceptions").

**An error is not unwound at all.** Once the kernel is installed, a
`Signal::Error` a step raised becomes an `Error`, and the unwinder pushes
`signal` on it above the activation where it arose, with the continuation
`Continuation::VmErrorSignalled`, and pops nothing: the handler runs on top
of the stack, and what is popped is decided by the handler or the default
action, through the unwinds above. Before the kernel is installed there is
no `Error` class, and an error pops every activation of the run, running no
block, and leaves with the error.

**`ensure:` and `ifCurtailed:` blocks run on the way.** `Closure>>ensure:` and
`ifCurtailed:` are kernel methods whose code carries an unwind marker
(`bytecode::UnwindMarker`, recognized when the kernel is compiled,
`compiler::unwind_marker`) naming the registers of their block and of their
`complete` local. Before the unwinder pops an activation whose marker is one of
the two and whose `complete` is still nil, it sets `complete` and answers the
block; the loop pushes it above the marked activation, which stays on the stack,
with `Continuation::Unwind` holding the unwind in progress, and when the block
returns `Mutator::resume` raises the signal again, which carries on from the
marked activation. A newer unwind or an error that pops the block first drops
the old one. `ensure:`'s own normal path sets `complete` before it runs the
block, so the block runs exactly once whichever path gets there first. An
argument that is no closure is sent `value` through the kernel's
`Closure>>valueOnUnwind:`, whose own return carries the unwind on. An activation
without a marker costs the unwinder one test.

Because those blocks run code while an unwind is in progress, an activation
they push can reuse the stack index a popped target used. That is why the
target is a frame, never an `ActivationReference`: a release build has no
generation to tell two activations at one index apart, while a frame is
divorced on its activation's pop and married to nothing afterwards, so a
target popped meanwhile is found dead rather than mistaken for its successor
(`interpreter::unwind`'s `a_target_survives_its_index_being_reused`). The
unwinder asks the frame for its activation afresh each time it carries on,
and compares only that handle (`ActivationReference::same`). The depth
limit's reserve is held by a frame for the same reason ("The depth limit"
below).

**An initializer home follows the same rule.** A block created during
initialization — `| blk = [ ^2 ]. |` — whose `^` runs after initialization has
finished signals a `CannotReturn`, as `newspeak-spec.pdf` §7.2 requires, because
the initializer's frame was divorced when its activation returned. A `^` into an
initializer that is still running escapes: an initializer activation catches
no return, so the unwind pops it with everything else down to the floor of the
host's run, running every pending `ensure:` block on its way, and leaves
with the signal, and it reaches the top of the entry send as "internal:
non-local return escaped the entry send", a known gap
(`crates/ns-runtime/tests/trampoline.rs` pins it under both lowerings). A
`^` in a block in a superclass or `<:` clause's argument is such a `^`
too: the argument is lowered into the initializer of the class whose
clause it is (§6.3 evaluates the message "in the context of *i*"), so the
`^` targets that initializer's activation and escapes the instantiation;
its value is never handed to the superclass as the clause's argument
(`tests/vm_initializer.rs`'s
`a_return_inside_a_block_in_a_clause_argument_escapes_instantiate`).

A method body's own `^` is an ordinary `Return` and raises no signal. The
observable discipline — a block invoked after its home has returned gets
a `CannotReturn` — comes from the pop, which divorces a reified home on every exit
path, without a signal round-trip on the common path.

**Never `.unwrap()`, `.expect()` or `{:?}` an `EvaluationResult`.** `Signal`
deliberately carries no `Debug` impl of its own -- every field it holds
could support one today, but leaving `Signal` without one means
`EvaluationResult` (`Result<Value, Signal>`) has none either, and
`Result::unwrap`/`expect` require `E: Debug`. Calling either, or writing
`{:?}` on an `EvaluationResult`, is therefore a compile error, not a lapse a
reviewer has to catch. The same holds for the `HostResult` a host receives
(`Result<Root, HostSignal>`, [memory.md](memory.md), "Host roots"): `Root`,
`HostSignal` and `HostError` have no `Debug` either. Tests unwrap through a
local `expect_ok` helper.

## Measurements

`ns-next bench` times the entry send alone — parsing, resolving, bootstrapping and
compiling excluded — and reports the median of ten timed runs after a discarded
warm-up. Its three fixtures each run a 50,000-iteration `whileTrue:` loop, which
the compiler inlines: `bench_loop.ns` sums the counter, `bench_send.ns` adds a
user-method send per iteration and `bench_block.ns` a method that creates and
invokes a block. On a release build, on 2026-09-16, with the inline caches,
their medians were about 10.1, 11.1 and 19.4 ms ("What the inline caches
moved" below).

Never judge performance from a debug build.

Every figure in this section is a median of a whole-program benchmark on one
machine. Only numbers measured against each other in one sitting are compared
here; a figure carried across sessions says more about the machine's load than
about the code.

### The Are We Fast Yet benchmarks

`benchmarks/are-we-fast-yet/` holds real programs to measure on, beside the
microbenchmarks under `benchmarks/micro/`: all fourteen Are We Fast Yet
benchmarks -- Bounce, CD, DeltaBlue, Havlak, Json, List, Mandelbrot,
NBody, Permute, Queens, Richards, Sieve, Storage and Towers -- and
Primordial Soup's own Richards and DeltaBlue, `RichardsTon80`, derived from
the Dart Ton80 version, and `DeltaBlueTon80`, beside them, and the
harness that runs them, `AreWeFastYet.ns`, the root of the project
whose `newspeak.json` lies beside it, which reaches each benchmark as `manifest Sieve`
([image-format.md](image-format.md), "Packaging"). Every benchmark is one
module: SOMns's CD loads a second module, `RedBlackTree.ns`, which here is
folded into `CD.ns` as nested classes of `CD`, so CD too is reached as
`manifest CD` (decided with the user on 2026-10-01). The benchmarks are
adapted from SOMns's Newspeak versions
(`prior-art/somns/core-lib/Benchmarks/`, MIT) into Primordial Soup's
module shape and protocol: each is a module `usingPlatform:` whose one
public method, `bench`, runs the workload once and signals an `Error` when
its result is not the one Are We Fast Yet expects. The harness is adapted
from Primordial Soup's `BenchmarkRunner.ns`
(`prior-art/primordialsoup/newspeak/BenchmarkRunner.ns`, Apache 2.0),
`RichardsTon80.ns` from its `Richards.ns`
(`prior-art/primordialsoup/newspeak/Richards.ns`, Apache 2.0), and
`DeltaBlueTon80.ns` from its `DeltaBlue.ns`
(`prior-art/primordialsoup/newspeak/DeltaBlue.ns`, Apache 2.0). Each
file's header names its upstream file and commit and what was changed, and
`THIRD-PARTY-NOTICES.md` carries SOMns's and Primordial Soup's licences.
Three SOMns files carry their upstream's own licence rather than SOMns's
MIT, and the ports keep it in their headers, as SOMns's files do:
`Mandelbrot.ns` Brent Fulgham's BSD-3 text, `CD.ns` Purdue's and Apple's
BSD-3 text (for both of SOMns's files), and `Havlak.ns` Google's Apache
2.0 text. The protocol
they send is what the kernel and the platform gained for them
([core-library.md](core-library.md), "What exists", "The platform").

Mandelbrot runs at 750, which answers 50: SOMns's verified size
(`setupVerifiedRun:`), where Are We Fast Yet's own `rebench.conf` runs 500
(the user kept 750 on 2026-10-01). NBody runs at Are We Fast Yet's 250000
steps, which answers an energy of `-0.1690859889909308`. Each takes
seconds per run in a release build, so `--timed` (below) measures them
with one run per phase: its score for them is one run's rate, not a
throughput over many runs. Decided with the user on 2026-09-30, who chose
these sizes over smaller ones.

DeltaBlue at 12000, Havlak at 1500, which answers 6102 loops in a graph of
5213 nodes, and CD at 250, which answers 10830 collisions, are
`rebench.conf`'s sizes. Json parses its input once, since its 100 there is
a repetition count, and checks what SOMns's `verifyResult:` checks;
`DeltaBlueTon80` keeps Primordial Soup's 100 and 100 (decided with the
user on 2026-10-01). Release times of one run, measured while porting:
about 0.9 s for DeltaBlue, 5 s each for Havlak and CD, and 15 ms for Json.
CD's count does not depend on the last bits of `sin` and `cos`, whose leaves
call Rust's, which call the C library's as Primordial Soup's do
(`builtins/numbers/float.rs`): correctly rounded values, values one ulp up and
one ulp down, the Windows C library's values and a Taylor series off by up
to 557 ulps all gave 10830 (checked on 2026-10-01, while planning stage 3).

So the tests that run every benchmark are not part of the suite.
`crates/ns-cli/tests/run_benchmarks.rs` runs the harness on each benchmark
by name, at full size, under both lowerings, each run a test of its own,
and holds it to its line of `benchmarks/are-we-fast-yet.toml`,
`<Name>: ok`; the hints' behaviour oracle does the same, every hint
applied; the microbenchmarks under `benchmarks/micro/` go the same way
against the `.toml` beside each; and `timed::<Name>` runs each benchmark
with `--timed` and checks its score's shape. The tests verify; they measure
nothing, since a debug build's times say nothing. With Havlak and CD, the
slowest of stage 3, all sixteen programs took 57 s under the optimized
lowering and 136 s under the reference lowering one after another (timed
on 2026-10-01); run one by one in parallel, the whole
`--profile benchmarks-verify` run takes about 47 s, the longest single run
Havlak's under the reference lowering (2026-10-14). Under the stress modes
the gate took 37 minutes with them (MOVES, 2026-10-01).
`.config/nextest.toml`'s default profile leaves them out, so neither the
per-commit gate nor a stress run reaches them. They run on demand, when benchmarking, and once at the end of a
branch, in a plain run without a stress mode (decided with the user on
2026-10-01):

```bash
cargo nextest run --workspace --profile benchmarks-verify
```

There are two ways to measure, and they report different things:

- **`ns-next bench benchmarks/are-we-fast-yet -- <Name>`**,
  from the repository root, times the harness
  without `--timed`, which runs the benchmark named -- or, with no name,
  all sixteen in turn -- once and prints `<Name>: ok`. `bench` reports the
  median, minimum and maximum of its timed runs (ten unless `--iterations`
  says otherwise) after one discarded warm-up run. Each timed run is one
  `main:args:` of a freshly assembled application, so it includes what
  that send does once per application -- instantiating the harness's
  `Benchmarking` and each benchmark module, building their nested class
  objects, and the workload's single run, with no warm-up of its own --
  while assembling the application, `packageUsing:` its manifest, is
  excluded from the timed span, as parsing, resolving, bootstrapping and
  compiling are. What carries over from the discarded run is what one
  interpreter keeps between sends: the inline caches and the platform's
  lazy members. The untimed run does the same work every time, which is
  what lets `benchmarks/are-we-fast-yet.toml` pin its output and `bench` time it.
- **`ns-next run benchmarks/are-we-fast-yet -- --timed [--warm-up <milliseconds>] [--measure <milliseconds>] [<Name>]`**
  lets the
  harness measure itself, as Primordial Soup's does: for each benchmark, or
  the one named, it runs `bench` repeatedly for at least the warm-up window,
  3 ms unless `--warm-up` says otherwise, then for at least the measurement
  window, 20 ms unless `--measure` says otherwise, reading
  `platform time Stopwatch`, and prints
  `<Name>: <runs per second>` with one fraction digit (`asStringFixed: 1`).
  That score is a throughput after warm-up, comparable with Primordial
  Soup's own; it varies from run to run, so only its shape is checked
  (`crates/ns-cli/tests/run_benchmarks.rs`, `timed::<Name>`). The two flags
  are accepted only after `--timed`, in either order, each at most once;
  a value is a non-negative decimal integer, read with `Integer parse:`,
  whose `ArgumentError` the harness reaches as `platform kernel
  ArgumentError`. A flag without `--timed`, a repeated flag, a flag without
  its value or any other value signals the usage error, whose text names
  the flags. Every `--timed` score in this section was taken with the
  default windows. The Java counterparts (below) take the same flags with
  the same defaults (decided with the user on 2026-10-22).

Either way the protocol of the `measuring-performance` skill applies, as it
does to every figure in this section: release builds only, and every binary
compared, the current tree included, built with the same explicit
`RUSTFLAGS` that pin function alignment, several builds per tree pooled
("What removing the re-entries moved" below). The five programs that take
seconds per run -- CD, Havlak, Mandelbrot, NBody and DeltaBlue -- are
timed with `bench --iterations 3`, every other program with
`--iterations 10` (decided with the user on 2026-10-02): a run of seconds
varies by far less than the changes measured, and ten of them made a
comparison of three builds a side take an hour and a half, three bring it
to about half an hour. A comparison of two ns-next trees uses `bench`
alone (decided with the user on 2026-10-02): on the eleven programs whose
`--timed` score is a throughput it agreed with `bench` ("What the
eight-byte value moved"), on the five slow ones it is one run's rate to one
fraction digit, and it added about eighteen minutes. `--timed` is for a
comparison with Primordial Soup or another VM, whose scores it matches in
form, and for a short program whose per-application setup might hide a
change in its workload.

**The Java counterparts.** `benchmarks/are-we-fast-yet/java/` holds a Java
counterpart of each of the sixteen programs and of the harness, so that
ns-next's scores can be put beside a JVM's under one protocol. Decided with
the user on 2026-10-22:

- **Same work, not idiomatic Java.** The Java programs keep Are We Fast
  Yet's rules: the algorithm of the Newspeak version, and the `som`
  collections (`Vector`, `Dictionary`, `Set`, ...) in place of
  `java.util`, which only the harness uses. A comparison measures two VMs
  on one program, not two programs.
- **Upstream's Java, adapted.** The fourteen benchmarks and the harness
  are Are We Fast Yet's own Java versions
  (`prior-art/are-we-fast-yet/benchmarks/Java/src/`, commit
  `74306fec151070fd07157cefeacf19e7e0bcdc89`), adapted, not translated
  afresh. Only `RichardsTon80` and `DeltaBlueTon80`, which have no Java
  version there, are translated from our `.ns` ports, class by class and
  method by method, keeping their sizes (100 and 100) and their checks:
  each module's slots are a `Constants` class and its nested classes files
  of their own under `richardston80/` and `deltablueton80/`, the ports'
  `List` is `som.Vector`, and a value is a `long` wherever the `.ns` value
  can exceed 2^31, since Java's `int` overflows silently where Newspeak's
  integers do not.
- **No microbenchmarks.** `benchmarks/micro/` gets no Java counterpart: it
  times the interpreter's internals in loops a JIT removes, so a Java score
  for it says nothing.
- **Plain `javac --release 21`**, no build tool, no JMH, no dependency.

The layout is upstream's package structure: in the default package the
harness, `AreWeFastYet.java` (upstream's `Harness.java` and `Run.java`
folded into one), `Benchmark.java` and one file per benchmark; beside them
the packages `som/`, `cd/`, `deltablue/`, `havlak/`, `json/`, `nbody/`,
`richards/`, `richardston80/` and `deltablueton80/`. Upstream's
`build.xml`, `build.sh` and checkstyle configuration are not taken. What
changed from upstream is what the `.ns` ports changed, and no more.
`Benchmark` has one method, `void bench()`, into which upstream's
`benchmark()`, `verifyResult(Object)` and `innerBenchmarkLoop(int)` are
folded: it runs the workload once at the `.ns` port's size and throws an
`IllegalStateException` whose message is the port's `Error` text
(`Sieve: expected 669, got …`) when the result is wrong. The sizes and
results are the ports' (above: Mandelbrot 750, NBody 250000 steps,
DeltaBlue 12000, Havlak 1500, CD 250, Json parsing its input once), every
departure a port's header names is named in its Java file's header too,
and the checks for upstream's other sizes are dropped. Names are written
out in every identifier the adaptation touches, as in the ports. The one
addition to the `som` collections is `Vector.removeLast()`, for
`deltablueton80`, whose port takes its work lists from the end: upstream's
`Vector` has no removal from the end and keeps its storage private, and
taking from the front would change the order in which the planner visits
the constraints. Any other departure from upstream's algorithm is a
defect. Each file starts with a comment naming its upstream file and
commit and what was changed, then upstream's notice verbatim; a file that
has no notice upstream keeps upstream's comment as it is, and the CD files
add the notice of CDx, from which they derive (below).

Upstream is under several licences, which its `LICENSE.md` describes, and
each adapted file keeps the notice it carries upstream;
`THIRD-PARTY-NOTICES.md`, "Are We Fast Yet", carries the texts:

| Files | Notice |
|---|---|
| `AreWeFastYet`, `Benchmark`, `som/*` but `IdentityDictionary`, `Bounce`, `List`, `Permute`, `Queens`, `Sieve`, `Storage`, `Towers` | MIT, the SOM class library's authors |
| `CD`, `Havlak` | the same MIT text, `Copyright (c) 2001-2016 Stefan Marr`; `CD` adds the BSD-3 notice of `cd/*` |
| `Json`, `json/JsonPureStringParser` | MIT, Stefan Marr |
| the rest of `json/` | MIT, EclipseSource (minimal-json) |
| `havlak/*` but `UnionFindNode` | Apache License 2.0, Google; `UnionFindNode` carries no notice |
| `Mandelbrot` | Revised BSD, Brent Fulgham (the Computer Language Benchmarks Game) |
| `NBody`, `nbody/*` | none of their own: their comment names the Benchmarks Game, whose Revised BSD text upstream's `LICENSE.md` carries |
| `Richards`, `richards/*`, `DeltaBlue`, `deltablue/*` | none of their own: derived from Mario Wolczko's versions, they point to his licence page, as `LICENSE.md` does |
| `cd/*` | BSD-3, Purdue University 2001-2010 and Apple 2015, copied from `CD.ns`'s header: upstream's Java files omit it, but derive from Purdue's CDx under it, as do upstream's JavaScript and SOM versions of CD, which carry it |
| `som/IdentityDictionary` | none: upstream's file carries no notice, and `LICENSE.md` names none for it |
| `RichardsTon80`, `DeltaBlueTon80`, `richardston80/*`, `deltablueton80/*` | Apache License 2.0, the Newspeak project authors: the header of the `.ns` port each is translated from |

The files with no notice at all are `som/IdentityDictionary` and
`havlak/UnionFindNode`, beside `NBody` and `nbody/*`, whose comment names
the Benchmarks Game without a licence text. The CD files are the one place
a notice is added: upstream's Java `cd/*` carry none and `CD.java` only
the MIT text, but CD is a port of Purdue's CDx, whose BSD-3 notice
`CD.ns` (from SOMns) and upstream's `cd.js` and `CD/*.som` carry, so
each of them carries it too, after its adaptation comment (and, in
`CD.java`, after the MIT notice), and its header says why.

`AreWeFastYet.java` takes the `.ns` harness's command line and prints its
lines:

```
java -cp <classes> AreWeFastYet [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [<Name>]
```

It registers the benchmarks in `AreWeFastYet.ns`'s order. Without
`--timed` it runs each, or the one named, once and prints `<Name>: ok`,
the line written only after `bench()` returns, so a benchmark that throws
leaves none; with `--timed` it measures each for the warm-up window, then
for the measurement window, and prints `<Name>: <runs per second>` with
one fraction digit, formatted in `Locale.ROOT` so that a German Windows
writes a point, not a comma. Its measuring loop is the `.ns` harness's:
run, count, read `System.nanoTime()`, repeat while the elapsed time is
under the window, answer runs × 10⁹ / elapsed. The flags, their defaults
of 3 and 20 ms and their usage error are the `.ns` harness's; a value
also reads only an optional `-` and ASCII digits, as `Integer parse:`
does, and one whose nanoseconds a `long` cannot hold is refused. An
unknown name throws `Unknown benchmark: <name>`, a malformed command line
the usage text, adjusted to `java AreWeFastYet`; either way the JVM exits
non-zero with the message on stderr. The system property
`-DareWeFastYet.failFirstCheck=true` replaces the first selected benchmark
with one whose check always fails; it is the harness's only test hook.

The `benchmarks` gate runs the Java side in
`crates/ns-cli/tests/run_benchmarks.rs`, beside the ns-next trials, so
the profile `benchmarks-verify` needs a JDK; the default profile runs none
of it, so an ordinary commit does not. `javac` and `java` come from
`$JAVA_HOME/bin` when `JAVA_HOME` is set -- a `JAVA_HOME` without
`bin/javac` fails, naming the path, and never falls back to `PATH` -- and
from `PATH` otherwise, an empty `JAVA_HOME` counting as unset. The trials
run every `java` and `javac` without the environment's JVM option
variables, `JAVA_TOOL_OPTIONS`, `JDK_JAVA_OPTIONS` and `_JAVA_OPTIONS`:
the JVM announces each on stderr, which the trials compare exactly, and
the options would change what Java measures. A missing JDK fails the
trials and says the gate needs one, never skips them, since a gate that
passes without running is no gate. The sources compile with
`javac --release 21 -encoding UTF-8` once per content: into
`<CARGO_TARGET_TMPDIR>/are-we-fast-yet-java/<key>`, the key a hash of the
sorted relative paths and contents of every `.java`, compiled into a
temporary sibling and renamed into place, since nextest runs each trial
as a process of its own, in parallel; a process that finds the directory
uses it, one that loses the rename uses the winner's, and a changed,
renamed or deleted source gives a new key, so no stale class runs. Once
`<key>` is in place, every entry of `are-we-fast-yet-java/` that belongs
to another key, an older key's classes or a leftover temporary directory,
is removed, errors ignored: while the sources stay unchanged during a
run, no process uses another key; editing them mid-run, or two runs at
different states of the sources sharing one target directory, lets a
later process remove an older key's directory that a live process uses,
and can fail the trials of the older state. The key's own temporary
siblings stay, since other trials may still be compiling into them,
unless they are over an hour old: a compilation takes seconds, so such a
sibling is a killed process's, and the age decides without inspecting
processes. A compilation error fails every Java trial with `javac`'s
output. The trials:

- **`java::<Name>`**, for each benchmark: the Java harness run by name,
  held to that benchmark's line of `benchmarks/are-we-fast-yet.toml`
  (`<Name>: ok`), so the `.ns` and Java programs share one record of what
  each must answer.
- **`java-timed::<Name>`**: `--timed <Name>`, its score's shape checked as
  `timed::<Name>` checks ns-next's.
- **`java::unknown-benchmark`**, **`java::a-failing-check-prints-no-ok`**
  (through the test hook) and **`java::missing-jdk-is-named`** (a
  `JAVA_HOME` that holds no JDK is named in the failure, and an empty one
  answers the tool's bare name, for `PATH`).
- **`flags::ns-next::*` and `flags::java::*`**, on both harnesses:
  `windows-print-a-score` and `either-order` run `--timed --warm-up 0
  --measure 1 Sieve` and the flags swapped, and print a score of the right
  shape; `rejected` holds `--warm-up 5` without `--timed`, `--measure x`,
  `--measure -1`, a repeated `--warm-up` and `--measure` without a value
  to a non-zero exit, nothing on stdout and the usage on stderr.

The default windows suit an interpreter; a JIT is still compiling after
3 ms. A peak comparison gives both sides the same longer windows, for
example ten seconds of warm-up, from the repository root, the ns-next side
a release build pinned as the `measuring-performance` skill says:

```bash
javac --release 21 -encoding UTF-8 -d .temp/are-we-fast-yet-java $(find benchmarks/are-we-fast-yet/java -name '*.java')
java -cp .temp/are-we-fast-yet-java AreWeFastYet --timed --warm-up 10000 Richards
ns-next run benchmarks/are-we-fast-yet -- --timed --warm-up 10000 Richards
```

Measure one benchmark per JVM or ns-next invocation, as the example does.
A timed run of all sixteen in one process slows the later ones: they share
the `som` collections' call sites, which the earlier benchmarks have made
megamorphic for the JIT (and, less so, for ns-next's inline caches).

**The baseline**, measured on 2026-10-02 at `1792df5` on one Windows 11
machine (Intel Core Ultra 9 285K): three pinned release builds of the tree,
told apart by `-C metadata=alpha`, `beta` and `gamma`, each checked to
import no `VCRUNTIME140.dll` and to print `ok` for all sixteen. Three
rounds, the builds' order rotated from round to round; in each round every
build ran `bench --iterations 10 -- <Name>` for every benchmark, then the
harness's `--timed` over all sixteen. `bench` gives nine medians per
benchmark (three builds times three rounds); the table pools them by their
median and gives their range. The `--timed` score is the median of nine
scores, in runs per second.

| Benchmark | `bench` median, ms | range of the nine | `--timed`, runs/s |
|---|---|---|---|
| Bounce | 1.48 | 1.45–1.51 | 688.2 |
| CD | 4906 | 4867–5009 | (0.2) |
| DeltaBlue | 895 | 887–942 | (1.1) |
| DeltaBlueTon80 | 8.26 | 8.21–9.29 | 110.2 |
| Havlak | 5249 | 5217–5263 | (0.2) |
| Json | 14.39 | 14.00–14.61 | 70.1 |
| List | 1.26 | 1.26–1.28 | 799.0 |
| Mandelbrot | 4045 | 4021–4057 | (0.2) |
| NBody | 2631 | 2625–2670 | (0.4) |
| Permute | 3.01 | 2.93–3.10 | 331.4 |
| Queens | 2.50 | 2.47–2.55 | 408.5 |
| Richards | 90.2 | 89.8–92.2 | 11.0 |
| RichardsTon80 | 6.14 | 6.06–6.40 | 163.5 |
| Sieve | 1.67 | 1.66–1.70 | 621.5 |
| Storage | 2.01 | 1.98–2.05 | 501.5 |
| Towers | 4.44 | 4.42–4.54 | 228.4 |

The bracketed scores are one run's rate (see above), printed to one
fraction digit, too coarse to compare; for those five `bench` is the
figure. The three builds' medians lie within 3.1% of each other on every
benchmark (Bounce the widest, then Permute at 2.7%; all others within
1.6%), so code layout moves these programs less than it moves
`bench_block.ns`. Within one build the nine values spread further where a
single round stood out: DeltaBlueTon80 at 9.29 ms (one build, round 1,
otherwise 8.21–8.40) and DeltaBlue at 940 ms (two builds, round 1,
otherwise 887–900). What the baseline does not cover: one machine and one
operating system; data layout, which `-C metadata` does not vary (above);
and, for the `bench` figures, what each timed `main:args:` does once per
application -- instantiating the harness and the benchmark module --
which they include and whose share was not measured.

### What the eight-byte value moved

Measured on 2026-10-02 on the machine of the baseline above (Windows 11,
Intel Core Ultra 9 285K): the tree the branch `eight-byte-value` started
from, `2204e8f` (the same code as `1792df5`), against the branch at
`16c6022`, in one sitting. Three pinned release builds of each tree, told
apart by `-C metadata=alpha`, `beta` and `gamma`, each checked to import no
`VCRUNTIME140.dll` and to print the expected output for every source.
Three rounds, the six binaries' order rotated by one from round to round; in
each round every binary ran `bench --iterations 10 -- <Name>` for each of
the sixteen programs, `bench --iterations 30` for the eight microbenchmark
sources (the four fixtures under `benchmarks/micro/` and the four of
"What spec-conformant lookup moved"), and the harness's `--timed` over all
sixteen. Each figure below is the median of a tree's nine values (three
builds times three rounds), with their range.

| Source | Before, ms | Range | After, ms | Range | Change |
|---|---|---|---|---|---|
| Bounce | 1.49 | 1.46–2.03 | 1.09 | 1.07–1.11 | **−26.9%** |
| CD | 4,910 | 4,879–5,024 | 4,234 | 4,192–4,356 | **−13.8%** |
| DeltaBlue | 899 | 889–974 | 741 | 729–776 | **−17.5%** |
| DeltaBlueTon80 | 8.30 | 8.13–8.49 | 6.92 | 6.86–7.09 | **−16.7%** |
| Havlak | 5,293 | 5,162–5,693 | 4,251 | 4,182–4,417 | **−19.7%** |
| Json | 14.3 | 14.1–14.7 | 10.7 | 10.4–10.9 | **−24.9%** |
| List | 1.27 | 1.26–1.28 | 0.99 | 0.98–1.00 | **−22.1%** |
| Mandelbrot | 4,033 | 4,005–4,085 | 3,275 | 3,210–3,360 | **−18.8%** |
| NBody | 2,659 | 2,644–2,690 | 2,339 | 2,319–2,346 | **−12.0%** |
| Permute | 2.98 | 2.92–3.10 | 2.42 | 2.38–2.44 | **−18.8%** |
| Queens | 2.52 | 2.46–2.58 | 2.05 | 2.02–2.07 | **−18.9%** |
| Richards | 91.6 | 90.4–95.1 | 77.8 | 76.5–80.2 | **−15.1%** |
| RichardsTon80 | 6.21 | 6.06–6.68 | 4.57 | 4.46–4.71 | **−26.4%** |
| Sieve | 1.65 | 1.64–1.69 | 1.33 | 1.30–1.39 | **−19.5%** |
| Storage | 2.00 | 1.97–2.08 | 1.62 | 1.60–1.65 | **−18.9%** |
| Towers | 4.44 | 4.29–4.54 | 3.76 | 3.63–3.90 | **−15.3%** |
| `bench_block.ns` | 12.1 | 11.9–12.3 | 10.5 | 10.4–10.6 | **−13.6%** |
| `bench_loop.ns` | 3.60 | 3.52–3.78 | 2.26 | 2.25–2.37 | **−37.1%** |
| `bench_megamorphic.ns` | 7.66 | 7.52–9.63 | 5.32 | 5.29–5.53 | **−30.5%** |
| `bench_send.ns` | 4.94 | 4.81–6.93 | 3.47 | 3.41–3.50 | **−29.8%** |
| `bench_do.ns` | 4.59 | 4.48–4.79 | 3.93 | 3.84–4.02 | **−14.4%** |
| `bench_new.ns` | 3.94 | 3.91–4.10 | 3.31 | 3.27–3.33 | **−16.0%** |
| `bench_new2.ns` | 4.92 | 4.78–5.02 | 4.13 | 4.04–4.25 | **−16.0%** |
| `bench_newarg.ns` | 6.71 | 6.61–6.88 | 5.92 | 5.80–6.00 | **−11.9%** |

Every source got faster, and on every one the two trees' ranges are
disjoint: the sixteen programs by 12% (NBody) to 27% (Bounce), the
microbenchmarks by 12% (`bench_newarg.ns`) to 37% (`bench_loop.ns`). The
largest gains are on the paths the store-forwarding stall sat on ("Where
the 3% goes"): `bench_loop.ns`, whose loop is `IntegerBinary` and
register traffic, and the send-heavy `bench_send.ns` and
`bench_megamorphic.ns`. The three builds of a tree lie within 4.5% of each
other on every source (`bench_send.ns` and `bench_loop.ns` before the
branch the widest), far less than any change. The `--timed` scores agree, for the eleven programs
whose score is a throughput over many runs (the other five, one run's rate
to one fraction digit, are read from `bench` above):

| Benchmark | Before, runs/s | After, runs/s | Change |
|---|---|---|---|
| Bounce | 696.3 | 955.4 | **+37.2%** |
| DeltaBlueTon80 | 109.1 | 130.8 | **+19.9%** |
| Json | 70.0 | 93.7 | **+33.9%** |
| List | 806.7 | 1014.3 | **+25.7%** |
| Permute | 329.3 | 407.0 | **+23.6%** |
| Queens | 403.1 | 488.1 | **+21.1%** |
| Richards | 11.0 | 13.0 | **+18.2%** |
| RichardsTon80 | 161.0 | 217.7 | **+35.2%** |
| Sieve | 615.8 | 760.2 | **+23.4%** |
| Storage | 506.9 | 621.4 | **+22.6%** |
| Towers | 226.1 | 270.3 | **+19.5%** |

`vm::run` is one function of 6,978 instructions at `16c6022`, against the
7,143 last recorded ("What the loop over locals moved"), with no
`vm::run::closure` and no `run_stretch` of its own (`dumpbin /disasm` of a
pinned release build with `CARGO_PROFILE_RELEASE_DEBUG=limited`). The
tagged operations (`Value::small_integer_sum` and its siblings) also exist
as functions of their own, because `builtins/numbers/integer.rs` passes them to its
shared arithmetic as function values; `vm::run` calls none of them.

What the measurement does not cover: one machine and one operating system;
data layout, which `-C metadata` does not vary; for the `bench` figures,
what each timed `main:args:` does once per application, as for the
baseline; and which of the branch's changes -- the word itself, the bare
words in the heap, the tagged arithmetic, passing a value by value -- gave
how much, since only the two ends were measured.

### What the compact instruction encoding moved

Measured on 2026-10-02 on the same machine: the tree the branch
`feature/compact-instruction-encoding` started from, `49e1fcc`, against the
branch at `65ad13a`, in one sitting. Three pinned release builds of each
tree (`-C metadata=alpha`, `beta`, `gamma`), each checked to print `ok` for
all sixteen programs; three rounds, the six binaries' order rotated from
round to round; in each round every binary ran `bench --iterations 3 --
<Name>` for CD, Havlak, Mandelbrot, NBody and DeltaBlue, `--iterations
10` for the other eleven, and `bench --iterations 30` for the eight
microbenchmark sources -- `bench` alone, as a comparison of two trees is
("The Are We Fast Yet benchmarks"). Each figure is the median of a tree's
nine values, with their range; "separated" says the two ranges do not
overlap.

| Source | Before, ms | Range | After, ms | Range | Change | Separated |
|---|---|---|---|---|---|---|
| Bounce | 1.10 | 1.09–1.13 | 1.09 | 1.06–1.20 | −1.4% | no |
| CD | 4,284 | 4,234–4,363 | 4,080 | 4,061–4,150 | **−4.8%** | yes |
| DeltaBlue | 755 | 746–780 | 741 | 721–746 | −1.8% | no |
| DeltaBlueTon80 | 7.13 | 6.97–7.29 | 6.86 | 6.80–7.14 | −3.8% | no |
| Havlak | 4,415 | 4,343–4,480 | 4,216 | 4,164–4,355 | −4.5% | no |
| Json | 10.9 | 10.7–11.1 | 10.5 | 10.4–10.7 | **−3.5%** | yes |
| List | 0.992 | 0.985–1.011 | 0.993 | 0.976–1.007 | +0.2% | no |
| Mandelbrot | 3,345 | 3,330–3,378 | 3,238 | 3,218–3,258 | **−3.2%** | yes |
| NBody | 2,368 | 2,323–2,472 | 2,340 | 2,304–2,388 | −1.2% | no |
| Permute | 2.45 | 2.40–2.48 | 2.46 | 2.38–2.49 | +0.3% | no |
| Queens | 2.07 | 2.03–2.10 | 2.03 | 1.99–2.05 | −2.0% | no |
| Richards | 78.3 | 77.6–79.1 | 77.1 | 76.3–78.8 | −1.5% | no |
| RichardsTon80 | 4.65 | 4.57–4.75 | 4.54 | 4.50–4.76 | −2.3% | no |
| Sieve | 1.33 | 1.32–1.36 | 1.30 | 1.26–1.38 | −2.8% | no |
| Storage | 1.65 | 1.61–1.71 | 1.59 | 1.57–1.64 | −3.2% | no |
| Towers | 3.72 | 3.70–3.77 | 3.60 | 3.57–3.67 | **−3.3%** | yes |
| `bench_block.ns` | 10.65 | 10.51–10.74 | 10.37 | 10.25–10.60 | −2.6% | no |
| `bench_loop.ns` | 2.278 | 2.263–2.298 | 2.235 | 2.225–2.253 | **−1.9%** | yes |
| `bench_megamorphic.ns` | 5.44 | 5.38–5.49 | 5.15 | 5.13–5.17 | **−5.4%** | yes |
| `bench_send.ns` | 3.54 | 3.49–3.69 | 3.54 | 3.49–3.59 | +0.1% | no |
| `bench_do.ns` | 4.00 | 3.99–4.05 | 3.92 | 3.83–4.01 | −2.1% | no |
| `bench_new.ns` | 3.36 | 3.29–3.41 | 3.29 | 3.27–3.33 | −2.2% | no |
| `bench_new2.ns` | 4.25 | 4.20–4.32 | 4.10 | 4.04–4.12 | **−3.5%** | yes |
| `bench_newarg.ns` | 5.97 | 5.87–6.04 | 5.85 | 5.76–5.96 | −2.0% | no |

**Twenty-one of the twenty-four sources are faster, by 1.2% to 5.4%**,
seven of them with separated ranges; List, Permute and `bench_send.ns` move
by +0.1% to +0.3%, inside their ranges. The gain is small beside the
eight-byte value's: the encoding removed the niche decode and the argument
list's heap indirection, not a stall.

**Measured first, then once more on the way.** The gate (Task 1 of [the
plan](../superpowers/plans/2026-10-02-1910-ns-next-compact-instruction-encoding.md))
ran the same protocol on a throwaway copy whose four send kinds met in one
join after the match: thirteen of the sixteen programs got faster by 0.3%
to 6.5%, but `bench_loop.ns` got 3.9% slower, separated, and the branch's
first loop kept that (+3.5%). An `xperf` profile found the join: the four
kinds' pending sends merged into one stack aggregate written with narrow
stores, read back as a byte, bounds-checked, and dispatched through a
second jump table on the kind -- about 8,600 more samples on the send path
than the 5,600 the slot dispatch saved. One join per kind, each send arm
breaking out of its own kind's labelled block, removed it, and turned
`bench_loop.ns` from 3.5% slower into the 1.9% faster above.

`vm::run` is 8,498 instructions with a stack frame of 0x398 bytes at
`51c4c49`, against 7,000 and 0x358 at `49e1fcc` (`dumpbin /disasm`, a
pinned release build with `CARGO_PROFILE_RELEASE_DEBUG=limited`), with no
`vm::run::closure` and no `run_stretch` of its own; `run_wide`, which runs
every instruction that has no eight-byte form, is a function of its own.
The loop grew by the arity forms of the sends and the side-table reads, and
is faster for it.

What the measurement does not cover: one machine and one operating system;
data layout, which `-C metadata` does not vary; for the `bench` figures,
what each timed `main:args:` does once per application; the memory the
encoding saves, which is counted in "The compact instruction encoding"
("The forms compared"), not measured as a working set; and code
that runs wide forms, which no measured program reaches.

### What the object representation moved

Measured on 2026-10-03 on the same machine (Windows 11, Intel Core Ultra 9
285K): the tree the branch `feature/object-representation` started from,
`d0e8b08`, against the branch at `e664c77`, in one sitting. Three pinned
release builds of each tree (`-C metadata=alpha`, `beta`, `gamma`, with `-C
target-feature=+crt-static -C llvm-args=-align-all-functions=6`), each checked
to print `ok` for all sixteen programs and the expected output for the eight
microbenchmark sources; three rounds, the six binaries' order rotated from
round to round; in each round every binary ran `bench --iterations 3 --
<Name>` for CD, Havlak, Mandelbrot, NBody and DeltaBlue, `--iterations 10`
for the other eleven, and `bench --iterations 30` for the eight
microbenchmark sources. Each figure is the median of a tree's nine values,
with their range; "separated" says the two ranges do not overlap. The three
builds of a tree lie within 9.2% of each other on every source (one
`bench_new2.ns` value before the branch the widest).

| Source | Before, ms | Range | After, ms | Range | Change | Separated |
|---|---|---|---|---|---|---|
| Bounce | 1.07 | 1.04–1.12 | 1.13 | 1.10–1.23 | +5.2% | no |
| CD | 4,078 | 4,037–4,122 | 4,583 | 4,562–4,629 | **+12.4%** | yes |
| DeltaBlue | 736 | 723–764 | 781 | 760–801 | +6.1% | no |
| DeltaBlueTon80 | 6.86 | 6.80–7.25 | 7.52 | 7.42–7.83 | **+9.6%** | yes |
| Havlak | 4,344 | 4,177–4,431 | 4,076 | 3,966–4,285 | −6.2% | no |
| Json | 10.28 | 10.21–10.54 | 11.72 | 11.50–11.98 | **+14.0%** | yes |
| List | 0.990 | 0.978–0.999 | 1.02 | 1.00–1.06 | **+3.6%** | yes |
| Mandelbrot | 3,264 | 3,230–3,319 | 3,296 | 3,274–3,335 | +1.0% | no |
| NBody | 2,345 | 2,312–2,368 | 2,454 | 2,413–2,471 | **+4.6%** | yes |
| Permute | 2.38 | 2.31–2.46 | 2.56 | 2.50–2.61 | **+7.6%** | yes |
| Queens | 2.02 | 1.99–2.06 | 2.15 | 2.09–2.22 | **+6.6%** | yes |
| Richards | 77.23 | 75.02–78.59 | 84.41 | 83.10–86.53 | **+9.3%** | yes |
| RichardsTon80 | 4.52 | 4.44–4.59 | 5.13 | 4.98–5.30 | **+13.4%** | yes |
| Sieve | 1.25 | 1.24–1.27 | 1.59 | 1.58–1.62 | **+27.5%** | yes |
| Storage | 1.59 | 1.57–1.67 | 1.91 | 1.87–2.11 | **+20.1%** | yes |
| Towers | 3.60 | 3.57–3.65 | 3.98 | 3.90–4.19 | **+10.6%** | yes |
| `bench_block.ns` | 10.30 | 10.22–10.45 | 13.44 | 13.24–13.65 | **+30.5%** | yes |
| `bench_loop.ns` | 2.18 | 2.15–2.22 | 2.43 | 2.41–2.45 | **+11.7%** | yes |
| `bench_megamorphic.ns` | 5.08 | 5.04–5.14 | 5.41 | 5.38–5.48 | **+6.5%** | yes |
| `bench_send.ns` | 3.38 | 3.31–3.46 | 3.63 | 3.58–3.66 | **+7.6%** | yes |
| `bench_do.ns` | 3.90 | 3.88–4.07 | 4.45 | 4.36–4.54 | **+14.3%** | yes |
| `bench_new.ns` | 3.28 | 3.24–3.29 | 3.48 | 3.45–3.57 | **+6.2%** | yes |
| `bench_new2.ns` | 4.11 | 4.08–5.09 | 4.48 | 4.45–4.57 | +9.1% | no |
| `bench_newarg.ns` | 5.84 | 5.75–5.89 | 6.48 | 6.43–6.58 | **+10.8%** | yes |

**Twenty-three of the twenty-four sources are slower**, nineteen of them with
separated ranges, by 3.6% (List) to 30.5% (`bench_block.ns`); Havlak is 6.2%
faster, inside its range, and the four slower ones that are not separated
(Bounce, DeltaBlue, Mandelbrot, `bench_new2.ns`) move by 1.0% to 9.1%. This is
the interim cost the design expected ([object-model.md](object-model.md), "The object
layout"): every guard and class read went through the slot's pointer to its
block until road step 3b put the objects in the space ("What the object
space moved" below).

**Measured, profiled, measured again.** A first measurement of the branch at
`3cdbd5e`, by the same protocol, showed DeltaBlue +42.0%, `bench_block.ns`
+41.2% and Sieve +38.5%. An `xperf` profile of the three (one trace per
program per tree, 0.1221 ms sampling, one debug-info build each) reproduced
the regressions (DeltaBlue +42.7%, `bench_block.ns` +40.4%, Sieve +31.2%) and
the sample totals moved alike: DeltaBlue 44,390 to 63,076 samples,
`bench_block.ns` 35,116 to 49,313, Sieve 32,439 to 42,297. About 14,500 of
DeltaBlue's 18,686 added samples (78%) were in `Heap::symbol_value`: a
Symbol's primitives (`hash`, `=`, `asString`, `printString`, and the hash of
any object a table hashes) re-derived the interned symbol from the packed
text, unpacking the words into a buffer, converting the code points to a
UTF-8 `String`, hashing it and probing the interner, on every call; the
symbol-keyed lookups of `Strength of: #required` made one per constraint.
A per-heap inverse map from a symbol object's index to its `Symbol`, kept in
step with the canonical symbol table ([memory.md](memory.md), "Weak
references"), made it one map probe (`e664c77`), which took DeltaBlue from
+42.0% to +6.1%.

What the profile found in `bench_block.ns` and Sieve is a heap allocation per
frame block and per closure block on every `MakeClosure` (Sieve's `at:` and
`at:put:` build an `ifFail:` closure per element): the Windows heap's own
exclusive time grew by 9,420 samples in `bench_block.ns` (66% of its added
samples) and 5,622 in Sieve (57%), where the frame and the closure had been
inline in the arena vectors. That is the per-object allocation road step 3b's
bump allocation in the space replaced. The profile also found the temporary
list of copied values every closure built and freed, which `e664c77` removed;
and about 5,800 (`bench_block.ns`) and 3,900 (Sieve) samples in the new
out-of-line accessors (`pointer_words`, `frame_payload`, `register_code`),
the format check through the block's pointer that the same step removed.

What the measurement does not cover: one machine and one operating system;
data layout, which `-C metadata` does not vary; for the `bench` figures, what
each timed `main:args:` does once per application; the profile, which is one
trace per side from one debug-info build each, so layout effects of a few
percent are not excluded, and which could not split code inlined into
`vm::run`; and the decode amplification, which is computed, not measured
([memory.md](memory.md), "The amplification bound of a rejected load").

### What the object space moved

Measured on 2026-10-03 on the same machine (Windows 11, Intel Core Ultra 9
285K), in two sittings: the tree the branch `feature/object-space` started
from, `d3e9f6f` ("before", the arena of road step 3a), against the branch
at three points -- `e187399`, after handles became 64-bit offsets, still on
the arena; `a51ed31`, with the objects in the space and every allocation
written in place; and `5d1b903`, `a51ed31` plus the inlining fix below, the
branch's head. Three pinned release builds of each tree (`-C
metadata=alpha`, `beta`, `gamma`, with `-C target-feature=+crt-static -C
llvm-args=-align-all-functions=6`), built in throwaway worktrees, each
checked to print `ok` for all sixteen programs and the expected output for
the eight microbenchmark sources; three rounds, the binaries' order rotated
from round to round; in each round every binary ran `bench --iterations 3
-- <Name>` for CD, Havlak, Mandelbrot, NBody and DeltaBlue, `--iterations
10` for the other eleven, and `bench --iterations 30` for the eight
microbenchmark sources. Each figure is the median of a tree's nine values,
with their range; "separated" says the two ranges do not overlap.

The second sitting, the branch's head against the tree it started from:

| Source | Before, ms | Range | After, ms | Range | Change | Separated |
|---|---|---|---|---|---|---|
| Bounce | 1.11 | 1.08–1.12 | 1.11 | 1.09–1.12 | +0.1% | no |
| CD | 4,597 | 4,562–4,630 | 4,380 | 4,361–4,545 | **−4.7%** | yes |
| DeltaBlue | 773 | 768–791 | 744 | 739–755 | **−3.8%** | yes |
| DeltaBlueTon80 | 7.50 | 7.44–7.60 | 7.68 | 7.59–7.77 | +2.4% | no |
| Havlak | 4,064 | 3,982–4,261 | 3,889 | 3,864–3,927 | **−4.3%** | yes |
| Json | 11.48 | 11.03–11.73 | 11.38 | 11.13–11.47 | −0.9% | no |
| List | 1.01 | 1.00–1.05 | 1.03 | 1.03–1.05 | +2.2% | no |
| Mandelbrot | 3,323 | 3,297–3,337 | 3,405 | 3,376–3,446 | **+2.5%** | yes |
| NBody | 2,468 | 2,449–2,479 | 2,504 | 2,479–2,818 | **+1.5%** | yes |
| Permute | 2.58 | 2.51–2.62 | 2.50 | 2.49–2.54 | −3.1% | no |
| Queens | 2.14 | 2.07–2.27 | 2.14 | 2.10–2.21 | −0.2% | no |
| Richards | 84.72 | 84.14–88.02 | 84.41 | 83.16–85.80 | −0.4% | no |
| RichardsTon80 | 5.18 | 5.13–5.66 | 4.97 | 4.89–5.18 | −4.0% | no |
| Sieve | 1.49 | 1.47–1.50 | 1.36 | 1.35–1.36 | **−8.7%** | yes |
| Storage | 1.86 | 1.84–1.93 | 1.89 | 1.85–1.90 | +1.7% | no |
| Towers | 3.97 | 3.94–4.20 | 3.64 | 3.58–3.75 | **−8.4%** | yes |
| `bench_block.ns` | 13.45 | 13.34–13.61 | 10.47 | 10.39–10.57 | **−22.1%** | yes |
| `bench_loop.ns` | 2.44 | 2.42–2.47 | 2.49 | 2.48–2.50 | **+1.9%** | yes |
| `bench_megamorphic.ns` | 5.45 | 5.39–5.49 | 5.35 | 5.29–6.74 | −1.8% | no |
| `bench_send.ns` | 3.65 | 3.62–3.70 | 3.63 | 3.59–3.72 | −0.4% | no |
| `bench_do.ns` | 4.45 | 4.38–4.55 | 4.12 | 4.05–4.15 | **−7.4%** | yes |
| `bench_new.ns` | 3.52 | 3.46–3.58 | 3.06 | 3.03–3.07 | **−13.1%** | yes |
| `bench_new2.ns` | 4.43 | 4.38–4.56 | 4.04 | 4.02–4.08 | **−8.9%** | yes |
| `bench_newarg.ns` | 6.47 | 6.41–6.59 | 5.84 | 5.76–5.99 | **−9.8%** | yes |

NBody's ranges print as sharing 2,479 ms; unrounded they are separated by
0.34 ms, the before maximum 2,478.76 ms and the after minimum 2,479.10 ms,
below the table's precision.

**Seventeen of the twenty-four sources are faster, ten of them with
separated ranges**, by 3.8% (DeltaBlue) to 22.1% (`bench_block.ns`); three
are slower with separated ranges -- Mandelbrot by 2.5%, `bench_loop.ns` by
1.9% and NBody by 1.5% -- and the other eleven move by −4.0% to +2.4% inside
their ranges. The gains are where road step 3a's profile put its costs
("What the object representation moved"): the allocating sources --
`bench_block.ns`, which reifies a frame and makes a closure every iteration,
`bench_do.ns` and the three instantiation benchmarks, Sieve, Towers and CD --
no longer make a call into the system allocator per object or drop one per
dead object, and a guard or a field is one load from the space's base rather
than two through a slot's pointer. The three slower programs were not
profiled, so what slows them is not known ([OPEN-WORK.md](../OPEN-WORK.md)).

**The first sitting** measured what each half of the branch did, against the
same base, before the inlining fix:

| Source | Before, ms | Range | Handle, ms | Range | Space, ms | Range | Handle vs before | Space vs before |
|---|---|---|---|---|---|---|---|---|
| Bounce | 1.10 | 1.09–1.18 | 1.15 | 1.14–1.16 | 1.12 | 1.11–1.14 | +4.2% | +2.3% |
| CD | 4,588 | 4,489–4,635 | 4,785 | 4,677–4,873 | 4,493 | 4,438–4,626 | **+4.3%** | −2.1% |
| DeltaBlue | 770 | 751–800 | 792 | 780–831 | 769 | 753–772 | +2.8% | −0.2% |
| DeltaBlueTon80 | 7.51 | 7.34–7.60 | 7.81 | 7.70–7.95 | 7.91 | 7.75–8.08 | **+4.0%** | **+5.3%** |
| Havlak | 4,014 | 3,857–4,050 | 4,175 | 4,012–4,476 | 3,967 | 3,862–4,149 | +4.0% | −1.2% |
| Json | 11.45 | 11.09–11.66 | 11.78 | 11.62–12.03 | 11.61 | 11.46–11.66 | +2.9% | +1.4% |
| List | 1.01 | 0.99–1.06 | 1.04 | 1.03–1.07 | 1.08 | 1.06–1.08 | +2.6% | **+6.1%** |
| Mandelbrot | 3,310 | 3,259–3,367 | 3,340 | 3,283–3,391 | 3,402 | 3,358–3,422 | +0.9% | +2.8% |
| NBody | 2,471 | 2,418–2,506 | 2,553 | 2,521–2,590 | 2,506 | 2,480–2,530 | **+3.3%** | +1.4% |
| Permute | 2.53 | 2.51–2.62 | 2.62 | 2.56–2.68 | 2.53 | 2.46–2.56 | +3.5% | −0.2% |
| Queens | 2.11 | 2.08–2.23 | 2.21 | 2.13–2.29 | 2.15 | 2.10–2.20 | +4.5% | +2.1% |
| Richards | 84.69 | 82.77–85.37 | 87.09 | 85.61–89.83 | 87.99 | 87.68–89.27 | **+2.8%** | **+3.9%** |
| RichardsTon80 | 5.18 | 5.15–5.31 | 5.38 | 5.35–5.47 | 5.03 | 4.95–5.14 | **+4.0%** | **−2.9%** |
| Sieve | 1.48 | 1.48–1.54 | 1.53 | 1.50–1.56 | 1.36 | 1.36–1.38 | +3.1% | **−8.1%** |
| Storage | 1.86 | 1.83–1.96 | 1.98 | 1.95–2.14 | 1.88 | 1.86–1.91 | +6.6% | +1.1% |
| Towers | 4.00 | 3.89–4.18 | 4.08 | 3.98–4.26 | 3.66 | 3.62–3.74 | +1.8% | **−8.5%** |
| `bench_block.ns` | 13.47 | 13.35–13.83 | 13.71 | 13.61–14.19 | 10.56 | 10.40–10.68 | +1.8% | **−21.6%** |
| `bench_loop.ns` | 2.43 | 2.42–2.48 | 2.67 | 2.66–2.71 | 2.53 | 2.51–2.57 | **+9.9%** | **+4.0%** |
| `bench_megamorphic.ns` | 5.45 | 5.36–5.52 | 5.66 | 5.62–5.71 | 5.39 | 5.38–5.43 | **+3.8%** | −1.2% |
| `bench_send.ns` | 3.64 | 3.59–3.70 | 3.81 | 3.77–3.89 | 3.69 | 3.63–4.48 | **+4.5%** | +1.2% |
| `bench_do.ns` | 4.40 | 4.37–4.61 | 4.54 | 4.45–4.76 | 4.18 | 4.15–4.35 | +3.3% | **−5.0%** |
| `bench_new.ns` | 3.48 | 3.44–3.57 | 3.63 | 3.61–3.73 | 3.30 | 3.27–3.31 | **+4.3%** | **−5.3%** |
| `bench_new2.ns` | 4.43 | 4.34–5.37 | 4.69 | 4.63–4.72 | 4.36 | 4.32–4.39 | +5.7% | −1.8% |
| `bench_newarg.ns` | 6.52 | 6.42–6.75 | 6.82 | 6.75–6.94 | 6.00 | 5.96–6.14 | +4.5% | **−8.0%** |

Bold marks a change whose ranges are separated from the base's.
**The 64-bit handle alone made every source slower**, by 0.9% (Mandelbrot) to
9.9% (`bench_loop.ns`), nine of them with separated ranges: every handle a
Rust structure holds doubled from 4 bytes to 8, and every resolution a send
returns, `Resolved`, grew from 16 bytes to 24 in a release build
(`a_resolution_stays_the_size_the_hot_path_was_measured_at`,
`interpreter/resolve.rs`); how the cost divides among them was not
measured. The space at `a51ed31` won the allocating sources back and more,
but left four sources slower with separated ranges -- List by 6.1%,
DeltaBlueTon80 by 5.3%, `bench_loop.ns` by 4.0% and Richards by 3.9%.

**Profiled between the sittings.** An `xperf` profile (0.1221 ms sampling,
one debug-info build per tree, one trace per program) of List,
DeltaBlueTon80 and `bench_loop.ns` found `Heap::pointer_words` an outlined
function at `a51ed31`, holding 5.3% (List), 8.2% (DeltaBlueTon80) and 5.7%
(`bench_loop.ns`, plus 2.8% in `pointer_words_mut`) of the process's
samples, where at the base it was inlined into its callers. The space had
made `Heap::block` larger -- through `Space::words`, `Reservation::prefix`
and `block_range` -- and `#[inline]` no longer sufficed. `5d1b903` marks
`pointer_words`, `pointer_words_mut` and `pointer_word` `#[inline(always)]`
and `Reservation::prefix`, `prefix_mut` and `check_prefix` `#[inline]`;
`dumpbin /disasm` of a debug-info build shows none of them as a function of
its own. The second sitting above measured the branch with it: of the four,
`bench_loop.ns` stays slower by 1.9%, separated; List, DeltaBlueTon80 and
Richards no longer separate from the base.

**Memory.** Peak working set (`PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize`
through `GetProcessMemoryInfo` on the still-open handle, as in
[memory.md](memory.md), "The interim cost, and the one question still
open"), release, one build a side:

| Command | Before | After |
|---|---|---|
| `bench --iterations 10` `bench_loop.ns` | 6.7 MB | 6.7 MB |
| `bench --iterations 10` `bench_send.ns` | 6.7 MB | 6.7 MB |
| `bench --iterations 10` `bench_block.ns` | 7.5 MB | 7.3 MB |
| `bench --iterations 1 -- Havlak` | 41.9 MB | 30.2 MB |
| `bench --iterations 1 -- CD` | 18.3 MB | 16.3 MB |
| `bench --iterations 1 -- Json` | 16.8 MB | 16.3 MB |

Havlak, whose live set is the largest of these, peaks 28% lower, which is
consistent with an object no longer carrying a 16-byte slot and the Rust
allocator's own overhead beside its words; that was not decomposed. `memory.md`'s older table measured `bench_loop.ns` at
4.6-4.8 MB on an older base; the base here measures 6.7 MB the same way, so
compare within this table only. Under `--collector-statistics` and `bench
--iterations 10`, `bench_loop.ns` and `bench_send.ns` make no collection on
either tree -- the 512 KiB floor keeps them out of the collector -- and
`bench_block.ns` makes 229 collections, 12.0 ms of pause in all and 0.095
ms at the longest, against 410, 17.1 ms and 0.158 ms before.

What the measurement does not cover: one machine and one operating system;
data layout, which `-C metadata` does not vary; for the `bench` figures,
what each timed `main:args:` does once per application; why Mandelbrot,
NBody and `bench_loop.ns` are slower, since none of the three was profiled
after the fix; the profile, one trace per side from one debug-info build
each; the working set of one build a side, measured once; and the commit
charge, which the working set does not show.

### What generations moved

Measured on 2026-10-04 on the same machine (Windows 11 Pro 22631, Intel
Core Ultra 9 285K, 24 cores): `master` at `664f5f07`, the tree road step 4
started from ("before", one space under the mark-compact collector),
against `feature/generations` at `24a4e0e0` ("after", the nursery, the
scavenger and the barrier; [memory.md](memory.md), "Generations"). Three
pinned release builds of each tree (`-C metadata=alpha`, `beta`, `gamma`,
with `-C target-feature=+crt-static -C llvm-args=-align-all-functions=6`),
built in throwaway worktrees, none importing `VCRUNTIME140.dll`, each
checked to print `ok` for all sixteen programs and the expected output for
the eight microbenchmark sources. Every program was timed with `bench`
alone: `--iterations 3` for CD, Havlak, Mandelbrot, NBody and DeltaBlue,
`--iterations 10` for the other eleven and for the eight microbenchmark
sources. Two sittings of three rounds, the six binaries' order rotated by
one from round to round, so each figure is the median of a tree's eighteen
values (three builds, three rounds, two sittings), with their range;
"separated" says the two ranges do not overlap. The machine was not idle:
an IDE with its language server, a monitoring tool and the virus scanner
ran beside it, at about a quarter of one core of twenty-four.

| Source | Before, ms | Range | After, ms | Range | Change | Separated |
|---|---|---|---|---|---|---|
| Bounce | 1.12 | 1.09–1.28 | 1.11 | 1.09–1.16 | −0.6% | no |
| CD | 4,305 | 4,281–4,397 | 3,947 | 3,924–3,996 | **−8.3%** | yes |
| DeltaBlue | 731 | 727–741 | 684 | 671–725 | **−6.5%** | yes |
| DeltaBlueTon80 | 7.55 | 7.48–7.74 | 7.67 | 7.55–8.30 | +1.5% | no |
| Havlak | 3,787 | 3,743–3,813 | 3,495 | 3,460–3,553 | **−7.7%** | yes |
| Json | 11.3 | 11.2–11.4 | 11.3 | 11.2–11.3 | 0.0% | no |
| List | 1.04 | 1.02–1.09 | 1.04 | 1.02–1.13 | +0.1% | no |
| Mandelbrot | 3,330 | 3,319–3,425 | 3,356 | 3,321–3,389 | +0.8% | no |
| NBody | 2,460 | 2,445–2,479 | 2,439 | 2,413–2,452 | −0.9% | no |
| Permute | 2.49 | 2.45–2.57 | 2.60 | 2.55–2.66 | +4.5% | no (yes in sitting 1) |
| Queens | 2.15 | 2.11–2.25 | 2.16 | 2.13–2.20 | +0.7% | no |
| Richards | 83.6 | 82.1–85.1 | 83.1 | 81.8–84.3 | −0.6% | no |
| RichardsTon80 | 4.92 | 4.83–5.05 | 5.07 | 5.00–5.18 | +3.1% | no (yes in sitting 1) |
| Sieve | 1.37 | 1.36–1.39 | 1.46 | 1.44–1.50 | **+6.8%** | yes |
| Storage | 1.88 | 1.85–1.92 | 1.75 | 1.72–1.84 | **−6.9%** | yes |
| Towers | 3.65 | 3.57–3.73 | 3.46 | 3.38–3.54 | **−5.1%** | yes |
| `bench_loop.ns` | 2.49 | 2.48–2.50 | 2.49 | 2.47–2.64 | −0.1% | no |
| `bench_send.ns` | 3.64 | 3.60–3.69 | 3.65 | 3.58–3.83 | +0.2% | no |
| `bench_block.ns` | 10.4 | 10.2–10.9 | 9.37 | 9.27–9.95 | **−9.9%** | yes |
| `bench_megamorphic.ns` | 5.34 | 5.23–5.39 | 5.36 | 5.27–5.44 | +0.4% | no |
| `bench_do.ns` | 4.09 | 4.02–4.20 | 4.21 | 4.14–4.44 | +2.8% | no |
| `bench_new.ns` | 3.07 | 3.03–3.57 | 3.14 | 3.09–3.20 | +2.3% | no |
| `bench_new2.ns` | 4.05 | 3.99–4.24 | 4.20 | 4.14–4.27 | +3.7% | no |
| `bench_newarg.ns` | 5.83 | 5.78–5.92 | 5.91 | 5.83–6.03 | +1.5% | no |

**Six sources are faster with separated ranges**, by 5.1% (Towers) to 9.9%
(`bench_block.ns`), each by about the same in both sittings: the two
seconds-long allocating benchmarks CD and Havlak, DeltaBlue, Storage,
Towers and `bench_block.ns`. **Sieve is the one source slower with
separated ranges**, by 6.8%; Permute (+4.5%) and RichardsTon80 (+3.1%) are
slower in both sittings and separated in the first; the four allocating
microbenchmarks are 1.5% to 3.7% slower inside their ranges; the rest are
level. The three after-builds lie within 2.6% of each other on every
source.

**The full collections fell on every source.** `--collector-statistics`
over the same iteration counts, discarded warm-up run included, alpha build
of each tree; before has full collections only:

| Source | Before: collections, total / longest pause (ms) | After: scavenges, total / longest (ms) | After: full collections, total / longest (ms) |
|---|---|---|---|
| CD | 18,844, 1,715.7 / 0.279 | 913, 44.6 / 0.149 | 0 |
| DeltaBlue | 183, 229.6 / 3.397 | 52, 20.0 / 1.796 | 9, 18.2 / 3.126 |
| DeltaBlueTon80 | 15, 1.03 / 0.090 | 0 | 0 |
| Havlak | 780, 1,880.0 / 6.588 | 412, 268.1 / 1.841 | 85, 378.4 / 6.302 |
| Json | 10, 0.83 / 0.088 | 0 | 0 |
| Mandelbrot | 1, 0.10 / 0.102 | 0 | 0 |
| NBody | 2,649, 115.5 / 0.153 | 172, 0.37 / 0.024 | 0 |
| Permute | 16, 0.74 / 0.061 | 1, 0.021 | 0 |
| Queens | 4, 0.22 / 0.066 | 0 | 0 |
| Richards | 283, 13.8 / 0.062 | 18, 0.16 / 0.025 | 0 |
| RichardsTon80 | 21, 0.99 / 0.065 | 1, 0.022 | 0 |
| Sieve | 28, 1.27 / 0.057 | 1, 0.039 | 0 |
| Storage | 20, 2.96 / 0.217 | 0 | 0 |
| Towers | 50, 2.43 / 0.071 | 3, 0.041 / 0.025 | 0 |
| `bench_block.ns` | 229, 11.9 / 0.074 | 15, 0.052 / 0.017 | 0 |
| `bench_do.ns` | 14, 0.87 / 0.102 | 0 | 0 |
| `bench_new.ns` | 10, 0.56 / 0.064 | 0 | 0 |
| `bench_new2.ns` | 13, 0.57 / 0.060 | 0 | 0 |
| `bench_newarg.ns` | 23, 2.18 / 0.181 | 1, 0.022 | 0 |

Bounce, List, `bench_loop.ns`, `bench_send.ns` and `bench_megamorphic.ns`
collect in neither tree. Only Havlak (780 to 85) and DeltaBlue (183 to 9)
still reach a full collection; Havlak's collector time falls from 1,880 ms
to 646 ms, and it is the one program whose scavenges are expensive, 0.65 ms
each and 1.84 ms at the longest, so its survivors are many. The longest
full-collection pause is unchanged (Havlak 6.6 to 6.3 ms, DeltaBlue 3.4 to
3.1 ms); the longest scavenge pause is 1.8 ms. Whether Havlak's remaining
full collections come from survivors tenured early -- a full survivor space
or the adaptive threshold -- or from a graph that is long-lived and rightly
tenured, the statistics line cannot say: it reports no promoted bytes, no
survivor occupancy and no threshold ([OPEN-WORK.md](../OPEN-WORK.md)). Its
live-object count is not comparable across the trees either: after, it
counts every object lying in the space, eden's dead ones not yet scavenged
included (`bench_new.ns` reports 220,116 against 4,176).

**Where the gains come from.** An `xperf` profile (0.1221 ms sampling, one
debug-info release build per tree, one trace per program) puts the gain in
the full collector the sources no longer run: `bench_block.ns`'s collector
samples fall from 10.2% to 0.0% and CD's from 9.5% to 0.2%, all of it
scavenging, with `vm::run`, `push_activation` and the rest within about a
point of each other on those two; Havlak's from
11.7% to 4.0% (1.4% of all samples in the scavenger) and DeltaBlue's from
7.4% to 1.1% (0.5%). The per-iteration closures and frames now die young.

**The barrier costs 0.1% to 0.3% of samples.** `write_barrier` is forced
inline, so its compare sequence -- holder old, word young, the tag test, a
call or tail jump to `remember` -- was located by hand in each store
accessor's disassembly: about eight instructions after the store in
`Heap::set_pointer_word`, `frame_set_local` and `object_set_class`, which
stay out-of-line functions in both trees, and one more push/pop pair in
`set_pointer_word` (114 instructions against 101). `vm::run` is 7,971
instructions in both trees. Over the profiles of DeltaBlue, Richards,
Havlak, CD, Sieve and `bench_block.ns`, the barrier's compares hold 0.08%
to 0.28% of the samples and `remember` at most 0.02%; `set_pointer_word`
as a whole gained 0.1 to 0.6 points, and on no profile is the barrier among
the fifteen hottest functions.

**The short allocating programs pay eden's first touch.** Sieve, Permute,
RichardsTon80 and the four allocating microbenchmarks scavenge at most once
in a `bench --iterations 10`, so nearly every allocation lands on a page of
eden the process has never touched, where the before tree reused its small
old space, resident and warm, at the price of 16 to 28 collections. Four
measurements say the cost is the first touch, not the barrier and not the
scavenger:

- **Peak working set** (`PeakWorkingSetSize` and `PageFaultCount` through
  `GetProcessMemoryInfo` on the still-open handle, all six builds, within
  0.1 MB of each other): `bench_loop.ns` and `bench_send.ns`, which
  allocate nothing per iteration, stay at 6.4–6.5 MB and about 1,800 page
  faults in both trees, so the nursery's commit costs nothing untouched.
  `bench --iterations 10` of `bench_block.ns` rises from 7.0 MB and 1,939
  faults to 13.9–14.0 MB and 3,729–3,741, about 1,800 faults more, or 7 MiB
  of 4 KiB pages; Sieve, Permute, RichardsTon80 and Storage rise from
  14.9–15.0 MB to 21.5–22.7 MB, `bench_new2.ns` and `bench_do.ns` from
  7.0 MB to 13.1–14.0 MB.
- **Kernel time.** Sixty processes of `bench --iterations 10` each under
  `xperf`, aggregated: Sieve's kernel samples inside `ns-next` rise from
  2,496 to 3,401 and Permute's from 2,523 to 3,450 -- page faults and the
  zeroing of demand-zero pages, the `System` process's samples rising with
  them -- while their collector samples fall from 709 to 11 and from 445
  to 3 and the rest is level. Sieve's net, +654 samples over sixty
  processes, is about 0.12 ms a run against the 0.09–0.10 ms measured.
- **The barrier** holds 0.14% (Sieve) and 0.03% (Permute) of those
  samples, and `remember` none.
- **Once eden has been cycled the regression is gone.** At
  `--iterations 200`, one build a side, every one of them is faster after:
  Sieve 1.356 to 1.268 ms (−6.5%), Permute 2.477 to 2.423 ms (−2.2%),
  RichardsTon80 4.99 to 4.85 ms (−2.8%), `bench_new2.ns` 4.01 to 3.96 ms
  (−1.3%), `bench_do.ns` 4.04 to 3.98 ms (−1.5%).

So a program that allocates a few megabytes and stops pays once per process
for faulting in up to 8 MiB of eden, about 2,000 soft faults and their
zeroing, 1.5 to 2 ms on this machine, which a 1–5 ms program under `bench
--iterations 10` shows as 3% to 7%; the full collections it no longer runs
repay it only once it has cycled eden. A smaller eden or one pre-faulted
when the heap is made would trade it differently, and neither was measured
([OPEN-WORK.md](../OPEN-WORK.md)). Sieve's 5,000-element array, 40 KB, is
allocated old under the 32 KiB rule, so a long Sieve run still collects in
full, less often than before: over 1,500 runs `World::collect` holds 3.6%
of the after tree's samples against 8.1% of the before tree's.

What the measurement does not cover: one machine and one operating system;
data layout, which `-C metadata` does not vary; one profile trace per
program and tree; the background load above; the second sitting repeated
the first's three build orders rather than continuing the rotation; and
nothing measured argues for or against eden's 512 KiB reserve or the 32 KiB
large-object limit.

### What inline allocation moved

Measured on 2026-10-22 on the same machine (Windows 11 Pro 22631, Intel
Core Ultra 9 285K, 24 cores, rustc 1.98.0) by the protocol of "What
removing the re-entries moved": `master` at `dc7ae2f6`, the tree road
step 6 started from, against each of its five parts in order -- bump
(`af894802`, the fast bump before the cold `allocate_block`), codeid
(`cfadf647`, the code-id cache), templates (`8555b814`, closures, frames
and boxed floats with a precomputed header, and the tests that prove the
fast bump ran), arrays (`df7d55c9`, `Array new:`, `MakeArray` and
`MakeVector` with no `Vec`) and new (`fee106bb`, `new` of a class with no
initializer code; [memory.md](memory.md), "Inline allocation";
[object-model.md](object-model.md), "Instantiation"). Three pinned
release builds of each tree (`-C metadata=alpha`, `beta`, `gamma`, with
`-C target-feature=+crt-static -C llvm-args=-align-all-functions=6`),
`cargo xtask shipped-code` before each, in throwaway worktrees; all 18
binaries distinct, none importing `VCRUNTIME140.dll`. Every program timed
with `bench` alone: `--iterations 3` for CD, Havlak, Mandelbrot, NBody
and DeltaBlue, `--iterations 10` for the other eleven and the eight
microbenchmark sources, the four not in the tree written out verbatim
from "What spec-conformant lookup moved". `master` and new ran all 24
sources; the four trees between them the eight microbenchmark sources and
CD, Havlak, DeltaBlue, Richards and NBody. Two sittings of three rounds,
the 18 binaries interleaved by build in the first sitting and in the
reverse order in the second, rotated by one from round to round; each
figure is the median of a tree's 18 values (three builds, three rounds,
two sittings), with their range. Every one of the 1,800 runs printed its
expected output. The machine was not idle (an IDE and background tools,
as before).

`master` against the whole step; "separated" says whether the 18-value
ranges, and the three per-build medians, of the two trees do not overlap:

| Source | `master`, ms | Range | Step 6, ms | Range | Change | Separated: 18 values / build medians |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 2.624 | 2.533–2.670 | 2.352 | 2.337–2.451 | **−10.4%** | yes / yes |
| `bench_send.ns` | 3.956 | 3.855–4.484 | 3.746 | 3.664–3.850 | **−5.3%** | yes / yes |
| `bench_block.ns` | 8.756 | 8.613–9.366 | 7.638 | 7.500–8.095 | **−12.8%** | yes / yes |
| `bench_megamorphic.ns` | 5.425 | 5.333–5.608 | 5.199 | 5.148–5.397 | −4.2% | no / yes |
| `bench_new.ns` | 3.150 | 3.089–3.259 | 3.042 | 2.972–3.122 | −3.4% | no / yes |
| `bench_new2.ns` | 4.199 | 4.063–4.836 | 4.028 | 3.912–4.149 | −4.1% | no / yes |
| `bench_newarg.ns` | 5.144 | 5.074–5.447 | 4.905 | 4.822–5.123 | −4.6% | no / yes |
| `bench_do.ns` | 2.982 | 2.912–3.079 | 2.802 | 2.730–2.918 | −6.0% | no / yes |
| Bounce | 1.003 | 0.977–1.038 | 0.961 | 0.940–1.009 | −4.2% | no / yes |
| CD | 3,858 | 3,843–3,876 | 3,687 | 3,656–3,909 | −4.5% | no / yes |
| DeltaBlue | 699.4 | 693.7–709.0 | 673.3 | 665.0–691.1 | **−3.7%** | yes / yes |
| DeltaBlueTon80 | 7.592 | 7.513–8.352 | 7.423 | 7.320–7.739 | −2.2% | no / no |
| Havlak | 3,593 | 3,566–3,630 | 3,476 | 3,450–3,541 | **−3.3%** | yes / yes |
| Json | 11.25 | 11.13–13.69 | 10.94 | 10.82–11.76 | −2.8% | no / yes |
| List | 1.071 | 1.030–1.097 | 1.001 | 0.989–1.060 | −6.5% | no / yes |
| Mandelbrot | 3,109 | 3,094–3,129 | 3,034 | 3,005–3,081 | **−2.4%** | yes / yes |
| NBody | 2,342 | 2,327–2,367 | 2,286 | 2,270–2,314 | **−2.4%** | yes / yes |
| Permute | 2.588 | 2.544–2.701 | 2.487 | 2.466–2.657 | −3.9% | no / yes |
| Queens | 2.149 | 2.115–2.234 | 2.097 | 2.056–2.237 | −2.4% | no / yes |
| Richards | 72.89 | 71.37–74.21 | 70.53 | 69.63–72.91 | −3.2% | no / yes |
| RichardsTon80 | 4.922 | 4.854–5.076 | 4.726 | 4.636–4.903 | −4.0% | no / yes |
| Sieve | 1.411 | 1.391–1.795 | 1.335 | 1.296–1.409 | −5.4% | no / yes |
| Storage | 1.578 | 1.529–1.819 | 1.481 | 1.443–1.576 | −6.2% | no / yes |
| Towers | 3.574 | 3.457–3.836 | 3.428 | 3.264–3.587 | −4.1% | no / yes |

**Every one of the 24 sources is faster**, by 2.2% (DeltaBlueTon80) to
12.8% (`bench_block.ns`); seven with the 18-value ranges separated, in
bold, and 23 of the 24 with the per-build medians separated -- all but
DeltaBlueTon80, whose beta build at 7.615 ms lies above `master`'s
fastest build. CD's 18 values lie below `master`'s lowest but one, at
3,909 ms.

Each part's own share, the change of the pooled median against the tree
before it; bold where the 18-value ranges are separated:

| Source | bump | codeid | templates | arrays | new | `master` to new |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | **−9.0%** | −1.6% | +0.8% | +0.1% | −0.8% | −10.4% |
| `bench_send.ns` | −5.7% | −0.9% | +0.7% | +0.4% | +0.2% | −5.3% |
| `bench_block.ns` | **−3.9%** | −2.0% | −4.4% | −2.3% | −0.9% | −12.8% |
| `bench_megamorphic.ns` | −4.2% | +0.2% | +0.2% | −0.2% | −0.1% | −4.2% |
| `bench_new.ns` | −3.0% | +0.5% | +0.1% | −0.6% | −0.5% | −3.4% |
| `bench_new2.ns` | −2.6% | −0.7% | +1.2% | −0.9% | −1.1% | −4.1% |
| `bench_newarg.ns` | −1.5% | +0.9% | +0.9% | −4.3% | −0.8% | −4.6% |
| `bench_do.ns` | −3.3% | −0.7% | +1.9% | −4.0% | −0.0% | −6.0% |
| CD | **−2.3%** | −0.5% | −1.0% | −0.7% | −0.1% | −4.5% |
| Havlak | −2.2% | +0.0% | −0.9% | −0.8% | +0.5% | −3.3% |
| DeltaBlue | −2.9% | −0.7% | −0.1% | −0.8% | +0.8% | −3.7% |
| Richards | −3.1% | −0.1% | −0.0% | −1.1% | +1.1% | −3.2% |
| NBody | **−3.3%** | +0.0% | −1.3% | +1.7% | +0.5% | −2.4% |

**The bump pays most**, 1.5% to 9.0% on every source on its own, the
per-build medians separated on all thirteen. The code-id cache moves
`bench_loop.ns`, `bench_block.ns` and DeltaBlue by 0.7% to 2.0%, the
precomputed headers `bench_block.ns` by 4.4% and CD, Havlak and NBody by
about 1%, the arrays `bench_newarg.ns`, `bench_do.ns` and `bench_block.ns`
by 2.3% to 4.3%, each with the per-build medians separated.

**No part is slower than the tree before it with the 18-value ranges
separated.** Slower with the per-build medians separated, and in both
sittings: templates on `bench_send.ns` (+0.7%) and `bench_new2.ns`
(+1.2%), codeid on `bench_newarg.ns` (+0.9%), **arrays on NBody (+1.7%)**,
the largest, which gives back half of the bump's NBody gain, and **new on
Richards (+1.1%), DeltaBlue (+0.8%), NBody (+0.5%) and Havlak (+0.5%)**.
On the microbenchmark sources new is about level (−1.1% to +0.2%): the
classes of `bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` all
declare slots with initializers, so their `new` still walks. Each of these lies
inside the 18-value ranges and at or below the 1–3% that pinned builds
of one tree still differ by on other sources, and neither the arrays nor
the fast `new` obviously touches NBody's, Richards' or DeltaBlue's hot
paths, so code or data layout is a likelier cause than the work; this
measurement cannot decide it. All five parts were kept (decided with the
user on 2026-10-22); the residuals are measured again once `new` runs as
compiled code, which replaced the fast `new` path; that measurement is
"What `new` as compiled code moved" below.

What the measurement does not cover: one machine and one operating
system, not idle; data layout (symbol ids, object offsets, dispatch ids),
which `-C metadata` does not vary, so a difference that follows from it
survives the pool; the eleven Are We Fast Yet programs measured only at
`master` and at the end, whose changes are not known part by part;
`--timed` and the collector statistics; and the `dumpbin /disasm` check
of `vm::run` and `run_stretch`, not made, since no part changes the send
path's code on purpose.

### What a send costs, and where it goes

Milestone 4's design rests on a decomposition rather than on the aggregate. The
decomposition below was measured on the pre-arena `Rc<RefCell<_>>`
representation, and it is what chose the milestone's order. "What the activation
stack moved" below re-takes it against the arena and against the stack, in one
sitting; "What the arena moved" re-measures the aggregate only.
Three separate fixtures, shaped like `bench_loop.ns` but not it -- each with
the body `[n < 50000] whileTrue: [<first statement>. n:: n + 1]` -- isolate
the pieces, because each differs from the next by exactly one send. Row
one's `sum:: sum + 1` reads a literal where `bench_loop.ns` itself reads
the variable `n` (`sum:: sum + n`), so its 19.02 ms is a separate
measurement that happens to land close to `bench_loop.ns`'s own ~19.6 ms
at the time, not the same run reported twice:

| First statement | Method it calls | bytecode median | ns/iteration |
|---|---|---|---|
| `sum:: sum + 1` | — | 19.02 ms | 380.5 |
| `sum:: self tick: sum` | `public tick: x = (^x)` | 22.44 ms | 448.7 |
| `sum:: self tick: sum` | `public tick: x = (^x + 1)` | 27.28 ms | 545.7 |

Rows one and three each pay three native sends per iteration; row two pays two,
because its method body has no `+`. So the third minus the second is one native
`+` send: **~97 ns**. The third minus the first is exactly one user-method send,
both paying three native sends: **~165 ns**. The second minus the first, 68 ns,
is then what a user-method send costs beyond a native one. It is the difference
of the other two by construction, so it is not an independent check on them.

So **building an activation cost about 68 ns, roughly 41% of a user-method
send**, and it was a fixed cost: a two-argument send measured the same as a
one-argument one. What was in it was the representation, not the work: heap
allocations (the `Rc<RefCell<Frame>>` block and the `arguments` vector `collect`
builds), reference-count traffic on `sender`, `method`, `bindings` and the
defining class, and a `HashMap` lookup per invocation just to find the compiled
body. A method with temporaries also allocated a `temporaries` vector. Row two's
`^x` has none (`ns-next-analyzer disasm` gives `tick:` one register, its argument, where
row three's has three), so its `temporaries` vector stayed empty and never allocated,
and row three's allocation landed in the ~97 ns attributed to `+`. On top of
that, every register read and write in the VM took a `RefCell` borrow, which
was spread across the straight-line instructions of all three rows rather than
isolated in any one of them.

Two caveats. `U - N` is an incremental difference, not a profiler attribution:
the native path pays a `match selector.as_str()` string comparison chain the
user path does not. And these are medians of a whole-program benchmark.

### What the arena moved

Replacing `Rc<RefCell<_>>` with an arena of 32-bit handles, for objects and
activations together (`docs/design/memory.md`, "The representation"), was
measured before and after in one sitting: the pre-arena binary was built from
this branch's merge-base into a throwaway worktree, and both binaries ran the
same `ns-next bench benchmarks/micro/bench_loop.ns` — ten timed runs after a
discarded warm-up — three times each, alternating.

| Run | Before, bytecode | After, bytecode |
|---|---|---|
| 1 | 19.38 ms | 16.45 ms |
| 2 | 19.54 ms | 16.82 ms |
| 3 | 19.60 ms | 16.53 ms |

**The VM gained about 15%**, from ~19.5 ms to ~16.5 ms. The three
before-figures and the three after-figures do not overlap, and the split is
larger than either group's own spread, so this is a real move rather than noise.

What moved is precisely two of the itemized costs above: the `Rc` box and its
reference-count traffic on `sender`, `home` and every shared handle, and the
`RefCell` borrow on every register read and write — which is why the gain is
spread across the straight-line instructions rather than concentrated in sends.

What did **not** move was the rest of the activation cost. `Frame.arguments` and
`Frame.temporaries` were still a `Vec` allocation apiece on every activation that had
arguments or temporaries, the `HashMap` lookup per invocation that finds the
compiled body survived, and building an activation was still an allocation rather
than an index bump. The activation stack took the two vectors and the allocation;
see "What the activation stack moved" below. The lookup went when compiled code
moved onto the declaration it belongs to (see "The VM").

### A 13% regression, found and recovered

Routing every `Object`/`Frame` access through `Heap` (`docs/design/memory.md`,
"The seam") was designed to cost nothing: `--release` inlines an accessor call
away, and each one does the same borrow-clone-drop the call site did before.
Migrating the lexical-send helper (`dispatch_lexical_send`, now
`sends::decide_lexical_send`) broke that measurably: `bench_loop.ns` went from
20.07 ms to 22.77 ms, about 13% (13.45%).

The cause was `dispatch_lexical_send`'s head, which every implicit-receiver
send runs. Before the migration it took one `RefCell` borrow and read both
the receiver and the current class out of it; the migration replaced that
with two separate accessor calls, each its own borrow. `bench_loop.ns` sends
roughly 250,000 lexical sends per run, so the extra borrow pair cost on the
order of 10 ns each — enough, at that call count, to account for the whole
regression. A batched accessor restored the single-borrow shape and recovered
the cost in full; it is `Heap::activation_receiver_and_current_class` today.

The companion result is what makes the number legible rather than alarming:
the identical pattern — one extra `RefCell` borrow per call — also appeared
in `dispatch::walk`, on the per-inheritance-level path method lookup walks,
and cost nothing measurable there. The difference is not that one borrow is
expensive and the other cheap; it is executions per benchmark run. A lexical
send runs 250,000 times in this benchmark, an inheritance level far fewer.
The lesson generalized to the arena migration that followed: a per-access cost is
only worth chasing where the access count is large, and the way to tell
which sites those are is to measure, not to guess from the shape of the code.

### What the activation stack moved

Moving activations onto the stack was measured against the arena in one sitting,
on 2026-09-10: the before-binary was built from the branch's merge-base in a
throwaway worktree, and both binaries ran `ns-next bench` — ten timed runs
after a discarded warm-up — over five sources, three rounds, alternating
the binaries within each round. Three sources are fixtures: `bench_loop.ns`,
`bench_send.ns` (row three of the decomposition above, `tick: x = ( ^x + 1 )`)
and `bench_block.ns` (a method that creates and invokes a block per iteration).
The other two are the decomposition's rows one and two, which differ from
`bench_send.ns` by one statement each and are not fixtures. Medians per round,
and the middle of the three as ns per iteration (`median_ms * 1e6 / 50000`):

| Bytecode | Before (arena), rounds 1–3 | After (stack), rounds 1–3 | ns/iteration |
|---|---|---|---|
| `bench_loop.ns` | 16.27, 16.50, 16.43 ms | 16.07, 16.22, 16.57 ms | 328.6 → 324.4 |
| `bench_send.ns` | 23.49, 23.46, 23.58 ms | 20.68, 20.51, 20.51 ms | 469.8 → 410.2 |
| `bench_block.ns` | 38.83, 38.99, 39.16 ms | 38.46, 40.84, 39.24 ms | 779.8 → 784.7 |
| row one, `sum:: sum + 1` | 16.16, 16.18, 16.24 ms | 15.97, 15.98, 16.26 ms | 323.5 → 319.5 |
| row two, `tick: x = ( ^x )` | 19.18, 20.07, 19.28 ms | 17.84, 17.83, 17.90 ms | 385.6 → 356.9 |

The decomposition, re-taken from the middle medians (ns per iteration):

| | Row one | Row two | Row three | User send (3 − 1) | 2 − 1 | 3 − 2 |
|---|---|---|---|---|---|---|
| Bytecode, arena | 323.5 | 385.6 | 469.8 | 146.3 | 62.1 | 84.2 |
| Bytecode, stack | 319.5 | 356.9 | 410.2 | **90.7** | **37.3** | 53.4 |

**A user-method send fell from ~146 ns to ~91 ns, about 38%**, and
`bench_send.ns` from 23.5 ms to 20.5 ms, about 13%. The three
before-medians and the three after-medians do not overlap, and the per-round
differences agree with each other: 146.7, 145.7 and 146.8 ns before, 94.1, 90.7
and 85.0 ns after. What a user send costs beyond a native one (row two minus row
one) fell from ~62 ns to ~37 ns — per round 60.4, 78.0 and 60.9 before, 37.5,
37.1 and 32.8 after.

Row three minus row two fell as well, from ~84 ns to ~53 ns, although both
binaries run the same native `+`. That is the `temporaries` allocation described under
the decomposition: under the arena, row three's `tick:` allocated a temporaries
vector and row two's did not, so the arena binary's 3 − 2 carries that allocation
and its 2 − 1 leaves it out. The stack allocates for neither. That attribution is
an inference from the two bodies' register counts; no profile was taken. It also
means the arena binary's 2 − 1 understated what an activation with temporaries
cost, so 3 − 1 is the figure to compare.

`bench_loop.ns` and row one did **not** move: the compiler inlines `whileTrue:`,
so the loop builds no activation for the stack to make cheaper, and the medians
overlap.

**`bench_block.ns` did not move.** Every iteration creates a
block, so every iteration still reifies `bump:`'s activation into the frame arena,
and divorcing it copies its registers into two vectors when it returns. The stack
took the block's own activation out of the arena, yet the median did not move;
a profile taken later attributes that to allocation ("What the profiles show"
below).
What does show is memory: peak working set over this source fell from 966.9 MB
to 446.0 MB ([memory.md](memory.md)).

The caveats are the arena subsection's: whole-program medians, incremental
differences rather than a profiler attribution, and a native path that pays a
string-comparison chain the user path does not. The pre-arena figures above —
~165 ns a user send, ~68 ns an activation — come from another sitting and are not
compared with these.

### What retiring the tree-walker moved

The retirement was measured against the branch's merge-base in one sitting, on
2026-09-11, the same way: the before-binary built in a throwaway
worktree, `ns-next bench` over the three fixtures, three rounds, alternating the
binaries within each round. Bytecode medians in ms:

| Source | Before, rounds 1–3 | After, rounds 1–3 |
|---|---|---|
| `bench_loop.ns` | 16.58, 16.53, 16.43 | 16.57, 16.38, 16.51 |
| `bench_send.ns` | 20.75, 20.68, 20.83 | 20.67, 20.48, 20.44 |
| `bench_block.ns` | 39.40, 39.91, 39.04 | 37.65, 37.61, 38.48 |

Nothing got slower. `bench_loop.ns` did not move. `bench_send.ns` moved about
1% — the two groups just fail to overlap — which is at the edge of noise.
**`bench_block.ns` got about 4% faster**, from ~39.4 ms to ~37.6 ms, by more than
either group's spread. That source creates and invokes a block every iteration,
and the block is what changed: a closure no longer carries the resolver's
bindings map or a body enum, so creating one no longer clones a shared `Rc` and
invoking one no longer matches on the body, and the closure object it allocates
is smaller — an `Object` went from 64 to 56 bytes in a release build. That
attribution is an inference from what the change removed; no profile was taken.

### What step B moved

Step B, the behaviour-preserving cleanup that followed the retirement
([clean-up-with-one-engine](../superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md)),
was measured whole against its merge-base in one sitting, on 2026-09-12, the
same way: the before-binary built in a throwaway worktree, `ns-next bench`
over the three fixtures, three rounds, alternating the binaries within each
round. Bytecode medians in ms:

| Source | Before, rounds 1–3 | After, rounds 1–3 | ns/iteration |
|---|---|---|---|
| `bench_loop.ns` | 16.08, 16.17, 16.12 | 15.31, 15.32, 15.49 | 322.3 → 306.4 |
| `bench_send.ns` | 20.55, 20.28, 19.95 | 18.65, 19.06, 19.15 | 405.6 → 381.2 |
| `bench_block.ns` | 37.38, 38.29, 38.68 | 29.94, 29.74, 30.02 | 765.8 → 598.8 |

On every source the after-group lies entirely below the before-group.
`bench_loop.ns` got about 5% faster, `bench_send.ns` about 6%, and
**`bench_block.ns` about 22%**, from ~38.3 ms to ~29.9 ms. Every artifact the
fixtures produced is byte-identical between the two binaries (44
artifacts), as a change that preserves behaviour requires.

Each change that touched a hot path was also measured on its own, against the
commit before it, in its own sitting. Middle medians in ms, `bench_loop.ns` /
`bench_send.ns` / `bench_block.ns`:

- **Builtin classes indexed by discriminant** (a `Vec` indexed by
  `BuiltinClass` instead of a hashed map): 16.59 / 20.70 / 38.93 → 16.04 /
  19.90 / 37.44. Faster on all three, by 3–4%, with no overlap between the
  groups: a send to an integer or a boolean finds its class through that
  accessor, and so does every closure allocation.
- **The runtime's own symbols interned once, as constants**: 15.95 / 20.16 /
  37.46 → 16.03 / 20.07 / 37.71. **Nothing moved**; the groups overlap on every
  source, because none of the three fixtures reaches a changed site inside the
  timed send.
- **A closure behind an `Rc`**: 16.06 / 19.96 / 37.47 → 15.71 / 19.89 / 31.26.
  **`bench_block.ns` about 17% faster**: `value:` bumps a reference count
  where it used to copy `ClosureData` out of the heap and wrap it in a fresh
  `Rc`, and `MakeClosure` shares its parameter names instead of cloning them.
  `bench_loop.ns` moved about 2%; `bench_send.ns` did not move.
- **Compiled code on the declaration** (see "The VM"): 15.86 / 19.81 / 31.26 →
  16.14 / 19.56 / 30.59. `bench_send.ns` and `bench_block.ns` about 1–2%
  faster, with no overlap, from the hashed lookup of the compiled body that
  every user-method send no longer does. `bench_loop.ns`'s first three rounds
  did not separate the two groups; three more rounds put the after-binary
  lower each time, not slower.
- **Handles passed by value**: 15.58 / 18.75 / 29.67 → 15.41 / 19.13 / 29.93.
  **`bench_loop.ns` about 1% faster**: its after-range [15.23, 15.42] lies
  entirely below its before-range [15.46, 15.70] -- the groups just fail to
  overlap. `bench_send.ns` and `bench_block.ns` did not move. `bench_send.ns`'s
  first three rounds only just overlapped; three more narrowed the gap to
  0.17 ms, inside the before-group's own spread. A release build had already
  inlined the indirection away.

The per-change figures come from five different sittings, so they are not
compared with each other or summed into the whole; the whole-step table above
is the one comparison in a single sitting. The per-change attributions are
inferences from what each change removed; no profile was taken.

### What the trampoline moved

The trampoline ("The trampoline" below) was measured whole against the commit
it started from, `04e5fc0`, in one sitting, on 2026-09-12, the same way: the
before-binary built in a throwaway worktree, `ns-next bench` over the three
fixtures, three rounds, alternating the binaries within each round. Bytecode
medians in ms:

| Source | Before, rounds 1–3 | After, rounds 1–3 | ns/iteration |
|---|---|---|---|
| `bench_loop.ns` | 16.96, 16.40, 16.53 | 11.23, 11.27, 11.28 | 330.6 → 225.3 |
| `bench_send.ns` | 20.48, 20.33, 20.20 | 14.13, 13.95, 13.87 | 406.7 → 279.0 |
| `bench_block.ns` | 30.77, 30.17, 31.14 | 23.66, 24.38, 23.97 | 615.3 → 479.4 |

On every source the after-group lies entirely below the before-group, by far
more than either group's spread: **`bench_loop.ns` about 32% faster,
`bench_send.ns` about 31%, `bench_block.ns` about 22%.** The optimized lowering
emits the same instructions as before, so every artifact
of it is byte-identical between the two binaries: `compared 46 artifacts, 0
differing`, over the 23 fixtures. A reference-lowering artifact differs by
construction, since it now carries `SendInlinable`, and was not compared.

Each change on the hot path was also measured on its own, against the commit
before it, in its own sitting. They come from several sittings, so they are not
compared with each other or summed into the whole; the attributions are
inferences from what each change removed, and no profile was taken.

- **Deciding before running, still recursive.** Splitting every send into a
  decision (`Step`, `decide_send` and its two relatives) and a run first made
  every source 6–11% slower: each `Send*` arm made two out-of-line calls where
  it had made one. Answering a `Step::Value` in place, with only the method
  case behind a call, recovered part of it. Middle medians in ms,
  `bench_loop.ns` / `bench_send.ns` / `bench_block.ns`: 15.50 / 19.17 / 30.89
  before, 17.18 / 21.14 / 32.67 split, 16.67 / 20.11 / 31.72 with the value
  answered in place — still 3–8% slower, accepted as the cost of a shape the
  loop was about to consume.
- **The loop.** Against the commit before the split and against its own
  parent: 15.76 / 19.22 / 30.26 and 16.81 / 20.34 / 31.69 → 11.57 / 14.38 /
  25.99. Faster on every source against both, by about 27%, 25% and 14% against
  the commit before the split. `bench_loop.ns`, which makes no user-method send
  in its loop, gained most: every native-leaf and slot send used to allocate a
  `Vec` for its arguments, and the reusable buffer removes that allocation.
  `bench_send.ns` also loses the Rust recursion and the second call per method
  send.
- **Blocks as steps.** 11.45 / 14.30 / 26.12 → 11.15 / 13.91 / 23.58.
  **`bench_block.ns` about 9% faster**: `value:` is a push, where it was a
  native call, a nested run of the loop and a pop. `bench_loop.ns` and
  `bench_send.ns`, which activate no block in their loops, moved about 2.5%,
  outside both groups' spread but not explained by anything removed from their
  paths — code layout, most likely, and not something to bank on.
- **The two bounds** — L, and a bound on re-entries of the loop from Rust that
  the trampoline still had and that went with the re-entries ("No re-entry
  from Rust" below). The first version marked a control-flow native's block
  with a `bool` field on `Step::Block`, and measured 4–8% slower on every
  source, `bench_loop.ns` included, whose loop activates nothing. Taking the
  pieces out one at a time found the field itself to be the cause, with
  nothing reading it; the counting logic, the counted flag on a stack entry,
  the arm split in the loop and the iterative superclass walk each measured
  nothing:

  | Variant | `bench_loop.ns` | `bench_send.ns` | `bench_block.ns` |
  |---|---|---|---|
  | parent commit | 11.1 | 13.9 | 23.4 |
  | the bounds, marked by a field | 12.0 | 14.7 | 24.4 |
  | the same, with the counting, the counted flag and the iterative walk taken out | 12.0 | 15.1 | 24.5 |
  | and the field taken out too | 11.2 | 14.1 | 23.4 |
  | the bounds, marked by `StepArguments::ControlFlow` | 11.2 | 14.1 | 23.8 |

  `size_of::<Result<Step, Signal>>()` is 56 bytes with and without the field,
  so the likely mechanism is the niche a `bool` offers for `Step`'s
  discriminant, changing how every decision's result is encoded and tested;
  that was not checked at the assembly level. With the mark on the arguments
  instead, two sittings put every source level with the parent — 11.11 /
  13.97 / 23.46 against 11.23 / 14.07 / 23.60 in the first — `bench_loop.ns`
  about 1% higher in both, inside the after-group's spread.
- **Caching the running activation's register base, measured and reverted.**
  The loop re-reads an entry's `base` on every register access; holding it in
  a local beside the running activation, refreshed on every switch, measured
  no source faster and none slower over six rounds (group ranges in ms, before
  against after: `bench_loop.ns` 10.98–11.32 against 10.97–11.50,
  `bench_send.ns` 13.75–14.16 against 13.71–14.20, `bench_block.ns`
  22.95–23.81 against 23.13–24.00). A change that makes nothing faster is not
  kept, so it was reverted: one index into a hot entry is already cheap enough
  that avoiding it does not show.

The interpreter thread's stack, `INTERPRETER_STACK_BYTES = 8 MiB`
(`crates/ns-cli/src/main.rs`), is sized for what still recurses on the Rust
stack. Nothing re-enters the run loop from Rust ("No re-entry from Rust"
below), so that is the front end and the compiler, whose depth the parser's
nesting ceiling bounds, except along a chain of messages: it nests a send
per message through its receivers, as deep as the source is long, and the
compiler walks it by recursion ([OPEN-WORK.md](../OPEN-WORK.md)). The
suite runs the deepest nesting the parser accepts through every stage on
libtest's default 2 MiB threads in a debug build —
`the_deepest_module_the_parser_accepts_round_trips` and
`the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs` in
`crates/ns-runtime/tests/image.rs` — and the thread gets four times that.

The suite (`cargo test --workspace`, debug build, test binaries already built)
went from 9.36 s, 617 tests, to 12.82 s, 637 tests, in two runs alike. 3.1 s of
the difference is `crates/ns-runtime/tests/trampoline.rs`, whose boundary test
runs three recursions to 99,999 and to 100,000 counted activations under each
lowering, and whose other tests recurse 40,000 and 50,000 deep or run into the
limit.

### What the profiles show

Two binaries were profiled on 2026-09-12, both release builds with debug
information: the commit the trampoline started from (`04e5fc0`) and the
trampoline's head (`641b8d2`). Each ran `ns-next bench` over `bench_block.ns`
and `bench_send.ns` with 500 iterations under `samply record` (ETW sampling at
1 kHz, Windows 11). samply resolved the system libraries but not ns-next's own
frames, which were resolved afterwards against the PDB through `dbghelp`. Only
samples under the timed entry send (`timed_send`) are counted, because parsing,
compiling and, above all, dropping the heap at exit also land in a profile — for
`bench_block.ns` before the trampoline, the drop of the never-reclaimed arena
was 29% of all samples. The figures are inclusive shares of the timed samples;
`ntdll` has no public symbol names here, so its time is charged to the nearest
ns-next caller.

| Where the time goes | `bench_block.ns`, before | `bench_send.ns`, before | `bench_block.ns`, head | `bench_send.ns`, head |
|---|---|---|---|---|
| a send's argument vector (`vm::collect`), almost all heap allocation and free | 16.7% | 31.1% | — | — |
| `value` as a native callback (`builtins::call_block`) | 12.1% | — | — | — |
| reifying the frame (`Heap::reify`), mostly `RtlCopyMemory` | 9.6% | — | 12.5% | — |
| popping, including the divorce (`Heap::pop_activation`) | 8.9% | 0.2% | 13.1% | 0.4% |
| `Symbol::intern` | 7.1% | 9.3% | 5.8% | 11.9% |
| deciding a send (`Interpreter::decide_send`) | — | — | 10.8% | 16.7% |

**This answers what offset the activation stack's saving on `bench_block.ns`**
("What the activation stack moved" above): allocation. The stack took the
block's own activation out of the frame arena, but every send still built an
argument vector, every block creation still reified its method's activation
into an arena that grows by doubling — the `RtlCopyMemory` under `reify` is
that growth copying every frame so far — and every return still divorced the
frame into two fresh vectors, while `value:` ran through a native callback and
a nested run. Removing the closure copy in step B took one piece of that; the
trampoline took the argument vector and the callback.

What is left on `bench_block.ns` is mostly the reification and the divorce —
about a quarter of the timed samples — which is what closure conversion
("Closure conversion") and a collector ([memory.md](memory.md)) are
for. On `bench_send.ns` the profile shows a cost no measurement had
isolated: `Symbol::intern` is 11.9% of the timed samples. `self tick: sum` is a
lexical send whose selector has a setter's shape, so
`sends::decide_lexical_send` interns the getter name `tick` — hashing its text —
on every send to find out whether the receiver has a slot of that name. The
method and slot tables add SipHash on `Symbol` keys (`hash_one<Symbol>`, 5.1%
self). Spec-conformant lookup removed both: a slot's accessors are method-table
entries, so no send interns a name, and the tables hash identity keys with one
multiply ("What spec-conformant lookup moved" below).

### 3a's head, before the caches (2026-09-16)

Before building the caches, 3a's head (`60ac69e`, the same code as the plan's
`START`, `49a2a38`) was profiled to read the caches' ceiling off the send path
they will actually sit in front of, rather than assume it. One release build
with debug information and the protocol's flags (`+crt-static`, the
function-alignment pin), `samply record --save-only` at 500 iterations per
source (ETW sampling at 1 kHz, Windows 11), over the seven sources of "What
removing the re-entries moved" below: the three fixtures and the four
throwaway `target\bench-lookup` sources. `--save-only` defers all
symbolication to the viewer, so the saved profile carried no symbol names for
any module, ns-next.exe included; every ns-next.exe frame was resolved
afterwards against `target\release\ns_next.pdb` through `wholesym`, the
library samply's own local symbol server uses for the same job, including its
inline site records. At this optimization level `vm::run`,
`sends::decide_lexical_send`, `sends::decide_super_send` and
`dispatch::private_member` carry no standalone symbol at all: LLVM inlined
each of them into `Interpreter::run_from_host`, and only the PDB's inline
chain still names them, at their own program counters inside it.
`Mutator::decide_send`, `dispatch::lookup_public`, `lookup_protected`,
`metaclass_of`, `builtins::execute`, `decide_slot_accessor`, `nested_class`,
`decide_instantiation`, `push_step`, `push_activation`, `pop_activation` and
`reify` each still stand as their own symbol. Reading every call site confirms
`lookup_public`, `lookup_protected`, `private_member` and `metaclass_of` are
reached only from inside `Mutator::decide_send`, `sends::decide_lexical_send`
or `sends::decide_super_send`, so their samples are already inside those three
functions' union and add nothing on top of it. As in the 2026-09-12 profile,
only samples under `timed_send` are counted and a system-module frame
(`ntdll`, `KernelBase`) with no name of its own is charged to the nearest
ns-next.exe caller on the stack.

| Source | Timed samples | Deciding a send | `builtins::execute` | `push_step`/`push_activation` | `pop_activation`/`reify` | `vm::run` self | **Lookup ceiling** |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5,742 | 52.8% | 12.4% | 0.0% | 0.0% | 4.2% | **19.2%** |
| `bench_send.ns` | 6,282 | 48.6% | 12.1% | 5.8% | 0.5% | 3.7% | **19.8%** |
| `bench_block.ns` | 12,361 | 29.0% | 5.9% | 6.1% | 27.0% | 2.5% | **15.0%** |
| `bench_do.ns` | 3,802 | 37.9% | 13.9% | 15.2% | 0.6% | 2.5% | **13.5%** |
| `bench_new.ns` | 3,598 | 55.8% | 8.2% | 8.6% | 0.6% | 2.4% | **22.4%** |
| `bench_new2.ns` | 4,121 | 51.6% | 7.6% | 12.7% | 0.8% | 3.0% | **19.8%** |
| `bench_newarg.ns` | 5,387 | 41.5% | 5.4% | 11.9% | 1.1% | 2.6% | **15.7%** |

The lookup ceiling is "deciding a send" minus the samples where deciding also
touches `builtins::execute`, `decide_slot_accessor`, `nested_class` or
`decide_instantiation` — the primitive itself, the accessor and nested-class
answers a slot or nested declaration reaches directly, and instantiation's own
work, none of which a cache removes. On every source the ceiling sits at less
than half of "deciding a send": `bench_new.ns`'s instantiation-heavy path
spends most of its deciding time inside `decide_instantiation` itself, not the
method-table walk a cache would shortcut, and `bench_block.ns`'s repeated
reification and divorce — 27.0%, the largest single share on any source — is
what "Closure conversion" and a collector ([memory.md](memory.md))
are for, not the caches. `bench_loop.ns`'s `+` and `<` never leave the
primitive path, so it pushes no activation at all (0.0%) and pops none;
`bench_send.ns`'s single `tick:` call an iteration pushes one (5.8%) and,
since `tick:` creates no closure, pops it without reifying (0.5%, against
`bench_block.ns`'s 27.0%, where every iteration creates one).

### What removing the re-entries moved

Taking the last re-entries of the loop from Rust away ("No re-entry from Rust"
below) was measured whole against the commit it started from, `be3eb40`, in
two sittings on 2026-09-13: the before-binary built in a throwaway worktree,
`ns-next bench <source> --iterations 30` over seven sources, five rounds,
alternating the binaries within each round. Besides the three fixtures, four
throwaway sources exercise the paths that changed and no fixture does, each
printing the sum it accumulates:

- `bench_do.ns`: 50,000 block activations through `do:` — a 5,000-iteration
  `whileTrue:` loop over `{1. 2. 3. 4. 5. 6. 7. 8. 9. 10} do: [:e | sum:: sum + e]`.
- `bench_new.ns`: 20,000 instantiations of
  `class Cell = Object new ( | v ::= 1. | )`, each followed by `v`.
- `bench_new2.ns`: the same with two levels of slots,
  `class Base = Object new ( | u ::= 1. | )` and
  `class Cell = Base new ( | v ::= 1. | )`.
- `bench_newarg.ns`: the same with a clause message that has an argument,
  `class Base x: a = Object new ( | u ::= a. | )` and
  `class Cell = Base x: 1 ( | v ::= 1. | )`, each instantiation followed by
  `u`.

Those measurements timed the same entry send `bench` times today, `main:args:`
of the root's instance, so figures taken before `bench` assembled an
application like `run` stay comparable with figures taken after.

They are not fixtures; the first two are written out in the step's
[plan](../superpowers/plans/2026-09-13-0149-ns-next-no-reentry.md),
"Measurement protocol", and the other two are `bench_new.ns` with the classes
above.

Every binary a measurement compares — the current tree included — is built
with the same explicit flags, and those pin every function start to a
64-byte boundary. In PowerShell, before each build and for the measurement
only:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --release -p ns-cli
Remove-Item Env:RUSTFLAGS
```

`RUSTFLAGS` replaces the configuration's flags rather than adding to them,
which is why `+crt-static` is repeated there; a Linux or macOS build carries
the alignment flag alone. Why the pin: step 2c's brand commit changed no
instruction of the run loop and still measured 1–2% slower than its parent,
because smaller cold code ahead of the hot functions moved every one of them
into a different 64-byte phase ("What 2c moved" below). With every function
start pinned, that phase shift is gone — which is not the same as a
comparison measuring code rather than placement; the paragraph below says
what the pin does not cover. Why the shipped
build does not carry it: the pin costs about 110 KB of padding, 5.6% of the
release binary, and measured no faster — the unpinned build was level or up
to 1.8% faster than the pinned one — so `.cargo/config.toml` carries only
`+crt-static`, and a build from the configuration alone is the one that
ships, not the one that is compared. Before timing, check that no binary
imports `VCRUNTIME140.dll` (`dumpbin /dependents`) and that each prints the
expected output for every source; a linker map (`-C link-arg=/MAP:<file>`)
shows every function of the crate at a multiple of 64 — the only symbols off
the boundary are the static C runtime's own objects.

The pin removes the 64-byte phase shift and nothing else. `bench_block.ns` has
since shown a **16% spread between builds of the same tree** with the pin in
place, stable to about 1% within each build, so on that source one build per
commit measures the build rather than the commit; pool several builds of each
tree instead ("What `Top`, `Value` and `Number` cost" below, which found it).
The other seven sources stay within 0.3–2.3% between builds. Pooling several
plain builds varies nothing when the builds are byte-identical, as they are
on the machine these measurements ran on ("What the value conditions cost"
below): build the extra ones with `-C metadata=<word>` added to `RUSTFLAGS`.
That varies the order of functions, and nothing inside one: the three
`-C metadata` builds of a tree have `vm::run` identical instruction for
instruction at the same offsets from its start ("What frames as words
moved" below), so they sample one layout of the run loop three times, and
a difference that follows from where its blocks fall -- the indirect
dispatch jump every instruction arm shares among them -- survives the
pool. So the second and third builds also align every block not reached
by fall-through, each instruction arm the dispatch jumps to among them,
the second to 16 bytes and the third to 32 (decided with the user on
2026-10-24):

```powershell
$pinned = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
$buildA = $pinned
$buildB = "$pinned -C metadata=buildb -C llvm-args=-align-all-nofallthru-blocks=4"
$buildC = "$pinned -C metadata=buildc -C llvm-args=-align-all-nofallthru-blocks=5"
```

the same three on both sides, each set as `RUSTFLAGS` for its own build
in its own `--target-dir`. Before timing, check in a disassembly of a
build with debug information (`CARGO_PROFILE_RELEASE_DEBUG=limited`, the
build's flags) that the three give `vm::run` three different layouts; if
two coincide, take another alignment and say so. The runtime's data layout (which symbol ids,
object offsets and dispatch ids a program's objects receive) is varied by
none of them, so a difference that follows from data layout survives every
such pool.

Medians in ms from the first sitting, each group's mean, and the change of
the mean in both sittings:

| Source | Before, rounds 1–5 | mean | After, rounds 1–5 | mean | Change, sitting 1 / 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 10.95, 11.03, 10.96, 10.98, 11.05 | 11.00 | 11.35, 11.34, 11.08, 11.15, 11.10 | 11.20 | **+1.9% / +1.4%**, after entirely above in both |
| `bench_send.ns` | 14.25, 13.86, 13.86, 13.81, 13.74 | 13.90 | 13.87, 13.93, 13.93, 13.98, 13.94 | 13.93 | +0.2% / +0.8%, groups overlap in both |
| `bench_block.ns` | 23.12, 23.14, 22.90, 23.12, 22.92 | 23.04 | 23.31, 23.53, 23.45, 23.39, 23.37 | 23.41 | **+1.6% / +1.4%**, after entirely above in the first |
| `bench_do.ns` | 7.85, 7.90, 7.87, 8.00, 7.84 | 7.89 | 7.88, 7.74, 7.78, 7.78, 7.75 | 7.79 | −1.3% / −1.3%, groups overlap in both |
| `bench_new.ns` | 7.30, 7.27, 7.28, 7.26, 7.29 | 7.28 | 7.11, 7.19, 7.37, 7.15, 7.13 | 7.19 | −1.2% / −2.7%, after entirely below in the second |
| `bench_new2.ns` | 8.17, 8.19, 8.26, 8.16, 8.23 | 8.20 | 8.16, 8.09, 8.08, 8.16, 8.15 | 8.13 | −0.9% / −1.2%, after entirely below in both |
| `bench_newarg.ns` | 10.37, 10.60, 10.51, 10.66, 10.40 | 10.51 | 10.10, 10.04, 10.13, 10.18, 10.10 | 10.11 | −3.8% / −3.2%, after entirely below in both |

**Instantiation is faster than it was with the re-entries**, on all three
shapes, and `do:` and `bench_send.ns` are level with it. What decided that is
isolated, each change measured against its parent in a sitting of its own
(below): two boxes per instantiation body, the box and the list of levels
every instantiation allocated, and a call to `Continuation`'s drop glue on
every return. An initializer now runs in the same loop — `push_step`, a
switch, and `finish` — where it ran in a nested run of it, and once the walk
allocates nothing per instantiation, that is the cheaper of the two. `do:`
running its block as a step instead of in a nested run did not make it clearly
faster, although the design expected it to: −1.3% in both sittings, the groups
overlapping.

**Open finding: `bench_loop.ns` and `bench_block.ns` are 1.4–1.9% slower**,
the after-group entirely above the before-group in three of the four
comparisons. Neither runs `do:`, an instantiation or the accessor in its loop:
`bench_loop.ns` makes only native-leaf and slot sends, and `bench_block.ns`'s
`whileTrue:` is inlined and each iteration makes lexical sends for the slots
and `bump:`, public sends of `<`, `+` and `value:`, and one block creation.
What they run of the changed code is `Interpreter::decide_send` and
`sends::decide_lexical_send`, whose factory-send and nested-class branches now
answer a `Step` (neither is taken here); `builtins::dispatch_native`, whose
`Array` and `OrderedCollection` arms answer a `Step` themselves, with no arm
added; `Interpreter::push_step` (`bench_block.ns` takes its block arm, for
`value:`), whose match has four body arms more; and the types every one of
those sends passes through — `Step`, with four variants more, and the
`Continuation` `vm::finish` reads, with three more, whose `Deliver` it handles
inline, so the loop never reaches `Interpreter::resume`. This is the uniform
shift the first change of the step already showed on code it did not change
(below), an artifact of inlining and register allocation as far as that
investigation went. It is not isolated, and nothing removed from either
benchmark's path explains it. Code placement is the likely mechanism — a
change elsewhere moving the hot functions into another 64-byte phase — but
that was verified only for 2c, whose investigation found it and is why a
measurement now pins function alignment ("What 2c moved" below, and the
protocol above). The comparison with `be3eb40` is carried through
every measurement up to step 3; step 2b's ("What 2b moved" below) puts
`bench_loop.ns` at +1.1% in both of its sittings, the groups overlapping, and
`bench_block.ns` 2.5% faster than `be3eb40`.

Each change was measured against its parent on the way, in sittings of its
own, so its figures are not compared with these:

- **`do:` as a step.** It moved every source by a uniform 2–4%, `bench_loop.ns`
  included, which runs no `do:`. A control — the parent commit plus only the
  new `Continuation::DoElement` variant — did not reproduce it. In the
  post-LTO assembly `vm::finish` is out of line in every build, so the
  continuation machinery never sat in the loop; in the changed build
  `object_class::dispatch` was inlined into `dispatch_native` and `run`'s
  register allocation differed, doing the same work. Moving the continuation
  handling out of `finish` and marking `do_step` `#[inline(never)]` recovered
  nothing. It was accepted as an LLVM inlining and layout artifact on unchanged
  executed code, not bisected further. Code placement is the likely mechanism,
  verified only for 2c ("What 2c moved" below).
- **The nested-class accessor as a step** made no source slower; by the
  protocol's rule `bench_send.ns`, `bench_block.ns` and `bench_new.ns` came out
  faster, with nothing on their paths to explain it.
- **Instantiation as a state machine.** The first version made `bench_new.ns`
  about 15.6% slower. Counting allocations found the cause: two more boxes per
  instantiation, the walk's state and a boxed description of the body (4
  allocations, 296 bytes, per `Cell new`, against 2 and 144). Letting the last
  initializer deliver the instance and carrying it inline as
  `Step::Initializer` restored the count to 2 and 144 bytes on that shape and
  left about +1.8%. A chain of two levels with code still boxed both for every
  body but the last, which showed only once `bench_new2.ns` and
  `bench_newarg.ns` joined the measurement: 15.4% and 18.3% slower than
  `be3eb40`.
- **Every body inline.** Carrying `Step::InitializerThen`,
  `Step::ClauseMessage` and `Step::SuperclassClause` inline instead of a
  boxed body description (the first two since gone with the walk, "What
  `new` as compiled code moved"): `bench_new2.ns` −5.2%, `bench_newarg.ns` −8.0%, both
  entirely below, the other five sources level. The first version gave
  `Step::Initializer` an optional continuation, which made it as large as
  `Step::Method`; `Result<Step, Signal>` stayed at 56 bytes, yet the four
  sources that instantiate nothing got 4–6% slower, all entirely above ("Every
  body is inline in its `Step`" below).
- **Reusing the walk's box.** Taking the walk's box and its list of levels
  from the interpreter's spares instead of allocating both: `bench_new.ns`
  −3.0%, `bench_new2.ns` −8.8%, `bench_newarg.ns` −12.8%, all entirely below;
  the others level.
- **`Continuation`'s drop glue.** With three boxed variants, the drop glue of
  `Continuation` stayed out of line, and `vm::finish` called it on every
  return for the `Deliver` it left in the popped entry. Taking the `Deliver`
  and forgetting it moved `bench_block.ns`, `bench_new.ns`, `bench_new2.ns`,
  `bench_send.ns` and `bench_do.ns` by −0.1% to −1.4% in two sittings, none
  slower, the groups overlapping.
- **The in-progress marker** in the cache slot, replacing the list of
  creations underway, moved no source (−1.2% to +1.1%, overlapping); it was
  made for the test suite's time and a crafted binary's (below).
- **Comparing `do:` as a well-known symbol**, instead of through the interner,
  was tried and not kept: `bench_block.ns`, `bench_new.ns` and `bench_new2.ns`
  got 1.7–2.1% slower, none of which sends to a collection in its loop.

The suite (`cargo test --workspace`, debug build, test binaries already built)
takes 12.5 s for 662 tests, in two runs alike — as long as before the step,
12.5 s for 644. For a while it took 25 s: two tests recurse through the
nested-class accessor to the activation limit, about 50,000 creations underway
at the deepest, and the cyclic-definition check scanned a list of every
creation underway at each level, quadratic in the depth, where the bound on
re-entries had stopped such a recursion at 1,000 levels. With the marker in
the cache slot the check is one read, and
`goldens/runtime/depth-limit/recursion_through_the_nested_class_accessor_is_bounded_by_the_activation_limit.ns`
takes 0.3 s again.

### What 2b moved

Step 2b ([memory.md](memory.md), "Checked roots and a collector") put three
things into a release build: the safepoint poll in the run loop ("The VM"
above), a closure's payload inline in its object, and host roots at the host
boundary; the tracing, the move and the stress mode exist only in a debug
build. It was measured whole against the commit it started from, `4882a2c`,
and against `be3eb40`, in two sittings on 2026-09-13, by the protocol of "What
removing the re-entries moved" above: both before-binaries built in throwaway
worktrees, `ns-next bench <source> --iterations 30` over the same seven
sources, five rounds, alternating the three binaries within each round.
Medians in ms from the first sitting, and each group's mean:

| Source | `be3eb40`, rounds 1–5 | mean | `4882a2c`, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.22, 11.16, 11.12, 11.34, 11.21 | 11.21 | 11.64, 11.19, 11.40, 11.28, 11.37 | 11.38 | 11.32, 11.44, 11.30, 11.32, 11.28 | 11.33 |
| `bench_send.ns` | 14.07, 13.93, 14.09, 14.21, 13.87 | 14.03 | 14.03, 14.19, 14.12, 14.13, 14.15 | 14.12 | 14.01, 14.35, 14.10, 14.23, 14.11 | 14.16 |
| `bench_block.ns` | 23.26, 23.30, 23.52, 23.45, 23.17 | 23.34 | 23.87, 23.70, 23.52, 24.03, 23.64 | 23.75 | 22.76, 22.70, 22.67, 22.83, 22.82 | 22.76 |
| `bench_do.ns` | 7.92, 8.17, 7.96, 7.98, 8.05 | 8.02 | 8.00, 8.10, 7.90, 7.88, 8.22 | 8.02 | 7.96, 7.97, 7.80, 8.00, 7.81 | 7.91 |
| `bench_new.ns` | 7.46, 7.39, 7.49, 7.49, 7.37 | 7.44 | 7.44, 7.27, 7.41, 7.27, 7.31 | 7.34 | 7.32, 7.45, 7.46, 7.40, 7.42 | 7.41 |
| `bench_new2.ns` | 8.34, 8.41, 8.47, 8.38, 8.50 | 8.42 | 8.24, 8.33, 8.26, 8.35, 8.42 | 8.32 | 8.48, 8.44, 8.32, 8.31, 8.22 | 8.35 |
| `bench_newarg.ns` | 10.66, 10.45, 10.57, 10.59, 10.60 | 10.57 | 10.20, 10.24, 10.22, 10.27, 10.29 | 10.24 | 10.26, 10.21, 10.24, 10.26, 10.11 | 10.22 |

The change of the after-group's mean in both sittings:

| Source | Against `4882a2c`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|
| `bench_loop.ns` | −0.4% / −0.9%, overlapping | +1.1% / +1.1%, overlapping |
| `bench_send.ns` | +0.3% / −0.5%, overlapping | +0.9% / +1.4%, overlapping |
| `bench_block.ns` | **−4.2% / −3.9%**, after entirely below in both | **−2.5% / −2.6%**, after entirely below in both |
| `bench_do.ns` | −1.4% / −1.1%, overlapping | −1.4% / −1.3%, overlapping |
| `bench_new.ns` | +1.0% / +0.7%, overlapping | −0.4% / −1.4%, the groups touching at 7.46 in the second |
| `bench_new2.ns` | +0.4% / +0.8%, after entirely above in the second, by 0.01 ms | −0.8% / −0.6%, overlapping |
| `bench_newarg.ns` | −0.3% / −0.4%, overlapping | −3.4% / −3.2%, after entirely below in both |

**No source is slower than at `4882a2c` beyond the sittings' noise, and
`bench_block.ns` is about 4% faster**, which is the closure split's (below).
One group lay outside the rest: in the second sitting `bench_new2.ns`'s
after-group sat entirely above its parent-group, 8.37–8.44 against 8.30–8.36
ms. The first sitting overlapped, and a third, run to settle it, put it level
(−0.2%, overlapping).

**Against `be3eb40`, nothing is slower beyond the groups' spread.**
`bench_loop.ns`, which 2a left 1.4–1.9% slower with its after-group entirely
above, is +1.1% in both sittings, overlapping; `bench_block.ns`, which 2a left
1.4–1.6% slower, is 2.5% faster. `bench_send.ns` is +0.9% and +1.4%,
overlapping, where 2a measured +0.2% and +0.8%. That is 2a's figure read in
other sittings rather than something 2b added: 2b's binary is level with
`4882a2c`, and the unchanged `4882a2c` itself sat +0.6% and +1.9% above
`be3eb40` in these two sittings, as the parents of the changes below sat +0.9%
to +2.3% above it in theirs. Every other source is level with `be3eb40` or
faster.

Two changes were measured against their parents on the way, each in a sitting
of its own, so their figures are not compared with these:

- **A closure's payload inline in its object** ([memory.md](memory.md), "What
  the collector traces"): `bench_block.ns` −4.4% and `bench_newarg.ns` −0.8%,
  both entirely below, the rest level. `MakeClosure` no longer allocates an
  `Rc` per closure, and a popped block activation no longer drops one. A
  release `Object` stays 56 bytes: the inline `ClosureData` was 24 bytes when
  this was measured and is 32 since closure conversion, within the payload's 48
  either way.
- **The safepoint poll.** Its first measurement found no source slower, and
  measured nothing: with an empty slow path in a release build, fat LTO had
  deleted the poll's call, then its branch and the flag's load
  ([memory.md](memory.md), "Where a collection can happen"). With the slow path
  serving the request by clearing the flag, `vm::run`'s release assembly
  compares the flag at every switch and every backward jump and calls the
  safepoint when it is set, and the re-measurement found no source slower than
  its parent: −3.0% on `bench_block.ns` to +0.9% on `bench_new2.ns`, every
  group overlapping. Cog's fallback, folding the request into the depth check,
  was not needed.

Host roots changed what `bench` times, slightly: the timed span now fetches the
send's arguments from their roots and roots its result, an `Rc` allocation and
a push onto the table against a run of milliseconds. Every after-figure above
includes it.

The suite (`cargo test --workspace`, debug build, test binaries already built)
takes 12.7 s, measured at 682 tests, against 12.5 s for 662 before the step. Under
`NS_NEXT_STRESS_MOVES=1` it takes 190.2 s: almost half of that is
`crates/ns-runtime/tests/trampoline.rs`'s depth-limit test, and most of the
rest the CLI tests that spawn `ns-next` per fixture or golden
([memory.md](memory.md), "The move stress mode").

### What 2c moved

Step 2c ([memory.md](memory.md), "The brand, checked by the compiler rather
than by convention") put one thing into a release build: the step state
behind `World::enter`, at the time entered once per instruction the run loop
executes ("The VM" above); "The loop over locals" below says what runs
between entries today. The brand itself is a zero-sized lifetime and changes no
layout: `Object`, `FrameSlot` and `Result<Step, Signal>` kept their release
sizes, 56, 136 and 56 bytes. The step was measured whole against the commit
it started from, `a5d1e05`, and against `be3eb40`, in two sittings on
2026-09-14, by the protocol of "What removing the re-entries moved" above,
every binary built with every function start pinned to a 64-byte boundary
and `+crt-static` — the protocol's flag set; "The pin" below says where the
rule comes from — the before-binaries in throwaway worktrees through
`RUSTFLAGS`, the head from a configuration that carried the alignment flag
at the time (it no longer does; the protocol above says why), `ns-next
bench <source> --iterations 30` over the same seven sources, five rounds,
alternating the binaries within each round. A fourth binary ran in the same
rounds — `a5d1e05` as its own configuration builds it, unpinned — to show
what the pin itself does. Medians in ms, and each group's mean, from the
first sitting:

| Source | `be3eb40`, rounds 1–5 | mean | `a5d1e05`, rounds 1–5 | mean | `a5d1e05` unpinned, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.95, 11.01, 11.06, 11.12, 11.22 | 11.07 | 11.20, 11.14, 11.24, 11.19, 11.21 | 11.20 | 11.15, 11.17, 11.19, 11.16, 11.21 | 11.17 | 11.30, 11.27, 11.26, 11.24, 11.21 | 11.26 |
| `bench_send.ns` | 13.63, 13.77, 13.74, 13.99, 13.73 | 13.77 | 14.08, 14.16, 14.17, 14.14, 14.03 | 14.12 | 13.89, 14.02, 14.15, 14.20, 14.00 | 14.05 | 14.04, 14.14, 14.38, 14.25, 14.19 | 14.20 |
| `bench_block.ns` | 23.01, 23.35, 23.21, 23.10, 23.11 | 23.16 | 22.60, 23.12, 22.62, 22.53, 22.52 | 22.68 | 22.40, 22.50, 22.49, 22.71, 22.88 | 22.59 | 22.56, 22.66, 22.61, 22.47, 22.98 | 22.66 |
| `bench_do.ns` | 7.86, 8.07, 8.08, 8.13, 8.07 | 8.04 | 7.72, 7.79, 7.70, 7.71, 7.78 | 7.74 | 7.80, 7.70, 7.73, 7.68, 7.73 | 7.73 | 7.73, 7.77, 7.79, 7.78, 7.71 | 7.76 |
| `bench_new.ns` | 7.34, 7.40, 7.38, 7.36, 7.43 | 7.38 | 7.37, 7.39, 7.37, 7.33, 7.58 | 7.40 | 7.25, 7.30, 7.32, 7.26, 7.38 | 7.30 | 7.30, 7.36, 7.27, 7.34, 7.34 | 7.32 |
| `bench_new2.ns` | 8.44, 8.36, 8.32, 8.28, 8.54 | 8.39 | 8.25, 8.40, 8.39, 8.31, 8.48 | 8.36 | 8.21, 8.23, 8.17, 8.23, 8.22 | 8.21 | 8.32, 8.22, 8.40, 8.31, 8.35 | 8.32 |
| `bench_newarg.ns` | 10.36, 10.50, 10.51, 10.56, 10.61 | 10.51 | 10.29, 10.34, 10.20, 10.23, 10.23 | 10.26 | 10.09, 10.04, 10.25, 10.10, 10.09 | 10.11 | 10.09, 10.15, 10.40, 10.08, 10.29 | 10.20 |

And from the second:

| Source | `be3eb40`, rounds 1–5 | mean | `a5d1e05`, rounds 1–5 | mean | `a5d1e05` unpinned, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.02, 11.03, 11.07, 11.10, 10.96 | 11.03 | 11.36, 11.16, 11.24, 11.19, 11.32 | 11.25 | 11.29, 11.26, 11.22, 11.20, 11.11 | 11.21 | 11.20, 11.25, 11.27, 11.18, 11.21 | 11.22 |
| `bench_send.ns` | 13.70, 13.83, 13.80, 13.77, 13.84 | 13.79 | 14.23, 14.06, 14.26, 14.11, 14.11 | 14.16 | 14.15, 13.97, 14.13, 13.99, 14.05 | 14.06 | 14.09, 14.15, 14.13, 14.07, 14.03 | 14.09 |
| `bench_block.ns` | 23.32, 23.17, 23.27, 23.31, 23.39 | 23.29 | 22.39, 22.54, 23.38, 23.03, 22.90 | 22.85 | 22.86, 22.60, 22.54, 22.94, 22.44 | 22.68 | 22.47, 22.50, 22.74, 22.53, 22.44 | 22.54 |
| `bench_do.ns` | 7.87, 7.94, 7.93, 8.11, 7.97 | 7.96 | 7.65, 7.79, 7.72, 7.75, 7.76 | 7.73 | 7.79, 7.89, 7.70, 7.70, 7.83 | 7.78 | 7.77, 7.83, 7.80, 7.74, 7.66 | 7.76 |
| `bench_new.ns` | 7.35, 7.43, 7.33, 7.43, 7.34 | 7.37 | 7.29, 7.40, 7.34, 7.34, 7.33 | 7.34 | 7.40, 7.23, 7.45, 7.27, 7.40 | 7.35 | 7.45, 7.35, 7.32, 7.31, 7.25 | 7.34 |
| `bench_new2.ns` | 8.43, 8.32, 8.32, 8.38, 8.26 | 8.34 | 8.31, 8.39, 8.23, 8.32, 8.21 | 8.29 | 8.21, 8.28, 8.30, 8.12, 8.34 | 8.25 | 8.35, 8.34, 8.30, 8.28, 8.28 | 8.31 |
| `bench_newarg.ns` | 10.40, 10.31, 10.40, 10.42, 10.49 | 10.41 | 10.28, 10.20, 10.30, 10.16, 10.38 | 10.26 | 10.09, 10.18, 10.04, 10.14, 10.05 | 10.10 | 10.14, 10.15, 10.20, 10.16, 10.27 | 10.18 |

The change of the after-group's mean in both sittings, and what the pin
itself does — the unpinned `a5d1e05` against the pinned one:

| Source | Against `a5d1e05`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 | `a5d1e05` unpinned against pinned, sitting 1 / 2 |
|---|---|---|---|
| `bench_loop.ns` | +0.5% / −0.3%, overlapping | +1.7% / +1.7%, after entirely above in the second | −0.2% / −0.4%, overlapping |
| `bench_send.ns` | +0.6% / −0.4%, overlapping | +3.1% / +2.2%, after entirely above in both | −0.4% / −0.7%, overlapping |
| `bench_block.ns` | −0.1% / −1.4%, overlapping | −2.2% / −3.2%, after entirely below in both | −0.4% / −0.8%, overlapping |
| `bench_do.ns` | +0.2% / +0.4%, overlapping | −3.5% / −2.5%, after entirely below in both | −0.1% / +0.6%, overlapping |
| `bench_new.ns` | −1.1% / +0.0%, overlapping | −0.8% / −0.5%, overlapping | −1.4% / +0.2%, overlapping |
| `bench_new2.ns` | −0.5% / +0.2%, overlapping | −0.8% / −0.4%, overlapping | −1.8% / −0.5%, unpinned entirely below in the first |
| `bench_newarg.ns` | −0.5% / −0.8%, overlapping | −2.9% / −2.1%, after entirely below in the second | −1.4% / −1.6%, overlapping |

**Every source is level with `a5d1e05`**: the after-group overlaps its
parent-group on all seven sources in both sittings, the change of the mean
between −1.4% and +0.6%. A zero-sized lifetime and an entry that inlines away
cost nothing at run time, as the design expected.

**Against `be3eb40`**, the after-binary's shift is the parent's, read again:
`a5d1e05` itself sits +2.5% and +2.7% above `be3eb40` on `bench_send.ns`
and +1.1% and +2.0% on `bench_loop.ns` in these sittings — 2a's shift on
those two sources ("What removing the re-entries moved" above) — and the
after-binary +3.1% / +2.2% and +1.7% / +1.7%, level with its parent. No
source's shift against `be3eb40` grew past what 2b left by more than either
group's spread: `bench_send.ns`, which 2b left at +0.9% to +1.4%, grew by
0.24 and 0.11 ms against `be3eb40`-group spreads of 0.36 and 0.14 ms, within
them, just so in the second sitting. `bench_block.ns` is 2.2% and 3.2%
faster than `be3eb40`, `bench_do.ns` 2.5–3.5%, `bench_newarg.ns` 2.1–2.9%,
and `bench_new.ns` and `bench_new2.ns` are level with it.

**What the pin itself does** is the last column: the unpinned `a5d1e05`
reads −0.1% to −1.8% against the pinned one, the groups overlapping on every
source in both sittings but one (`bench_new2.ns` in the first, the unpinned
group entirely below by 1.8%). So the unpinned binary is, if anything,
slightly faster: the pin fixes the phase of every function start, not the
phase the compiler's own layout gives the loop headers inside a function,
and `a5d1e05`'s hot functions happened to sit well. What the pin buys is
that a change elsewhere can no longer move them, which is the history below.
It costs about 110 KB of padding — 2,091,520 against 1,979,904 bytes, 5.6%,
some 4,600 functions each padded to the next boundary. Padding that buys
repeatability and no speed is why the shipped build does not carry it: the
pin belongs to the measurement protocol, where every compared binary gets
it through `RUSTFLAGS`, and `.cargo/config.toml` stays as it was.

**The pin.** Before it, the same step measured 1–2.5% slower on the block
and instantiation benchmarks, and finding out why is where the pin comes
from. Two sittings of the same protocol on 2026-09-14, with the binaries as
their configurations built them, put `bench_new2.ns` at +1.8% and +2.5%
against `a5d1e05`, entirely above in both, `bench_block.ns` at +2.5% / +1.8%
and `bench_newarg.ns` at +1.2% / +1.7%, each entirely above in one; the
loop, the send and `do:` level. A bisection sitting over the step's commits
— the four sources that moved, five rounds, the change of each group's mean
against `a5d1e05` — placed the shift on the brand commit:

| Source | `d27e40b` (the entry) | `7b41649` (the names written out) | `e59dc52` (hosts through `enter`) | `1549fb0` (the brand) | `2caf3e0` (no cast) |
|---|---|---|---|---|---|
| `bench_send.ns` | +0.7% | +0.8% | +0.6% | +1.6% | +1.0% |
| `bench_block.ns` | +0.5% | +1.1% | +1.4% | +2.8% | +3.1% |
| `bench_new2.ns` | −0.3% | +0.0% | +0.7% | +1.6% | +1.5% |
| `bench_newarg.ns` | +0.5% | +0.1% | +0.1% | +1.3% | +1.6% |

`e59dc52` to `1549fb0` is the one step between neighbours that separates
groups, though the brand commit's diff is types and signatures apart from
`rebrand` and small reshapes the lifetimes required, and its binary 512
bytes smaller. The post-LTO assembly of both commits, split into functions
and diffed, explained it: the hot path is instruction for instruction
identical — `Interpreter::run_from_host` with `vm::run` inlined (2,470
instructions), `builtins::dispatch_native` (3,082), `vm::finish`,
`Mutator::decide_send`, `push_step`'s block and method arms,
`Heap::push_activation`, `pop_activation` and `reify`, the lookups — and
only cold host code differs: the `for<'gc>` closure through
`Interpreter::enter` needs a smaller stack frame in the host entries —
`Interpreter::send`, ns-cli's `entry::instantiate_root` and its command
dispatcher shrink by 16, 16 and 256 bytes, `Interpreter::instantiate` grows
by two instructions — and the `CREATION_IN_PROGRESS` constant that became a
function stores 2 bytes where it stored 16, in `push_step`'s cold
nested-class arm (−16 bytes). The linker map showed every function behind
them moved by 224 or 240 bytes — 32 or 16 modulo 64 — so every hot entry
point, and with it every loop header and branch target in the run loop, sat
in a different 64-byte line than before. Rebuilding both commits with
`-C llvm-args=-align-all-functions=6` changed no instruction and brought
them level in two sittings (`bench_block.ns` from +1.5% / +1.8% to −0.3% /
−0.1%, `bench_send.ns` from +1.5% / +0.8% to +0.4% / +0.2%). That is the
mechanism, and the decision, 2026-09-14, is that the pin belongs to the
measurement protocol and to nothing else: a change to cold code must not
move the hot loop's phase *in a comparison*, so every binary a measurement
compares is built with the alignment flag through `RUSTFLAGS` ("What
removing the re-entries moved" above, the protocol), while the shipped
build stays unpinned, since the pin measured no faster and costs 5.6% of
the binary ("What the pin itself does" above). The tables above are that
measurement, every binary aligned.

One sitting of its own, so its figures are not compared with these: **the
per-instruction entry** (Task 1 of the step, `d27e40b`, the step state moved
behind `World::enter` with no lifetime yet), against `a5d1e05` in one
sitting on 2026-09-14, unpinned: level on every source, every group
overlapping its parent's — `bench_loop.ns` +0.1%, `bench_send.ns` −0.3%,
`bench_block.ns` −0.9%, `bench_do.ns` +0.4%, `bench_new.ns` +0.9%,
`bench_new2.ns` −0.0%, `bench_newarg.ns` +0.5%. So the entry inlines away,
as `#[inline(always)]` asks.

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 13.1 s at 691 tests, and 190.3 s under `NS_NEXT_STRESS_MOVES=1`,
where 2b left 12.7 s and 190.2 s ([memory.md](memory.md), "The move stress
mode").

### What spec-conformant lookup moved

The step that made name resolution and lookup follow the specification
([dispatch.md](dispatch.md), "Resolution", "Lookup" and "Slot accessors")
changed three things a release build runs whenever it decides a send. A slot's
accessors are entries of the method table, found by the ordinary lookups, so
the lexical-send slot pre-check is gone and with it the `Symbol::intern` of a
getter name on every setter-shaped lexical send ("What the profiles show"
above); in its place a lexical send asks N's own table for §5.9's private rule
before `lookupProtected` asks the receiver's. The explicit-receiver, lexical and
super lookups apply the specification's verdicts. And the method tables, the
nested-class slot maps and the interpreter's application caches hash their
identity keys with one multiply and a rotation instead of SipHash
([object-model.md](object-model.md), "Identity-keyed tables hash with
`hash::IdentityMap`").

It was measured whole against the commit it started from, `59061ec`, and
against `be3eb40`, in two sittings on 2026-09-15, by the protocol of "What
removing the re-entries moved" above: every binary built through `RUSTFLAGS`
with `+crt-static` and the function-alignment pin, the before-binaries in
throwaway worktrees, `ns-next bench <source> --iterations 30` over the seven
sources, five rounds, the order of the three binaries rotating from round to
round. Three of the four throwaway sources had to change. `bench_new.ns`,
`bench_new2.ns` and `bench_newarg.ns` declared a slot and a getter method of
one name in one class body (`| v ::= 1. |` beside `public v = ( ^v )`), which
the resolver now refuses as a duplicate member. Their slots are renamed
(`held`, `heldU`, `heldV`) and the methods keep their names and bodies, so every
binary does the same work, a method activation and a slot read per
instantiation. Making the slots public and dropping the methods instead would
have left both before-binaries, which answer no explicit-receiver slot send,
with a `doesNotUnderstand:`.

These are the three as every later measurement uses them, written under
`target\bench-lookup\` (ignored by git), and after them `bench_do.ns`, first
written out in the
[2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md),
"Measurement protocol". All four are spelled with the setter send, which every
compared binary back to `be3eb40` parses; the 2c plan's copy spells `:=`,
which the tree has refused since "M4 — Removing `:=`" ([PROGRESS.md](../PROGRESS.md)),
and is the same program. The three print `20000`:

```
class BenchNew = Object new (
	| sum ::= 0. |
) (
	class Cell = Object new (
		| held ::= 1. |
	) (
		public v = ( ^held )
	) : ()
	public main: p args: a = (
		| n |
		n:: 0.
		[n < 20000] whileTrue: [sum:: sum + Cell new v. n:: n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

```
class BenchNew2 = Object new (
	| sum ::= 0. |
) (
	class Base = Object new (
		| heldU ::= 1. |
	) (
		public u = ( ^heldU )
	) : ()
	class Cell = Base new (
		| heldV ::= 1. |
	) (
		public v = ( ^heldV )
	) : ()
	public main: p args: a = (
		| n |
		n:: 0.
		[n < 20000] whileTrue: [sum:: sum + Cell new v. n:: n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

```
class BenchNewArg = Object new (
	| sum ::= 0. |
) (
	class Base x: a = Object new (
		| heldU ::= a. |
	) (
		public u = ( ^heldU )
	) : ()
	class Cell = Base x: 1 (
		| heldV ::= 1. |
	) (
		public v = ( ^heldV )
	) : ()
	public main: p args: a = (
		| n |
		n:: 0.
		[n < 20000] whileTrue: [sum:: sum + Cell new u. n:: n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`bench_do.ns` prints `275000`:

```
class BenchDo = Object new (
	| sum ::= 0. |
) (
	public main: p args: a = (
		| n |
		n:: 0.
		[n < 5000] whileTrue: [{1. 2. 3. 4. 5. 6. 7. 8. 9. 10} do: [:e | sum:: sum + e]. n:: n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

Medians in ms, and each group's mean, from the first sitting:

| Source | `59061ec`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.22, 11.41, 11.45, 11.54, 11.42 | 11.41 | 11.39, 11.22, 11.10, 11.09, 11.23 | 11.21 | 11.29, 11.20, 11.32, 11.21, 11.19 | 11.24 |
| `bench_send.ns` | 14.51, 14.09, 14.26, 14.18, 14.17 | 14.24 | 13.73, 13.95, 13.98, 13.90, 14.05 | 13.92 | 12.65, 12.51, 12.36, 12.52, 12.31 | 12.47 |
| `bench_block.ns` | 22.61, 22.61, 22.99, 23.17, 22.93 | 22.86 | 23.76, 23.69, 23.39, 23.31, 23.37 | 23.50 | 21.29, 21.15, 21.43, 21.06, 21.09 | 21.20 |
| `bench_do.ns` | 7.74, 7.84, 7.76, 7.91, 7.95 | 7.84 | 7.94, 8.07, 8.30, 8.04, 8.01 | 8.07 | 7.43, 7.58, 7.83, 7.48, 7.41 | 7.54 |
| `bench_new.ns` | 7.41, 7.46, 7.36, 7.39, 7.45 | 7.41 | 7.42, 7.48, 7.58, 7.46, 7.35 | 7.46 | 6.83, 6.76, 6.92, 6.76, 6.77 | 6.81 |
| `bench_new2.ns` | 8.40, 8.37, 8.45, 8.37, 8.36 | 8.39 | 8.37, 8.39, 8.38, 8.39, 8.34 | 8.37 | 7.95, 7.83, 7.81, 7.77, 7.90 | 7.85 |
| `bench_newarg.ns` | 10.33, 10.33, 10.35, 10.43, 10.35 | 10.36 | 10.73, 10.62, 10.66, 10.57, 10.55 | 10.63 | 9.80, 9.87, 9.92, 10.03, 9.92 | 9.91 |

And from the second:

| Source | `59061ec`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | After, rounds 1–5 | mean |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.30, 11.34, 11.32, 11.43, 11.34 | 11.34 | 11.13, 11.13, 11.30, 11.30, 11.15 | 11.20 | 11.27, 11.33, 11.36, 11.33, 11.17 | 11.29 |
| `bench_send.ns` | 14.19, 14.29, 14.33, 14.30, 14.23 | 14.27 | 13.76, 13.90, 14.00, 13.81, 13.99 | 13.89 | 12.41, 12.46, 12.57, 12.53, 12.51 | 12.50 |
| `bench_block.ns` | 22.74, 23.07, 22.88, 22.78, 23.18 | 22.93 | 23.40, 23.39, 23.47, 23.31, 23.42 | 23.40 | 21.20, 21.16, 21.07, 21.18, 21.04 | 21.13 |
| `bench_do.ns` | 7.79, 8.01, 7.86, 7.75, 7.83 | 7.85 | 8.12, 8.08, 7.88, 7.96, 7.95 | 8.00 | 7.41, 7.46, 7.49, 7.44, 7.46 | 7.45 |
| `bench_new.ns` | 7.49, 7.46, 7.37, 7.47, 7.35 | 7.43 | 7.66, 7.55, 7.52, 7.63, 7.48 | 7.57 | 6.77, 6.82, 6.75, 6.85, 6.90 | 6.82 |
| `bench_new2.ns` | 8.36, 8.34, 8.35, 8.43, 8.30 | 8.36 | 8.30, 8.37, 8.43, 8.38, 8.35 | 8.37 | 7.84, 7.75, 8.06, 7.75, 7.76 | 7.83 |
| `bench_newarg.ns` | 10.27, 10.34, 10.30, 10.27, 10.28 | 10.29 | 10.72, 10.58, 10.66, 10.54, 10.70 | 10.64 | 9.76, 9.80, 9.87, 9.79, 9.79 | 9.80 |

The change of the after-group's mean in both sittings:

| Source | Against `59061ec`, sitting 1 / 2 | Against `be3eb40`, sitting 1 / 2 |
|---|---|---|
| `bench_loop.ns` | −1.5% / −0.5%, overlapping | +0.3% / +0.8%, overlapping |
| `bench_send.ns` | −12.5% / −12.4%, after entirely below in both | −10.4% / −10.1%, after entirely below in both |
| `bench_block.ns` | −7.3% / −7.8%, after entirely below in both | −9.8% / −9.7%, after entirely below in both |
| `bench_do.ns` | −3.8% / −5.1%, after entirely below in the second | −6.5% / −6.8%, after entirely below in both |
| `bench_new.ns` | −8.1% / −8.2%, after entirely below in both | −8.7% / −9.9%, after entirely below in both |
| `bench_new2.ns` | −6.4% / −6.3%, after entirely below in both | −6.2% / −6.4%, after entirely below in both |
| `bench_newarg.ns` | −4.3% / −4.7%, after entirely below in both | −6.8% / −7.9%, after entirely below in both |

**No source is slower than either commit, and six are faster than both.**
`bench_send.ns` gained most, 12.5% against `59061ec` and 10% against
`be3eb40`. Every one of its iterations makes a setter-shaped lexical send,
`self tick: sum`, which interned `tick` every time it ran. The others gained
3.8–9.9%, every after-group but one entirely below its before-group in both
sittings. `bench_loop.ns` makes only slot sends and native leaf sends, and it
is level with both. So the shift against `be3eb40` that 2a left on the send and
the loop, and that 2c carried ("What 2c moved" above), is gone: `bench_send.ns`
now sits 10% below `be3eb40` and `bench_loop.ns` level with it.

**The accessors alone were slower, and the hasher is what made the step
faster.** One sitting of its own, the same protocol, measured the step's last
two code commits against their parents. Its figures are not compared with the
tables above:

| Source | `3cf7e3e` mean | `431da35` mean | `7b1901c` mean | Accessors, `431da35` against `3cf7e3e` | Hasher, `7b1901c` against `431da35` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.87 | 14.02 | 11.26 | +18.1%, entirely above | −19.7%, entirely below |
| `bench_send.ns` | 14.55 | 15.46 | 12.44 | +6.3%, entirely above | −19.5%, entirely below |
| `bench_block.ns` | 23.47 | 24.21 | 21.26 | +3.1%, entirely above | −12.2%, entirely below |
| `bench_do.ns` | 8.13 | 8.53 | 7.44 | +5.0%, entirely above | −12.8%, entirely below |
| `bench_new.ns` | 7.62 | 7.96 | 6.84 | +4.5%, entirely above | −14.0%, entirely below |
| `bench_new2.ns` | 8.61 | 8.93 | 7.81 | +3.8%, entirely above | −12.6%, entirely below |
| `bench_newarg.ns` | 10.66 | 11.14 | 9.83 | +4.5%, entirely above | −11.8%, entirely below |

Making the accessors method-table entries (`431da35`) slowed every source,
`bench_loop.ns` most. Before it, a lexical slot send made one SipHash probe of
the application's slot map, and a setter-shaped one interned its getter name
first. After it, every lexical send made two probes of the method table: N's
for the private rule, then the receiver's for `lookupProtected`. The loop makes
300,000 slot sends, four getters and two setters an iteration, and 2.15 ms more
is about 7 ns a send, of the order of one SipHash probe of a 4-byte key. That
is an inference from what the change did; no profile was taken. The hasher
(`7b1901c`) made every probe a multiply, and took every source past where it
started.

**Sparing the second probe measured level, so the lexical path keeps it.**
When N is the receiver's class, which is the common case for a slot send,
`lookupProtected`'s first level is N and it answers what N's own table
already answered. Two variants reused the first probe, each measured against
`7b1901c` in one sitting of its own. The first passed the receiver's class to
the private-member probe, which answered a visible member when that class was
N. It came out level on every source, −0.4% to +2.2%, every group
overlapping, so with the hasher the second probe costs nothing measurable. The
second returned the probe's finding as a three-way enum. It measured 3.4–14.7%
slower on every source, entirely above on six. That variant changed only how a
result travelled between two functions, so the lexical send's decision is
sensitive to codegen. The inline caches replaced that decision on the hot
path, and their measurement closes the finding ("What the inline caches
moved" below).

The suite (`cargo test --workspace`, debug build, test binaries already
built) takes 12.7 s at 710 tests, and 189.9 s under `NS_NEXT_STRESS_MOVES=1`.

### What primitives as method-table entries moved

The step that made every builtin's native an entry of its class's method
table, found by the ordinary lookups instead of matched by
`selector.as_str()` ([dispatch.md](dispatch.md), "Primitives"), was measured
against the commit it started from, `883431b` ("start"), and against
`be3eb40`, by the protocol of "What removing the re-entries moved" above:
every binary — start's and `be3eb40`'s built in throwaway worktrees, the
current tree's built in place — through the same `RUSTFLAGS`, `+crt-static`
and the function-alignment pin; `ns-next bench <source> --iterations 30`
over the same seven sources, five rounds, the order of the three binaries
rotating from round to round; two whole sittings, each a complete pass of
the timing loop run one after the other in the same session, with nothing
else running on the machine during either.

Sitting 1, means in ms:

| Source | `be3eb40` mean | start mean | after mean | after − start | after − `be3eb40` | vs start | vs `be3eb40` |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.1225 | 11.1931 | 11.2503 | +0.51% | +1.15% | overlapping | overlapping |
| `bench_send.ns` | 13.8138 | 12.4300 | 12.4673 | +0.30% | −9.75% | overlapping | entirely below |
| `bench_block.ns` | 23.2705 | 21.2777 | 21.2112 | −0.31% | −8.85% | overlapping | entirely below |
| `bench_do.ns` | 7.9111 | 7.4094 | 7.3474 | −0.84% | −7.12% | overlapping | entirely below |
| `bench_new.ns` | 7.4628 | 6.8485 | 6.8266 | −0.32% | −8.53% | overlapping | entirely below |
| `bench_new2.ns` | 8.3601 | 7.7574 | 7.6929 | −0.83% | −7.98% | overlapping | entirely below |
| `bench_newarg.ns` | 10.5532 | 9.7983 | 9.7168 | −0.83% | −7.93% | overlapping | entirely below |

Sitting 2, means in ms:

| Source | `be3eb40` mean | start mean | after mean | after − start | after − `be3eb40` | vs start | vs `be3eb40` |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.1329 | 11.2409 | 11.3064 | +0.58% | +1.56% | overlapping | overlapping |
| `bench_send.ns` | 13.8035 | 12.4302 | 12.6491 | +1.76% | −8.36% | overlapping | entirely below |
| `bench_block.ns` | 23.4623 | 21.1425 | 21.2321 | +0.42% | −9.51% | overlapping | entirely below |
| `bench_do.ns` | 7.9702 | 7.4758 | 7.3564 | −1.60% | −7.70% | overlapping | entirely below |
| `bench_new.ns` | 7.4407 | 6.7809 | 6.6963 | −1.25% | −10.00% | overlapping | entirely below |
| `bench_new2.ns` | 8.3527 | 7.7316 | 7.7927 | +0.79% | −6.71% | overlapping | entirely below |
| `bench_newarg.ns` | 10.6841 | 9.8072 | 9.5929 | −2.18% | −10.21% | entirely below | entirely below |

**Both sittings agree: no source's after-group lies entirely above its
start-group in either one** (`bench_newarg.ns` is even entirely below
start in the second sitting — faster than start, not merely level). Against
`be3eb40`, six of the seven sources are faster by about 7–10% in both
sittings, entirely below throughout; `bench_loop.ns` is level, +1.15%/+1.56%,
its group overlapping in both. `bench_loop.ns` makes three native sends and
six lexical slot sends per iteration — 150,000 native sends and 300,000 slot
sends in all — and no method send or instantiation, so it is the source that
exercises the rebuilt decision path hardest with nothing else to gain from —
level with `be3eb40` there is this step's whole answer for that path, where
the other six sources also gain from lookup and hashing work earlier steps
already made faster.

**What the first shape of this decision cost, and what recovered it.** The
commit that put every native behind the ordinary lookup, `e4ec1bd`, first
measured 5.5% to 22.4% slower than start on all seven sources, in two
sittings — an order of magnitude past the 1–4% codegen jitter every earlier
step's measurement had allowed for, and reproduced in a bisection across the
step's four commits. `bench_loop.ns` was hit hardest, +20.9%/+20.4% against
start, consistent with the change removing a fast path only a native send
had used. The cause was not the extra method-table probe a builtin's send now
makes: priced directly with a diagnostic build that added a redundant
`black_box(methods.get(black_box(selector)))` to every level of every walk,
the extra probe cost 0.4% for nine of them a `bench_loop.ns` iteration makes,
about 0.05% each — the tables' hasher is a multiply and a rotation, not
SipHash, which is why. The cost was the shape every lookup's answer travelled
through on its way back to its sender:

- **A 64-byte settled enum on every send's return path.** Every decision —
  slot, method, nested class, primitive — answered `Settled { Answered(...),
  Found(Lookup) }`, 64 bytes against `Lookup`'s own 24, so a slot or method
  send that never touches a primitive still built, returned and re-matched the
  wider value. Removing it, so each of the five callers that can reach a
  `Lookup::Primitive` matches its `Lookup` directly, measured 6.5%
  (`bench_loop.ns`) and 5.1% (`bench_send.ns`) faster — the largest single
  piece (`8a05621`).
- **The lookup rule as a run-time value.** The three per-lookup verdict
  closures had become one shared `walk` taking a `Rule` at run time; naming
  the rule as a literal again, so each of the three lookups gets its own
  specialized walk with `Rule::verdict` folded into a constant test, measured
  2.0%/1.7% faster (`d295cac`).
- **`walk` stepping to the superclass before probing the level it stood
  on.** A lookup that answers where it starts — a slot accessor, a method the
  receiver's own class declares, a builtin's own primitive, which is most
  sends — paid an `Rc<MixinApplication>` clone and drop for a level it never
  read. Moving the step to the three places the search actually continues
  measured 2.5%/1.6% faster (`8eb6360`); this one predates the primitive step
  and would have cost the same regardless of it.
- **The decision split across a function boundary.** The three finishers
  marked `#[inline(always)]`, with their continuations broken out into
  `#[cold]` helpers so only the hot half duplicates into the senders, measured
  1.9%/2.6% faster (`0e71299`).
- **`doesNotUnderstand:` competing for room in the hot match.** Marking it and
  its `super` counterpart `#[cold]` measured a further 0.6–1.3% across three
  sources — the weakest of the five kept changes on its own numbers, but
  consistent in direction, and what turned `bench_send.ns` and `bench_do.ns`
  from entirely above start into overlapping it (`1abe627`).

Four further variants were measured and rejected: outlining the primitive
arm into `#[inline(never)]` helpers; letting the class-object branch finish
its own send instead of falling through an `Option`; `#[inline(always)]` on
the two lookup entry points themselves, which measured 6.6%/6.2% slower, the
worst of the four; and a batched heap read answering a lookup level's
application and its superclass together, which grows the walk's loop state
from two to three words and measured 2.0%/0.9% slower. What is on the send
path now is exactly the four kept changes above plus the one predating this
step; nothing about the design changed — a primitive still fails exactly as
"Primitives" ([dispatch.md](dispatch.md)) describes, into the same
`dispatch::lookup_above`.

**What the next step must not repeat.** 3b rebuilds this same decision path
to insert a cache in front of it, as `resolve` then `act`
([dispatch.md](dispatch.md), "3b — The caches"). A `resolve` that hands its
caller a decision through a settled enum the way this step's first shape did,
or a cache hit and miss sharing a function boundary the uncached path had
just been split out of, would pay the same 15–20% this step paid before it
was found and removed. 3b's own measurement watched for this shape
specifically, and met a relative of it: the split into `resolve` and `act`
pushed the run loop's step out of line ("What the inline caches moved"
below).

### What the inline caches moved

3b, the inline caches ([dispatch.md](dispatch.md), "3b — The caches"), was
measured in four parts, each against the commit it started from, and then
whole, against 3a's head (`49a2a38`, "start") and `be3eb40`. Every part used
the protocol of "What removing the re-entries moved" above: every binary built
with the same `RUSTFLAGS` (`+crt-static` and the function-alignment pin), the
before-binaries in throwaway worktrees; each binary checked to print the
expected answer for every source first;
`ns-next bench <source> --iterations 30` over the seven sources, five rounds,
the order of the binaries rotating from round to round; two whole sittings,
nothing else running. The import check and the inlining checks below used
`dumpbin` from the Visual Studio Build Tools, which is installed but not on
`PATH` — on this machine, found through `vswhere.exe` in
`C:\Program Files (x86)\Microsoft Visual Studio\Installer\`, at
`C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe`.
From Git Bash it must be invoked as `dumpbin -disasm`, not `/disasm`: MSYS
rewrites a `/option` argument into a Windows path, and the tool then silently
tries to dump a file called `C:\Program Files\Git\disasm`. `/dependents` on
every binary, and `/disasm` (or `-disasm`, from Git Bash) on a release copy
built with debug information to see which functions the optimizer kept whole.
That copy is built with the same `RUSTFLAGS` as the timed binary, plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir` so it does
not disturb the timed build: `CARGO_PROFILE_RELEASE_DEBUG=true cargo build
--release -p ns-cli --target-dir target-debuginfo`. "Entirely below" and
"entirely above" are the protocol's gate: all five after-medians below, or
above, all five of the group compared with.

**`feedback_base` on its own** (`f8fdcfa`, against its parent `401817a`).
Every activation carries the base, `vm::run` holds it in a local and
`Flow::Switch` carries it, and nothing reads it yet. Change of the mean,
sitting 1 / sitting 2:

| Source | vs parent | vs start | vs `be3eb40` |
|---|---|---|---|
| `bench_loop.ns` | +0.44% / −1.39% | +0.01% / −0.59% | +1.05% / +1.17% |
| `bench_send.ns` | +1.31% / +0.27% | +0.58% / +0.42% | −9.42% / −8.73% |
| `bench_block.ns` | +0.10% / −0.51% | −0.13% / −2.36% | −9.12% / −8.99% |
| `bench_do.ns` | −0.46% / −0.05% | −0.31% / +0.13% | −6.77% / −6.96% |
| `bench_new.ns` | +0.94% / +0.13% | −0.24% / +0.04% | −8.96% / −8.83% |
| `bench_new2.ns` | +1.30% / +1.33% | +0.76% / +0.88% | −6.37% / −7.00% |
| `bench_newarg.ns` | +1.02% / +1.51% | +1.39% / +2.21% | −8.31% / −8.08% |

No group lies entirely above its parent's in either sitting, so the field
was kept as designed — beside the activation's other header fields, loaded
by the switch that needs it — and none of the prepared variants (the base answered
by the push beside the activation, another place in the entry, `run` loading
it instead of `Flow::Switch` carrying it) was needed. `Result<Step, Signal>`
stayed 56 bytes: `Step::Method`'s base fits its padding.

**Resolve, then act** (`7d1f21d` and `1f4d6c6`, against `f8fdcfa`). The
split alone made every source 6.6–13.6% slower, entirely above in both
sittings. `dumpbin /disasm` showed why: before the split, the run loop's
per-instruction step, the lexical and `super` sends inlined into it, was part
of `run_from_host`; after it, the step was a function of its own,
`vm::run::closure$1`, so every instruction — not only every send — paid a
call and returned its `Flow` through memory. Four variants were measured
against the same parent, two sittings each:

| Variant | Change | `bench_loop.ns` | Sources entirely above the parent in both sittings |
|---|---|---|---|
| the split | the refactoring alone | +13.3% / +13.6% | all seven (+6.6% to +13.6%) |
| 1 | `#[inline(always)]` on the three resolves | +16.2% / +15.1% | all seven; the step still out of line, so this tested nothing |
| 2 | `#[inline(never)]` on `resolve_lexical` and `resolve_super` | +4.8% / +5.0% | `bench_loop.ns`, `bench_new2.ns` |
| 3, kept | `#[inline(never)]` on `decide_super_send`; the step inline again | +3.8% / +4.3% | `bench_loop.ns` |
| 4 | 3, `resolve_public` forced inline and the lexical fall-through behind a cold wrapper | +3.4% / +4.9% | `bench_loop.ns`, `bench_new.ns` |

Variant 3 against its parent, sitting 1 / 2: `bench_send.ns` +0.98% / +1.94%,
`bench_block.ns` +1.18% / +3.14%, `bench_do.ns` +1.43% / +2.26%,
`bench_new.ns` +1.28% / +2.32%, `bench_new2.ns` +2.32% / +1.67%,
`bench_newarg.ns` +1.02% / +0.82%; entirely above for `bench_loop.ns` in both
sittings, and for `bench_block.ns`, `bench_do.ns` and `bench_new2.ns` in one
sitting each. The gate was not met on `bench_loop.ns`, and the miss was
accepted and carried into the caches' own measurement: the split exists only
to put the caches between `resolve` and `act`, and the caches had to leave
`bench_loop.ns` no slower than 3a's head.

**The caches** (`ead0d85` and `a702875`, against `1f4d6c6`). On the caches'
first shape the step had fallen out of line again. Four reshapings were
tried and checked with `dumpbin` only, untimed, and each left
`vm::run::closure$1` a function of its own: `send_super` kept out of line;
the uncached fallbacks moved into cold functions; `materialize`'s level walk
moved into one out-of-line function; the handler arms and `Unhandled` routed
through one cold call. The step is called once, but its size exceeded even
the inliner's bonus for a single call site. Kept instead: `#[inline(always)]`
on the step closure, after which `vm::run` is 4,910 instructions with no call
per instruction. Means in ms, and the change of the after-mean, sitting 1 /
sitting 2:

| Source | parent | after | vs parent | vs start | vs `be3eb40` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.668 / 11.762 | 10.058 / 10.136 | −13.80% / −13.82% | −9.97% / −10.68% | −9.81% / −8.75% |
| `bench_send.ns` | 12.651 / 12.681 | 11.123 / 11.210 | −12.07% / −11.60% | −11.21% / −10.28% | −19.50% / −19.20% |
| `bench_block.ns` | 21.367 / 21.341 | 19.372 / 19.471 | −9.33% / −8.76% | −8.48% / −8.09% | −17.02% / −16.58% |
| `bench_do.ns` | 7.464 / 7.522 | 6.866 / 6.891 | −8.01% / −8.39% | −7.10% / −8.19% | −13.89% / −14.73% |
| `bench_new.ns` | 7.055 / 7.034 | 6.269 / 6.333 | −11.15% / −9.97% | −7.54% / −6.95% | −16.10% / −15.28% |
| `bench_new2.ns` | 7.934 / 7.893 | 7.289 / 7.297 | −8.13% / −7.56% | −5.58% / −5.04% | −12.42% / −12.82% |
| `bench_newarg.ns` | 9.919 / 9.888 | 9.148 / 9.145 | −7.77% / −7.51% | −5.81% / −5.58% | −14.79% / −13.74% |

Every source's after-group lies entirely below the parent's, start's and
`be3eb40`'s, in both sittings. The first shape of a cache — the design's —
was not timed, since its step was out of line.

**The step whole** (`c9c3c0f`, after invalidation and the stress mode), with
an attribution binary: start plus only the `#[inline(always)]` on the step
closure, and nothing of the caches. Means in ms:

| Source | `be3eb40` | start | attribution | after | after vs start | after vs `be3eb40` | attribution vs start |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.1697 / 11.1888 | 11.4004 / 11.3034 | 11.4019 / 11.2734 | 10.1160 / 10.0541 | −11.27% / −11.05% | −9.43% / −10.14% | +0.01% / −0.27% |
| `bench_send.ns` | 13.8400 / 13.8667 | 12.5251 / 12.5185 | 12.6298 / 12.5398 | 11.1194 / 11.1351 | −11.22% / −11.05% | −19.66% / −19.70% | +0.84% / +0.17% |
| `bench_block.ns` | 23.4688 / 23.3867 | 21.2699 / 21.1557 | 21.0996 / 21.2546 | 19.3142 / 19.5689 | −9.19% / −7.50% | −17.70% / −16.32% | −0.80% / +0.47% |
| `bench_do.ns` | 8.0435 / 7.9979 | 7.3997 / 7.3612 | 7.4823 / 7.4336 | 6.8773 / 6.9189 | −7.06% / −6.01% | −14.50% / −13.49% | +1.12% / +0.98% |
| `bench_new.ns` | 7.4893 / 7.5281 | 6.8036 / 6.7761 | 6.8229 / 6.8017 | 6.2731 / 6.3027 | −7.80% / −6.99% | −16.24% / −16.28% | +0.28% / +0.38% |
| `bench_new2.ns` | 8.3419 / 8.3924 | 7.7106 / 7.7195 | 7.8412 / 7.7087 | 7.3149 / 7.2420 | −5.13% / −6.19% | −12.31% / −13.71% | +1.69% / −0.14% |
| `bench_newarg.ns` | 10.6245 / 10.6709 | 9.6230 / 9.6057 | 9.6614 / 9.8064 | 9.1644 / 9.1958 | −4.77% / −4.27% | −13.74% / −13.82% | +0.40% / +2.09% |

**The step is 4.3–11.3% faster than 3a's head and 9.4–19.7% faster than
`be3eb40`**, every after-group entirely below both in both sittings.
`bench_loop.ns`, level with `be3eb40` before this step, is now clearly below
it.

**The forced inlining explains none of it.** The attribution binary is
within ±2.1% of start on every source, every group overlapping, the sign
changing between sittings on three sources: on 3a's code the optimizer inlined
the step anyway, so forcing it does nothing. The caches account for the whole
gain; the forcing is what keeps it, once the caches' hit path makes the step
too large for the inliner — the standing hazard [dispatch.md](dispatch.md)
records.

**The 16-byte entry** (an experiment measured against `c9c3c0f` and not
merged): an index into side
tables of methods and mixins instead of an `Rc`, which makes a `Target` 12
bytes and an entry 16. Change of the mean, sitting 1 / sitting 2:
`bench_loop.ns` +2.02% / +1.36%, `bench_send.ns` +0.84% / +2.33% (entirely
above in the second sitting), `bench_block.ns` +0.45% / +0.92%,
`bench_do.ns` +2.72% / +2.03%, `bench_new.ns` +0.22% / −1.42%,
`bench_new2.ns` +0.08% / +0.14%, `bench_newarg.ns` +0.06% / +0.14%; every other
group overlapping. No source is faster. A plausible cause, not verified: a
hit reads a second, separately allocated vector where the 24-byte entry
already holds the pointer. The side tables also grow with every genuine miss,
not with distinct methods, so a megamorphic site cycling through more
receiver shapes than the global cache holds grows them without bound until an
invalidation — a regime none of the seven sources reaches, and one the cache
stress mode cannot show, since it empties the tables at every safepoint. The user
decided to keep the 24-byte entry.

**What the caches hold** (`--cache-statistics`, a release build of
`c9c3c0f`). An entry is 24 bytes, a polymorphic block 96, a global-cache
entry 32, so the global cache is 32 KB whatever the program.

| Source | Sites (filled) | Global entries filled |
|---|---|---|
| `bench_loop.ns` | 15 (14) | 10 of 1024 |
| `bench_send.ns` | 15 (14) | 11 |
| `bench_block.ns` | 16 (15) | 12 |
| `bench_do.ns` | 12 (11) | 9 |
| `bench_new.ns` | 15 (14) | 12 |
| `bench_new2.ns` | 16 (15) | 13 |
| `bench_newarg.ns` | 16 (15) | 13 |
| `TestActor.ns`, `TextModule.ns` | 0 | 0 |
| `MirrorTestingModel.ns` | 1 (1) | 1 |
| `Ducts.ns`, `NewspeakASTs.ns`, `V5BytecodeSimulation.ns` | 2 (2) | 2 |

The corpus rows are each `corpus/runnable.toml` module run with its entry
selector. No site went polymorphic or megamorphic anywhere: every site these
programs reach sees one receiver id.

**Codegen sensitivity.** Spec-conformant lookup found a lexical send's
decision sensitive to how its result travelled, 3.4–14.7% for a variant that
changed nothing else ("What spec-conformant lookup moved" above). The cached
lexical path does not show it: `bench_send.ns`'s two after-sittings are
0.14% apart and `bench_block.ns`'s 1.3%, inside the jitter every measurement
allows, and a codegen-only change — the attribution binary — moved both by
less than 1%, every group overlapping start's. That closes the finding.

The suite (`cargo test --workspace`, debug build) passes under both stress
modes: about 3 min 9 s under `NS_NEXT_STRESS_MOVES=1` and about 1 min 5 s
under `NS_NEXT_STRESS_CACHES=1`, which invalidates at every safepoint but
moves nothing.

### What the step before 3c moved

The five refinements the user decided after 3b ([dispatch.md](dispatch.md),
"Decisions", "Decided by the user … built before 3c") landed together, and
were measured together and one at a time, on 2026-09-18, against the commit
the step starts from (`f99a29f`) and against `be3eb40`. The protocol is the
one above: every binary, the current tree included, built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the earlier commits in throwaway worktrees; no binary imports
`VCRUNTIME140.dll`; every binary checked to print the expected answer for
every source first; `ns-next bench <source> --iterations 30`, five rounds,
the order of the six binaries rotating from round to round; two whole
sittings, nothing else running. An eighth source joins the seven:
`benchmarks/micro/bench_megamorphic.ns`, added with the least-recently-used
groups, whose `value` site sees five receiver classes in turn and is the only
source that drives a megamorphic site — 50,000 iterations, the count the
other fixtures use.

**The step is 2.1–4.1% faster on the three instantiation sources and
0.2–3.6% slower on the other five.** Change of the mean against `f99a29f`,
sitting 1 / sitting 2, with the protocol's gate:

| Source | `f99a29f`, rounds 1–5 | mean | after, rounds 1–5 | mean | change | gate |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 9.91, 9.99, 9.93, 10.05, 9.89 | 9.955 | 10.13, 10.28, 10.29, 10.58, 10.30 | 10.317 | **+3.64% / +0.79%** | entirely above in both |
| `bench_send.ns` | 11.12, 11.01, 11.10, 11.06, 11.16 | 11.090 | 11.22, 11.25, 11.27, 11.28, 11.64 | 11.334 | **+2.20% / +2.37%** | entirely above in both |
| `bench_block.ns` | 19.19, 19.22, 19.43, 19.53, 19.44 | 19.362 | 19.44, 20.16, 19.48, 19.41, 19.83 | 19.664 | +1.56% / +1.61% | entirely above in the second |
| `bench_do.ns` | 6.89, 6.92, 6.84, 6.85, 6.81 | 6.862 | 6.88, 6.92, 7.25, 6.89, 6.92 | 6.973 | +1.62% / +0.19% | overlapping in both |
| `bench_new.ns` | 6.27, 6.25, 6.26, 6.25, 6.23 | 6.252 | 6.17, 6.19, 6.03, 6.03, 6.08 | 6.097 | **−2.49% / −2.05%** | entirely below in both |
| `bench_new2.ns` | 7.23, 7.16, 7.35, 7.24, 7.17 | 7.229 | 7.11, 7.04, 7.00, 7.18, 6.97 | 7.062 | **−2.31% / −2.40%** | entirely below in the second |
| `bench_newarg.ns` | 9.12, 9.26, 9.18, 9.22, 9.17 | 9.190 | 8.94, 8.85, 8.84, 8.93, 9.10 | 8.932 | **−2.81% / −4.10%** | entirely below in both |
| `bench_megamorphic.ns` | 16.51, 16.15, 16.19, 16.18, 16.25 | 16.254 | 16.37, 16.80, 16.82, 16.39, 16.77 | 16.628 | +2.30% / +1.78% | overlapping in both |

**Every source stays well below `be3eb40`**, the comparison carried through
every measurement up to step 3: `bench_loop.ns` −6.7% / −8.5%,
`bench_send.ns` −18.2% / −17.6%, `bench_block.ns` −15.4% / −16.1%,
`bench_do.ns` −13.0% / −14.0%, `bench_new.ns` −17.6% / −17.3%,
`bench_new2.ns` −15.5% / −15.1%, `bench_newarg.ns` −15.2% / −15.2%,
`bench_megamorphic.ns` −7.1% / −7.8%, every group entirely below in both
sittings.

**`vm::run`'s step is still inline.** `dumpbin /disasm` on a release build
with debug information, on all five binaries — the base, the three
intermediate checkpoints and the head — finds no `vm::run::closure` function
in any of them: `vm::run` is one function of 4,910 instructions at
`f99a29f`, then 4,860, 4,878, 4,878 and 4,926. It gained 16 instructions
across the whole step and never approached the cliff that splitting
`resolve` from `act` hit ("A standing hazard" in
[dispatch.md](dispatch.md)).

Each change was measured against its parent in the same two sittings, which
is what attributes the movement. Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `f99a29f` | after 1–3 | after 4 | after 5 | after 6 |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 11.060 / 11.123 | 9.955 / 10.093 | 9.972 / 9.973 | 10.181 / 10.304 | 10.201 / 10.176 | 10.317 / 10.173 |
| `bench_send.ns` | 13.855 / 13.753 | 11.090 / 11.072 | 11.164 / 11.062 | 11.415 / 11.358 | 11.301 / 11.363 | 11.334 / 11.334 |
| `bench_block.ns` | 23.247 / 23.356 | 19.362 / 19.297 | 19.440 / 19.399 | 19.492 / 19.570 | 19.678 / 19.564 | 19.664 / 19.607 |
| `bench_do.ns` | 8.013 / 8.012 | 6.862 / 6.879 | 7.053 / 7.107 | 7.174 / 7.126 | 7.140 / 7.153 | 6.973 / 6.893 |
| `bench_new.ns` | 7.400 / 7.375 | 6.252 / 6.228 | 6.001 / 6.022 | 6.007 / 6.059 | 6.042 / 6.026 | 6.097 / 6.100 |
| `bench_new2.ns` | 8.354 / 8.311 | 7.229 / 7.230 | 6.984 / 6.955 | 7.073 / 7.032 | 7.031 / 6.991 | 7.062 / 7.056 |
| `bench_newarg.ns` | 10.526 / 10.510 | 9.190 / 9.297 | 8.873 / 8.872 | 8.954 / 8.935 | 8.881 / 8.897 | 8.932 / 8.915 |
| `bench_megamorphic.ns` | 17.897 / 17.891 | 16.254 / 16.209 | 16.196 / 16.248 | 16.441 / 16.395 | 16.438 / 16.431 | 16.628 / 16.498 |

- **Flat metaclasses, the primary factory as an entry of the metaclass
  mixin's method table, and the resolver's class-side name check** (`a466c5d`,
  against `f99a29f`). **Instantiation is 3.3–4.6% faster**: `bench_new.ns`
  −4.02% / −3.31%, `bench_new2.ns` −3.40% / −3.80%, `bench_newarg.ns`
  −3.45% / −4.57%, all three entirely below in both sittings. `X new` is now
  an ordinary cached send of an entry found by the ordinary lookup, where it
  was a special case tested ahead of every lookup. Of the five sources that
  instantiate nothing in their loop, four are level — `bench_loop.ns`,
  `bench_send.ns`, `bench_block.ns` and `bench_megamorphic.ns`, −1.19% to
  +0.67%, one group entirely below. The fifth, `bench_do.ns`, is **2.78% /
  3.31% slower, entirely above in both sittings**, which nothing on its path
  explains: it neither instantiates nor sends to a class object in its loop.
  It is the uniform shift on unchanged executed code that this repository has
  recorded three times ("What 2c moved" and the two findings in "What
  removing the re-entries moved", both above), and the function-alignment
  pin, which removes a 64-byte phase shift, does not remove it.
- **The lexical global-cache key** (`da8beed`, against `a466c5d`). **This is
  the change that costs the send benchmarks**: `bench_loop.ns` +2.10% /
  +3.32% and `bench_send.ns` +2.25% / +2.68%, each entirely above in the
  second sitting; `bench_megamorphic.ns` +1.51% / +0.91%, entirely above in
  the second; the other five overlapping, +0.10% to +1.71%, none below. A
  control binary bisected it into its two halves; see "Which half of the
  lexical key costs the send benchmarks" below.
- **Call-site numbers off the wire** (`c9097d1`, against `da8beed`). Nothing
  beyond noise anywhere, −1.24% to +0.95%, every group overlapping except
  `bench_loop.ns` entirely below in the second sitting and `bench_newarg.ns`
  entirely below in the first. That is the expected answer: the change is in
  the encoder, the decoder and the opcode table, and a run from source
  touches none of it. The step's instruction count did not change at all,
  4,878 before and after.
- **The least-recently-used groups** (`393311f`, against `c9097d1`). Nothing
  beyond noise either, −0.25% to +1.23% on seven sources, every group
  overlapping; `bench_do.ns` is −2.34% / −3.65%, entirely below in the second
  sitting, which nothing on its path explains and which gives back most of
  what the flat-metaclass and factory change cost it. On
  `bench_megamorphic.ns`, the source added for this comparison, +1.16% /
  +0.41%, overlapping in both.

  **What that source can and cannot settle.** Its megamorphic site sends
  every hit to the global cache, so it measures the *probe*: one hash and one
  cache line, against 3b's two hashes at two independent places in the table.
  It cannot measure the *replacement* rule. `--cache-statistics` after a run
  reports 29 sites, 26 filled, one megamorphic, and 27 of 1024 global slots
  used, five of them the megamorphic site's own. With 27 keys over 512
  groups the expected number of collisions is under one, and a group would
  have to hold three keys before anything is evicted at all, so on the hot
  path every read almost certainly answers at the first slot and the
  replacement rule almost certainly never runs. Deciding least-recently-used
  against 3b's insert on numbers needs cache pressure — many more hot keys
  than any fixture has, or
  a debug build's small `NS_NEXT_GLOBAL_CACHE_ENTRIES` — and no measurement
  has supplied it, so the rule stands on Strongtalk's precedent
  ([dispatch.md](dispatch.md), "The global cache").

**What the caches hold** (`--cache-statistics`, the release build of the
step's head). Unchanged from 3b on the seven sources — 12–16 sites, 9–13
global entries, no site polymorphic or megamorphic — and, for the eighth:

| Source | Sites (filled) | Polymorphic blocks | Megamorphic sites | Global entries filled |
|---|---|---|---|---|
| `bench_megamorphic.ns` | 29 (26) | 0 of 1 allocated | 1 | 27 of 1024 |

The one block was allocated when the site went polymorphic and given back
when it went megamorphic, which is why the table holds one and uses none.

#### Which half of the lexical key costs the send benchmarks

The lexical key's commit does two separable things. **The key's shape**: the
global key gains a fourth component, N's application, so `GlobalEntry` and
`SendKey` each carry a `DispatchId` more, `probes`, `holds`, `get` and
`insert` each take an argument more, and `cached` and `miss` take a
`lexical_of` closure that answers it — all of which `vm::run`'s step inlines.
**The two probes**: `send_lexical_first` reads two global-cache rows before
it resolves, which is what the wider key makes safe, and which is the point
of the change.

A control binary separates them: the commit with the two probes cut out of
`send_lexical_first` and everything else kept. `send_lexical_first` is
`#[inline(never)]`, so the cut cannot touch the step — and does not:
`vm::run` is 4,878 instructions in the control and in the commit, and the two
functions' instruction streams are identical once relocated addresses are
normalized, against 4,860 in the parent. Anything the control measures
against the commit is therefore the probes and code placement; anything it
measures against the parent is the 18 instructions the step gained. Two
sittings, all three binaries in each, the same protocol.

| Source | control vs parent (the key's shape) | commit vs control (the two probes) | commit vs parent (both) |
|---|---|---|---|
| `bench_loop.ns` | **+1.89% / +1.48%**, above in the second | −0.07% / +0.04%, overlapping in both | +1.82% / +1.52% |
| `bench_send.ns` | **+2.25% / +1.66%**, above in both | +0.41% / +0.57%, overlapping in both | +2.67% / +2.23% |
| `bench_block.ns` | +0.10% / +0.21%, overlapping | +1.23% / +0.49%, overlapping in both | +1.33% / +0.70% |
| `bench_do.ns` | −2.18% / +0.34% | +3.45% above / +1.45% overlapping | +1.20% / +1.80% |
| `bench_new.ns` | +1.31% / +0.39%, overlapping | +0.47% / +1.38%, overlapping in both | +1.78% / +1.77% |
| `bench_new2.ns` | +0.34% / +0.44%, overlapping | −0.08% / −0.30%, overlapping in both | +0.26% / +0.13% |
| `bench_newarg.ns` | +0.46% / −0.00%, overlapping | +0.24% / +0.46%, overlapping in both | +0.70% / +0.45% |
| `bench_megamorphic.ns` | **+1.00% / +1.54%**, above in both | +0.52% / −0.38%, overlapping in both | +1.52% / +1.16% |

**The key's shape carries the cost; the probes carry none of it.** The
control alone reproduces the whole of `bench_loop.ns`'s loss and most of
`bench_send.ns`'s and `bench_megamorphic.ns`'s, and adding the probes on top
of it moves no source beyond the noise. That confirms by measurement what the
site counts only argued: the probes run once per still-vacant lexical site,
and no source fills more than sixteen sites in a 50,000-iteration run.

So the mechanism is the widened key inside the inlined step, not work
executed per send — the step's 18 extra instructions, on a path every send
runs. It is not entry size: `GlobalEntry` is 32 bytes and `CacheEntry` 24 at
the parent, at the control and at the commit alike, the fourth component
having fitted in padding the struct already had. Two limits worth stating.
The control also deletes a cold function, `global_hit`, which no longer has a
caller, so code placement differs between the control and the commit as well
as the probes; the conclusion rests on that pair measuring *null*, which a
placement effect could in principle have cancelled, and on the parent
comparison measuring positive where the step demonstrably differs. And
"the widened key" is still four things at once — the wider entry, the wider
key, the extra arguments and the extra closure parameter — which no further
binary separates.

The step's head no longer has the fourth component as a separate field: the
least-recently-used groups folded it into the rule as
`CacheRule::Lexical(DispatchId)` and gave `GlobalEntry` its field back, at no
measured cost either way. What the control measured is the cost of the key
knowing N at all, in whichever of the two shapes it is carried.

### What 3c moved

3c moved the dispatch id a guard reads from the class object into the object
header ([dispatch.md](dispatch.md), "3c — The dispatch id in the object
header"). Two variants were measured on 2026-09-18 against `393311f`, the
last commit before 3c that changed a send's path, and against `be3eb40`:
**C2** (`681126d`), the id in the header, and **C2+** (`5d3aeff`), the id in
the header with `ClassData` boxed out of `ObjectData`, which is what was
kept. `fbd0bef`, the commit the step started from, ran beside them. The
protocol is the one above: every binary, the current tree included, built
with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the earlier commits in throwaway worktrees; no binary imports
`VCRUNTIME140.dll`; every binary checked to print the expected answer for all
eight sources first, `be3eb40` included; `ns-next bench <source>
--iterations 30`, five rounds, the order of the five binaries rotating from
round to round; two whole sittings, nothing else running.

**C2+ is level with `393311f` on every source.** Change of the mean,
sitting 1 / sitting 2, with the protocol's gate; the rounds are sitting 1's
medians in ms:

| Source | `393311f`, rounds 1–5 | mean | C2+, rounds 1–5 | mean | change | gate |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.59, 10.44, 10.58, 10.55, 10.66 | 10.563 | 10.57, 10.47, 10.34, 10.33, 10.35 | 10.413 | −1.41% / −2.00% | overlapping in both |
| `bench_send.ns` | 11.58, 11.57, 11.58, 11.76, 11.72 | 11.641 | 11.51, 11.61, 11.56, 11.50, 11.59 | 11.554 | −0.75% / −2.78% | entirely below in the second |
| `bench_block.ns` | 20.32, 20.16, 20.20, 20.25, 20.45 | 20.276 | 20.24, 20.39, 20.29, 20.25, 20.29 | 20.291 | +0.07% / +0.15% | overlapping in both |
| `bench_do.ns` | 7.01, 7.04, 7.09, 7.31, 7.10 | 7.110 | 6.99, 7.15, 6.99, 7.04, 7.13 | 7.060 | −0.70% / −1.94% | overlapping in both |
| `bench_new.ns` | 6.30, 6.21, 6.14, 6.22, 6.29 | 6.234 | 6.16, 6.16, 6.15, 6.16, 6.21 | 6.167 | −1.07% / −0.85% | overlapping in both |
| `bench_new2.ns` | 7.17, 7.28, 7.17, 7.15, 7.26 | 7.207 | 7.19, 7.13, 7.26, 7.21, 7.12 | 7.181 | −0.36% / −0.97% | overlapping in both |
| `bench_newarg.ns` | 9.20, 9.12, 9.13, 9.16, 9.20 | 9.161 | 9.10, 9.09, 9.19, 9.20, 9.06 | 9.127 | −0.37% / −2.09% | overlapping in both |
| `bench_megamorphic.ns` | 16.94, 17.25, 17.26, 17.35, 17.12 | 17.183 | 16.67, 16.66, 16.75, 16.68, 16.94 | 16.740 | −2.58% / −1.62% | entirely below in the first |

The means moved down in 14 of the 16 cells, by 0.4–2.8%, but **no source is
entirely below `393311f` in both sittings**, which is what the protocol
accepts as a real change; the two single-sitting results do not repeat in
the other sitting. C2 is level too: −0.25% to −2.44% in 15 of 16 cells and
+0.30% on `bench_do.ns`'s first sitting, entirely below only on
`bench_megamorphic.ns` in the first sitting. `fbd0bef` is level with
`393311f` everywhere, −1.29% to +1.62%, every group overlapping.

Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `393311f` | `fbd0bef` | C2 | C2+ |
|---|---|---|---|---|---|
| `bench_loop.ns` | 11.461 / 11.436 | 10.563 / 10.575 | 10.685 / 10.561 | 10.488 / 10.428 | 10.413 / 10.363 |
| `bench_send.ns` | 14.215 / 14.167 | 11.641 / 11.811 | 11.786 / 11.756 | 11.612 / 11.588 | 11.554 / 11.483 |
| `bench_block.ns` | 23.915 / 23.901 | 20.276 / 20.221 | 20.450 / 20.549 | 20.020 / 19.970 | 20.291 / 20.252 |
| `bench_do.ns` | 8.241 / 8.263 | 7.110 / 7.185 | 7.150 / 7.176 | 7.131 / 7.009 | 7.060 / 7.046 |
| `bench_new.ns` | 7.651 / 7.653 | 6.234 / 6.252 | 6.247 / 6.231 | 6.193 / 6.133 | 6.167 / 6.199 |
| `bench_new2.ns` | 8.620 / 8.542 | 7.207 / 7.221 | 7.223 / 7.289 | 7.161 / 7.149 | 7.181 / 7.151 |
| `bench_newarg.ns` | 10.932 / 10.885 | 9.161 / 9.243 | 9.209 / 9.175 | 9.061 / 9.071 | 9.127 / 9.049 |
| `bench_megamorphic.ns` | 18.408 / 18.292 | 17.183 / 17.059 | 16.962 / 17.031 | 16.794 / 16.775 | 16.740 / 16.783 |

**Every source stays well below `be3eb40`**, every group entirely below in
both sittings. C2+: `bench_loop.ns` −9.1% / −9.4%, `bench_send.ns` −18.7% /
−19.0%, `bench_block.ns` −15.2% / −15.3%, `bench_do.ns` −14.3% / −14.7%,
`bench_new.ns` −19.4% / −19.0%, `bench_new2.ns` −16.7% / −16.3%,
`bench_newarg.ns` −16.5% / −16.9%, `bench_megamorphic.ns` −9.1% / −8.3%.

**`vm::run`'s step is still inline.** `dumpbin /disasm` on release builds
with debug information finds no `vm::run::closure` function in `393311f`, C2
or C2+: `vm::run` is one function of 4,926 instructions at `393311f` (the
count "What the step before 3c moved" recorded for the same commit), 4,863
at C2 and 4,878 at C2+.

**Sizes, and what they cost.** `size_of::<Object>()`, the amplification a
crafted image can reach at the graph section ([memory.md](memory.md);
record minima 6 and 18 bytes, a frame-arena entry 136 bytes in release and
160 in debug, unchanged by 3c), and the peak working set (`PeakWorkingSet64`):

| Binary | `Object`, release / debug | Amplification, release / debug | Peak working set, `run-corpus` | Peak working set, `bench_block.ns --iterations 30` |
|---|---|---|---|---|
| `393311f` | 56 B / 96 B | 16.9x / 24.9x | 4.598 MB | 454.699 MB |
| C2 | 64 B / 104 B | 18.2x / 26.2x | 4.598 MB | 466.559 MB |
| C2+ | 56 B / 80 B | 16.9x / 22.2x | 4.590 MB | 454.727 MB |

The sizes and amplification figures in this table are the measurement's
record; the current ones are in [memory.md](memory.md), "What an object
costs" and "The amplification bound of a rejected load".

C2 costs `bench_block.ns`, which allocates heavily, 11.9 MB (2.6%) at its
peak, its 8 bytes an object; C2+ is within 0.03 MB of `393311f`. Boxing
`ClassData` gave back exactly what the header's id took in release, and more
than that in debug, where every handle `ClassData` held also carries its
heap's id and a move epoch. `run-corpus` is too small to show either.

**What the protocol cannot see.** The interpreter's guard is one step among
the run loop's dispatch, decoding and bookkeeping, and a load more or less
before the compare is lost in them. The difference the header makes is to a
compiled guard: one compare against the receiver at a fixed offset, against
two dependent loads, a check that the class is a class object and a branch
for the class side — which is why the header was kept on a measurement that
shows it level ([dispatch.md](dispatch.md), "3c — The dispatch id in the
object header").

### What the specification gaps moved

The specification gaps ([PROGRESS.md](../PROGRESS.md), "M4 — The
specification gaps") touch a send's compiled path in two places: a
parameter's or an immutable local's register is never copied any more
(Task 4, `ff1657d`, riding on Task 3's resolver guarantee), and a symbol
literal now names the heap's one canonical object for its text instead of a
fresh `Symbol` per occurrence (Tasks 5–6, `7135854`, `67ba5c0`, `abd9bb4`).
The other two gaps — the §5.7 member set keyed by a declaration instead of a
class's simple name (Task 1, `df0f347`) and passing a setter send by a
parameter or an immutable local (Task 3, `73921e1`), which the local-slot
declaration fix (Task 2, `717300a`) sits beside — are resolver changes,
ahead of the run loop, and change nothing a send executes.

Measured 2026-09-19: the branch head `abd9bb4` against `22a56f7`, the merge
of 3c, with `be3eb40` carried as the reference. Three throwaway detached
worktrees, one each for `22a56f7`, `be3eb40` and `abd9bb4`; every binary
built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
none imports `VCRUNTIME140.dll` (`dumpbin /dependents`); every binary checked
to print the expected answer for all eight sources first, exit 0 everywhere.
`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the three binaries rotating by one position from round to round; two
whole sittings, nothing else running. None of the eight sources declares a
local slot with an initializer or writes a parameter, so Tasks 2–3 change
nothing any of them does and all eight stay comparable with earlier
measurements.

**The head is level with `22a56f7` on every source.** Sitting 1's medians in
ms, both means, the change of the head's mean in sitting 1 / sitting 2, and
the gate in sitting 1 / sitting 2:

| Source | `22a56f7`, rounds 1–5 | mean | head, rounds 1–5 | mean | change | gate |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.59, 10.39, 10.39, 10.16, 10.14 | 10.336 | 10.43, 10.19, 10.37, 10.41, 10.32 | 10.342 | +0.06% / +1.43% | overlapping in both |
| `bench_send.ns` | 11.48, 11.46, 11.52, 11.32, 11.45 | 11.447 | 11.54, 11.55, 11.60, 11.59, 11.62 | 11.578 | +1.15% / +0.51% | entirely above in the first, overlapping in the second |
| `bench_block.ns` | 19.80, 20.28, 19.95, 20.59, 20.11 | 20.146 | 19.60, 20.24, 20.27, 20.19, 20.42 | 20.146 | +0.00% / +0.21% | overlapping in both |
| `bench_do.ns` | 6.92, 7.07, 6.95, 6.99, 7.17 | 7.020 | 7.02, 6.96, 6.91, 7.03, 7.02 | 6.988 | −0.45% / +1.10% | overlapping in both |
| `bench_new.ns` | 6.15, 6.13, 6.13, 6.24, 6.26 | 6.183 | 6.17, 6.26, 6.27, 6.20, 6.18 | 6.218 | +0.58% / −0.88% | overlapping in both |
| `bench_new2.ns` | 7.17, 7.10, 7.22, 7.27, 7.09 | 7.170 | 7.19, 7.17, 7.18, 7.16, 7.19 | 7.179 | +0.13% / +0.86% | overlapping in both |
| `bench_newarg.ns` | 9.11, 9.11, 9.09, 9.15, 9.12 | 9.116 | 9.07, 8.88, 9.16, 9.10, 9.06 | 9.051 | −0.71% / +0.52% | overlapping in both |
| `bench_megamorphic.ns` | 16.66, 16.69, 16.70, 16.91, 16.74 | 16.742 | 16.66, 16.87, 16.79, 16.76, 16.84 | 16.783 | +0.25% / +0.13% | overlapping in both |

No source is entirely above `22a56f7` in both sittings — the protocol's rule
for a real regression — and none is entirely below in either. The one
single-sitting result, `bench_send.ns` entirely above in sitting 1 (+1.15%),
does not repeat in sitting 2 (+0.51%, overlapping). The means moved by
−0.88% to +1.43%, ten of the sixteen cells up, all within the spread the
protocol treats as level.

Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `22a56f7` | head `abd9bb4` |
|---|---|---|---|
| `bench_loop.ns` | 11.290 / 11.252 | 10.336 / 10.250 | 10.342 / 10.397 |
| `bench_send.ns` | 14.090 / 13.986 | 11.447 / 11.491 | 11.578 / 11.550 |
| `bench_block.ns` | 23.743 / 23.569 | 20.146 / 19.823 | 20.146 / 19.865 |
| `bench_do.ns` | 8.132 / 8.057 | 7.020 / 6.932 | 6.988 / 7.008 |
| `bench_new.ns` | 7.556 / 7.444 | 6.183 / 6.146 | 6.218 / 6.092 |
| `bench_new2.ns` | 8.507 / 8.372 | 7.170 / 7.053 | 7.179 / 7.114 |
| `bench_newarg.ns` | 10.751 / 10.582 | 9.116 / 8.980 | 9.051 / 9.026 |
| `bench_megamorphic.ns` | 18.296 / 18.077 | 16.742 / 16.554 | 16.783 / 16.576 |

**Every source stays well below `be3eb40`**, every group entirely below in
both sittings, for the head and for `22a56f7`:

| Source | head vs `be3eb40` | `22a56f7` vs `be3eb40` |
|---|---|---|
| `bench_loop.ns` | −8.40% / −7.60% | −8.46% / −8.90% |
| `bench_send.ns` | −17.82% / −17.42% | −18.75% / −17.84% |
| `bench_block.ns` | −15.15% / −15.71% | −15.15% / −15.89% |
| `bench_do.ns` | −14.06% / −13.02% | −13.67% / −13.97% |
| `bench_new.ns` | −17.70% / −18.16% | −18.17% / −17.43% |
| `bench_new2.ns` | −15.61% / −15.03% | −15.72% / −15.76% |
| `bench_newarg.ns` | −15.81% / −14.71% | −15.20% / −15.15% |
| `bench_megamorphic.ns` | −8.27% / −8.30% | −8.49% / −8.43% |

**`vm::run`'s step is still inline.** Release builds with debug information
of `22a56f7` and `abd9bb4` (the same `RUSTFLAGS`, plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`, "What the
inline caches moved" above), then `dumpbin /disasm`: no `vm::run::closure`
function in either. `ns_runtime::vm::run` is one function of **4,878
instructions at `22a56f7`** (the count "What 3c moved" records for C2+,
`5d3aeff`) and **4,878 at `abd9bb4`**. With relocated addresses normalized
the two streams differ in 192 lines, all of them displacements: the head's
`rbx`-relative field offsets are 0x18 higher from `+0A8h` on — the heap's new
canonical-symbol-table field — and some absolute data addresses moved; no
instruction was added, removed or changed in kind.

**The copy elision measured no gain on these eight sources.** None of the
eight declares a local slot with an initializer or writes a parameter, so
Task 4's change never fires on any of them; the two constructs it touches are
exercised by the golden suite, not by the benchmarks. What the numbers show
is that the four gaps together cost nothing on a send's hit path, not that
the elision itself is worth anything measured here.

### What closure conversion moved

Closure conversion ([PROGRESS.md](../PROGRESS.md), "M4 — Closure conversion")
changes what every block creation and every block activation does, and it
narrows the protective register copies the lowering emits, so it was measured
in two steps against one baseline: the conversion on its own, then the copies
on top of it.

Measured 2026-09-20. Four binaries, each `cargo build --workspace --release`
with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the three earlier commits in throwaway worktrees:

- `efac7d2` — the `master` the branch started from (the merge of
  `feature/remove-assign`);
- `be3eb40` — the reference carried since step 2c;
- `a3f36a2` — the conversion alone (`feat: convert closures in the optimized
  lowering`);
- `fbd7d9f` — the head, the conversion plus the narrowed protective copies.

The protocol is the one of "What removing the re-entries moved": no binary
imports `VCRUNTIME140.dll` (`dumpbin /dependents`); every binary was checked to
print the expected answer for all eight sources before anything was timed;
`ns-next bench <source> --iterations 30`, five rounds per sitting, the four
binaries' order rotating by one position from round to round, two whole
sittings, nothing else running — 320 timed data lines.

**Where the four out-of-tree sources came from.** `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` were copied from "What
spec-conformant lookup moved" above, not from the
[2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md)'s
measurement protocol, whose copies spell the retired `:=` and no longer parse —
and three of which also declare a slot and a getter of the same simple name,
which the resolver now refuses as a duplicate member (§5.7). That substitution
is what every measurement since the spec-conformant-lookup step has already
made; the next one should take all four from execution.md too.

**Level everywhere.** Means in ms, sitting 1 / sitting 2:

| Source | `be3eb40` | `efac7d2` | `a3f36a2` (conversion) | `fbd7d9f` (head) |
|---|---|---|---|---|
| `bench_loop.ns` | 11.185 / 11.271 | 10.257 / 10.216 | 10.336 / 10.392 | 10.475 / 10.323 |
| `bench_send.ns` | 13.913 / 13.924 | 11.410 / 11.510 | 11.459 / 11.503 | 11.489 / 11.470 |
| `bench_block.ns` | 23.405 / 23.432 | 19.848 / 19.863 | 19.727 / 19.792 | 19.933 / 19.785 |
| `bench_megamorphic.ns` | 17.945 / 17.967 | 16.538 / 16.501 | 16.655 / 16.583 | 16.670 / 16.555 |
| `bench_do.ns` | 8.094 / 8.114 | 6.970 / 7.007 | 7.016 / 6.855 | 6.949 / 7.030 |
| `bench_new.ns` | 7.493 / 7.509 | 6.115 / 6.124 | 6.185 / 6.179 | 6.178 / 6.117 |
| `bench_new2.ns` | 8.506 / 8.403 | 7.135 / 7.117 | 7.163 / 7.090 | 7.115 / 7.171 |
| `bench_newarg.ns` | 10.641 / 10.664 | 8.997 / 9.014 | 9.057 / 9.049 | 9.039 / 9.102 |

The gate is the protocol's: a source has moved only when its five-round range
(min to max of the five medians) lies entirely above, or entirely below, the
baseline's in **both** sittings.

- **The conversion against `efac7d2`**: −2.18% to +1.73%, every source
  overlapping in both sittings but one — `bench_do.ns`, entirely below in the
  second sitting only (−2.18%), which does not repeat in the first (+0.67%).
  No source is entirely above. The conversion is level with `efac7d2` on all
  eight.
- **The copies against the conversion**: −1.00% to +2.56%, every source
  overlapping in both sittings, `bench_block.ns` — the one source that
  exercises a write-after-capture block every iteration — included. Narrowing
  the protective copies moved nothing measurable.
- **The head against `efac7d2`**: −0.39% to +2.12%, every source overlapping in
  both sittings. **No regression.**
- **The head against `be3eb40`**: every source entirely below in both sittings,
  by **6–19%** — `bench_loop.ns` −6.4% / −8.4%, `bench_send.ns` −17.4% /
  −17.6%, `bench_block.ns` −14.8% / −15.6%, `bench_megamorphic.ns` −7.1% /
  −7.9%, `bench_do.ns` −14.2% / −13.4%, `bench_new.ns` −17.6% / −18.5%,
  `bench_new2.ns` −16.4% / −14.7%, `bench_newarg.ns` −15.1% / −14.7%. Every
  gain accumulated since step 2c is still there; the conversion gave none of it
  back.

**What the design expected and what was measured.** "Cost", under "Closure
conversion" below, predicted that a block would stop paying a hop per nesting
level and that an activation would stop copying its registers out. Both happen,
and neither shows in the running time of any of these eight sources. The
profile of the trampoline's head ("What the profiles show" above, the
`bench_block.ns` head column) is the reason to expect little: it put reifying a
frame at 12.5% of the timed samples and popping, the divorce included, at
13.1%, and the conversion removes neither the reification nor the divorce —
only the register copy inside the divorce, and only for an activation no
by-reference closure can read. What is recorded
here is that the conversion is free, not that it is faster.

**`bench_block.ns`'s peak working set rose about 2.6%.** Measured as
[memory.md](memory.md)'s "The interim cost, and the one question still open"
records it — Windows
11, release build, `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize` through
`psapi!GetProcessMemoryInfo` on the still-open handle immediately after the
process exits, `bench bench_block.ns --iterations 30`, three runs each:

| Binary | Three-run band |
|---|---|
| `efac7d2` | 454.719–454.754 MB |
| `a3f36a2` (conversion only) | 466.406–467.598 MB |
| `fbd7d9f` (head) | 466.418–466.469 MB |

**The rise belongs to the conversion.** `a3f36a2`'s band lies entirely above
`efac7d2`'s, already at the head's level, and `fbd7d9f`'s band sits inside
`a3f36a2`'s, so the copy-narrowing adds and removes nothing here. The
explanation — a block that writes a captured local now allocates an
indirection-vector `Array` per activation of the body that declares it, in an
arena that reclaims nothing — **is an inference from what changed, not a
measured attribution**: no allocation profile was taken, and the numbers are
consistent with it without singling it out. It is a cost the collector (step 5)
removes; until then a vector is retained exactly as every other object the
process has ever made.

**`vm::run`'s step is still inline.** Release builds with debug information of
`efac7d2` and the head (the same `RUSTFLAGS`, plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`), then
`dumpbin /disasm`: no `vm::run::closure` function in either.
`ns_runtime::vm::run` is one function of **4,878 instructions at `efac7d2`**
and **5,700 at the head**, +16.9% — the three vector instructions and the
two-mode block-creation logic, all still inlined into the one function rather
than split out. That is what the step's own size did; it cost none of the
eight sources any measurable time, and nothing here should be read as saying it
did.

### What the moving collector moved

The moving collector ([PROGRESS.md](../PROGRESS.md), "M4 — The moving
collector"; [memory.md](memory.md), "The shape: mark, compact, forward"
onward) marks from the roots through `trace.rs`'s one edge enumeration,
compacts by sliding each arena's live slots to the front, truncates and
shrinks the arena with hysteresis, and rewrites every handle through a flat
forwarding table; the canonical symbol table becomes weak; a trigger compares
an arena's length against a limit in `push_object` and `reify` and requests a
collection at the next safepoint; and `NS_NEXT_STRESS_COLLECT` collects at
every safepoint as a third gate. `bench --collector-statistics` reports the
counts and pauses the way `--cache-statistics` reports the caches. This is
what ends the arena's never-reclaim interim ([memory.md](memory.md), "The
interim cost, and the one question still open"), and it was measured against
the expectation recorded in [memory.md](memory.md), "How the collector is
measured", written before any of the numbers below existed.

**Measured 2026-09-20, against `dc0ffe5` (the merge of closure conversion into
`master`) and `be3eb40`, by the protocol of "What removing the re-entries
moved" above.** Every binary — `dc0ffe5` and `be3eb40` built in throwaway
worktrees, the head `014f0ff` in the current tree — built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
each checked to print the expected answer for all eight sources first;
`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the three binaries rotating by one position from round to round, two whole
sittings, nothing else running. Two facts that cost real time and are recorded
here for the next measurement to skip past them: `dumpbin` lives at
`C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe`
on this machine, found via `vswhere.exe` in
`C:\Program Files (x86)\Microsoft Visual Studio\Installer\`, and is not on
`PATH`; and from Git Bash it must be invoked as `dumpbin -disasm`, not
`/disasm` — MSYS rewrites a `/option` argument into a Windows path, and the
tool then silently tries to dump a file called `C:\Program Files\Git\disasm`.

**`vm::run`'s step is still one function.** A release build with debug
information, `dumpbin -disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`), no `vm::run::closure` of its own, at **5,732
instructions**, against 5,700 at closure conversion — the trigger's
comparison and the safepoint's now-real slow path add 32.

**Peak working set fell as the design predicted, and this is the point of
the step.** `PROCESS_MEMORY_COUNTERS.PeakWorkingSetSize` through
`psapi!GetProcessMemoryInfo` on the still-open handle, release build, the
`run` and `bench --iterations 10` rows [memory.md](memory.md)'s table
already carried:

| Command | `dc0ffe5` | head |
|---|---|---|
| `run`, `bench_loop.ns` | 4.7 MB | 4.7 MB |
| `bench --iterations 10`, `bench_loop.ns` | 4.8 MB | 4.8 MB |
| `bench --iterations 10`, `bench_send.ns` | 4.8 MB | 4.8 MB |
| `bench --iterations 10`, `bench_block.ns` | **228.8 MB** | **5.6 MB** |

`bench_loop.ns` and `bench_send.ns`, which never collect (below), do not
move. `bench_block.ns` — the one source that allocates a closure and reifies
a frame every iteration, and so is the one source the arena's never-reclaim
interim showed most starkly — falls from 228.8 MB to 5.6 MB, level with the
other two. [memory.md](memory.md)'s own peak-working-set table, spanning
before the arena to after the collector, has the fourth column.

**Throughput is level on seven of the eight sources, entirely below
`be3eb40`, and one source is not level.** Medians in ms per round, then each
group's mean; sitting 1:

| Source | head | mean | `dc0ffe5` | mean | `be3eb40` | mean | head vs `dc0ffe5` | head vs `be3eb40` |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.55, 10.37, 10.53, 10.55, 10.32 | 10.46 | 10.37, 10.48, 10.34, 10.46, 10.45 | 10.42 | 11.35, 11.44, 11.48, 11.38, 11.53 | 11.43 | +0.4% | −8.5% |
| `bench_send.ns` | 11.58, 11.97, 11.59, 11.58, 11.58 | 11.66 | 11.65, 11.81, 11.62, 11.59, 11.66 | 11.67 | 14.15, 14.33, 14.36, 14.12, 14.01 | 14.19 | −0.1% | −17.9% |
| `bench_block.ns` | 21.28, 22.84, 22.62, 22.97, 22.66 | 22.47 | 20.39, 20.02, 20.24, 20.07, 20.21 | 20.19 | 23.70, 24.31, 23.80, 23.79, 23.87 | 23.89 | +11.3% | −5.9% |
| `bench_megamorphic.ns` | 16.78, 16.68, 16.98, 16.73, 16.65 | 16.76 | 16.77, 16.76, 17.05, 16.98, 16.92 | 16.90 | 18.43, 18.20, 18.42, 18.26, 18.29 | 18.32 | −0.8% | −8.5% |
| `bench_do.ns` | 7.07, 7.12, 6.98, 7.03, 6.96 | 7.03 | 6.98, 7.00, 7.12, 7.12, 7.11 | 7.07 | 8.30, 8.25, 8.22, 8.13, 8.37 | 8.25 | −0.5% | −14.8% |
| `bench_new.ns` | 6.30, 6.36, 6.17, 6.23, 6.24 | 6.26 | 6.34, 6.28, 6.39, 6.38, 6.35 | 6.35 | 7.62, 7.64, 7.66, 7.58, 7.58 | 7.62 | −1.3% | −17.8% |
| `bench_new2.ns` | 7.24, 7.41, 7.24, 7.27, 7.23 | 7.28 | 7.20, 7.17, 7.24, 7.26, 7.32 | 7.24 | 8.51, 8.63, 8.52, 8.47, 8.42 | 8.51 | +0.6% | −14.5% |
| `bench_newarg.ns` | 9.28, 9.42, 9.37, 9.49, 9.27 | 9.37 | 9.27, 9.01, 9.28, 9.08, 9.16 | 9.16 | 10.91, 10.95, 10.93, 10.81, 10.71 | 10.86 | +2.2% | −13.8% |

Sitting 2:

| Source | head | mean | `dc0ffe5` | mean | `be3eb40` | mean | head vs `dc0ffe5` | head vs `be3eb40` |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.38, 10.51, 10.48, 10.52, 10.33 | 10.44 | 10.61, 10.66, 10.62, 10.46, 10.42 | 10.55 | 11.44, 11.38, 11.47, 11.54, 11.37 | 11.44 | −1.0% | −8.7% |
| `bench_send.ns` | 11.72, 11.75, 11.58, 11.75, 11.58 | 11.68 | 11.67, 11.78, 11.54, 11.71, 11.67 | 11.67 | 14.25, 14.27, 14.11, 14.30, 14.34 | 14.26 | +0.0% | −18.1% |
| `bench_block.ns` | 22.89, 22.62, 22.60, 23.04, 23.20 | 22.87 | 20.24, 20.10, 20.29, 20.16, 20.39 | 20.24 | 23.93, 23.76, 23.74, 24.26, 24.15 | 23.97 | +13.0% | −4.6% |
| `bench_megamorphic.ns` | 16.96, 16.82, 17.04, 16.81, 16.80 | 16.89 | 16.77, 16.75, 17.03, 16.96, 16.98 | 16.90 | 18.42, 18.25, 18.43, 18.56, 18.43 | 18.42 | −0.1% | −8.3% |
| `bench_do.ns` | 7.11, 7.09, 7.15, 6.96, 7.00 | 7.06 | 7.01, 7.00, 7.13, 7.02, 7.11 | 7.06 | 8.10, 8.11, 8.17, 8.24, 8.21 | 8.17 | +0.1% | −13.5% |
| `bench_new.ns` | 6.19, 6.31, 6.21, 6.28, 6.25 | 6.25 | 6.38, 6.48, 6.41, 6.47, 6.43 | 6.44 | 7.51, 7.62, 7.61, 7.70, 7.67 | 7.62 | −2.9% | −18.0% |
| `bench_new2.ns` | 7.39, 7.33, 7.37, 7.28, 7.37 | 7.35 | 7.30, 7.21, 7.25, 7.41, 7.37 | 7.31 | 8.50, 8.53, 8.64, 8.67, 8.59 | 8.59 | +0.5% | −14.5% |
| `bench_newarg.ns` | 9.30, 9.39, 9.38, 9.49, 9.49 | 9.41 | 9.07, 9.10, 9.07, 9.06, 9.16 | 9.09 | 10.74, 10.92, 10.86, 10.86, 10.88 | 10.85 | +3.5% | −13.3% |

Both sittings pooled, by group mean, with the protocol's gate — a source has
moved only when its five-round range lies entirely above, or entirely below,
the group compared with, in **both** sittings:

| Source | head | `dc0ffe5` | `be3eb40` | vs `dc0ffe5` | vs `be3eb40` | groups overlap vs `dc0ffe5` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 10.45 | 10.49 | 11.44 | −0.3% | −8.6% | yes |
| `bench_send.ns` | 11.67 | 11.67 | 14.22 | −0.0% | −18.0% | yes |
| `bench_block.ns` | 22.67 | 20.21 | 23.93 | **+12.2%** | −5.3% | **no** |
| `bench_megamorphic.ns` | 16.82 | 16.90 | 18.37 | −0.4% | −8.4% | yes |
| `bench_do.ns` | 7.05 | 7.06 | 8.21 | −0.2% | −14.2% | yes |
| `bench_new.ns` | 6.25 | 6.39 | 7.62 | −2.1% | −17.9% | yes |
| `bench_new2.ns` | 7.31 | 7.27 | 8.55 | +0.6% | −14.5% | yes |
| `bench_newarg.ns` | 9.39 | 9.13 | 10.86 | +2.9% | −13.5% | yes |

**Seven of the eight sources are level with `dc0ffe5`** — `bench_loop.ns`
through `bench_newarg.ns` above, means spanning −2.1% to +2.9% — **and
5.3–18.0% below `be3eb40`.** Six of the seven overlap `dc0ffe5`'s group in
both sittings; `bench_newarg.ns`, the narrowest of the seven at +2.9%
pooled, is entirely above `dc0ffe5` in the second sitting and overlaps it by
only 0.01 ms in the first — and it is the source with the second-most
collections of the eight, 107. `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` are the four
out-of-tree sources written out in "What spec-conformant lookup moved"
above, copied from there rather than reconstructed, as that section
requires.

**`bench_block.ns` is not level, and the honest reading is that the step
cost it +12.2%.** Its pooled mean rose from 20.21 ms to 22.67 ms against
`dc0ffe5` — the one source whose pooled groups do not overlap in either
sitting, and the only one whose difference against `dc0ffe5` runs beyond a
couple of percent — while it stays 5.3% below `be3eb40`. It is the only one
of the eight sources that allocates a closure every iteration, and so the
only one that collects enough to show: `bench --collector-statistics` over
`bench --iterations 10` records

| Source | Collections | Total pause | Longest pause | Live objects | Live frames |
|---|---|---|---|---|---|
| `bench_block.ns` | 269 | 15.479 ms | 0.202 ms | 3,077 | 1,529 |
| `bench_loop.ns` | 0 | 0.000 ms | 0.000 ms | 49 | 0 |
| `bench_send.ns` | 0 | 0.000 ms | 0.000 ms | 49 | 0 |
| `bench_megamorphic.ns` | 0 | 0.000 ms | 0.000 ms | 175 | 0 |
| `bench_do.ns` | 26 | 1.341 ms | 0.088 ms | 4,062 | 1 |
| `bench_new.ns` | 53 | 2.662 ms | 0.070 ms | 4,037 | 0 |
| `bench_new2.ns` | 54 | 2.831 ms | 0.076 ms | 26 | 0 |
| `bench_newarg.ns` | 107 | 5.415 ms | 0.088 ms | 4,056 | 0 |

Pause accounts for roughly 1.4 ms of the 2.46 ms the head adds per round on
`bench_block.ns`; the remaining amount is not accounted for by pause and is
recorded as unexplained rather than guessed at.

Setting each source's pause per entry send against the change actually
measured separates what the collector itself costs from everything else it
moved. The pause is the total over eleven entry sends — one warm-up and ten
timed — and the change is the head's pooled median minus `dc0ffe5`'s:

| Source | Pause per send | Change per send | Not accounted for by pause |
|---|---|---|---|
| `bench_block.ns` | 1.407 ms | +2.460 ms | **+1.053 ms** |
| `bench_newarg.ns` | 0.492 ms | +0.260 ms | −0.232 ms |
| `bench_new2.ns` | 0.257 ms | +0.040 ms | −0.217 ms |
| `bench_new.ns` | 0.242 ms | −0.140 ms | −0.382 ms |
| `bench_do.ns` | 0.122 ms | −0.010 ms | −0.132 ms |

**The last column changes sign.** On the four sources other than
`bench_block.ns` the pause *exceeds* the measured change by 0.13 to 0.38 ms
per send: collecting gives more back there than it costs. Only on
`bench_block.ns` does anything cost beyond the pause. A sign that holds
across four sources is not noise, and it is the part of this measurement
most in need of explaining.

What plausibly gives back: an arena that never passes its limit never
doubles. Under `dc0ffe5` these sources grow both arenas to hold everything
ever allocated — `bench_new.ns` reaches roughly 220,000 objects over eleven
sends — and a growing `Vec` reallocates and copies periodically, in the
mutator, counted in no pause. The head's arenas stay at a few thousand slots.

One candidate this measurement rules out. Freeing the dead payloads is **not**
a mutator cost: `compact_arena`'s `truncate` drops them, inside
`Heap::compact`, inside the span `World::collect` times. Only the subsequent
allocation falls outside it, and allocating from a free list the collection
has just refilled is not obviously dearer than growing into fresh pages.

What fits the sign change better is the *number* of collections rather than
their total cost. `bench_block.ns` collects 269 times against
`bench_new.ns`'s 53 at a similar live object count, and every collection
walks the whole live graph, leaving the mutator's caches holding the
collector's working set instead of the program's. That after-effect scales
with how often a program collects rather than with how long each collection
takes, and it falls outside the measured pause. `bench_block.ns` is also the
only source with a populated frame arena — 1,529 live frames against 0 or 1
elsewhere — so it is the only one whose closures have their
`captured_frame` rewritten at every collection.

Both of those are hypotheses. Two cheap experiments discriminate between
them: a fixture whose blocks capture nothing, which keeps the object churn
and removes the frame reification; and varying `COLLECTION_FLOOR` so that the
same program collects less often but does more per collection, since a cost
that tracks the collection count rather than the pause total is the cache
after-effect rather than the work. `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` collect too — 26 to 107
times over ten iterations — at a fraction of the pause, and stay level with
`dc0ffe5` — so
collecting itself is not what shows on `bench_block.ns`; something about
collecting *269 times in ten iterations*, an order of magnitude more often,
is.

**`bench_loop.ns`, `bench_send.ns` and `bench_megamorphic.ns` report zero
collections, and those are exactly the three sources whose throughput did
not move.** All three stay under the 4,096-slot floor ("The trigger and the
growth policy", [memory.md](memory.md)), so no allocation in them ever
requests a collection, and the send benchmarks do not feel the trigger's
added comparison in `push_object`. The recorded expectation named
`bench_loop.ns` and `bench_send.ns` by exactly this reasoning, in advance;
`bench_megamorphic.ns`, not named there, turns out to sit under the floor
too and shows the identical pattern — zero collections, no measurable move —
which is the expectation generalizing rather than a coincidence limited to
the two sources it named.

### What leaf inlining moved

The compiler now recognizes the nine binary integer primitives
(`+ - * // < > <= >= =`) at an ordinary explicit-receiver send and emits
`Instruction::IntegerBinary` instead of a send, with the send kept as the
fallback the instruction jumps to when either operand is not a
`SmallInteger` or the operation declines
([compiler.md](compiler.md), "Arithmetic inlining"). This is the step
[core-library.md](core-library.md) sequences ahead of the move to a VM
mirror: once every builtin method is Newspeak over a mirror, a leaf send
that costs an activation is paid for on every arithmetic operation in the
library, so the fast path has to exist first.

**The expectation, recorded before any binary was built.** Sources whose hot
loop does integer arithmetic should improve, because an operation that was a
full send becomes one instruction plus a guard; sources that do none should
not move. Counting the eight sources first turned up the fact that **the
second group is empty**: every one of them counts its iterations with
`n < <limit>` and `n:: n + 1`, so every one gets at least two operations per
iteration out of the send path. The prediction was therefore a ranking by
how much non-arithmetic work surrounds those operations — large (>10%) for
`bench_loop.ns`, whose loop body is arithmetic and slot sends and nothing
else; moderate (4–10%) for `bench_send.ns`, `bench_megamorphic.ns` and
`bench_do.ns`; small (0–4%) for `bench_block.ns`, `bench_new.ns`,
`bench_new2.ns` and `bench_newarg.ns`, whose iterations were expected to be
dominated by a closure allocation or an instantiation. No source was
expected to get slower: the unconditional `Jump` past the fallback that
every *successful* operation also executes makes two dispatches out of one
send, which should show as a smaller gain than the send count suggests, not
as a regression.

**Measured 2026-09-21, against `6f5a5b1`, the commit on `master` the branch
was cut from**, by the protocol of "What removing the re-entries moved"
above: both binaries built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
the baseline in a throwaway worktree, neither importing `VCRUNTIME140.dll`
and both checked to print the expected answer for all eight sources first;
`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the two binaries alternating from round to round, two whole sittings,
nothing else running.

**`vm::run`'s step is still one function.** A release build with debug
information, `dumpbin /disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`) in each binary, no `vm::run::closure` of its own.
The baseline measures **5,732 instructions**, the same figure recorded at
the moving collector, and the head **5,866** — the `IntegerBinary` arm adds
134, and the step did not split.

Medians in ms per round, then each group's mean; sitting 1:

| Source | Before (`6f5a5b1`), rounds 1–5 | mean | After, rounds 1–5 | mean | Change |
|---|---|---|---|---|---|
| `bench_loop.ns` | 10.29, 10.49, 10.49, 10.36, 10.52 | 10.43 | 7.17, 7.19, 7.42, 7.17, 7.21 | 7.23 | **−30.6%**, entirely below |
| `bench_send.ns` | 11.54, 11.60, 11.76, 11.71, 11.72 | 11.67 | 8.69, 8.58, 8.59, 8.61, 8.77 | 8.65 | **−25.9%**, entirely below |
| `bench_block.ns` | 20.18, 20.60, 20.35, 20.65, 20.59 | 20.48 | 17.96, 18.00, 18.20, 17.85, 17.82 | 17.97 | **−12.3%**, entirely below |
| `bench_megamorphic.ns` | 16.70, 16.81, 16.77, 16.80, 17.06 | 16.83 | 11.77, 11.67, 11.64, 11.75, 11.95 | 11.76 | **−30.1%**, entirely below |
| `bench_do.ns` | 7.09, 7.12, 7.05, 6.98, 7.06 | 7.06 | 5.82, 5.77, 5.84, 5.84, 5.78 | 5.81 | **−17.7%**, entirely below |
| `bench_new.ns` | 6.24, 6.27, 6.30, 6.21, 6.34 | 6.27 | 5.08, 5.00, 4.97, 4.96, 4.97 | 5.00 | **−20.3%**, entirely below |
| `bench_new2.ns` | 7.28, 7.33, 7.39, 7.34, 7.28 | 7.32 | 5.99, 6.05, 6.08, 6.08, 6.05 | 6.05 | **−17.4%**, entirely below |
| `bench_newarg.ns` | 9.39, 10.03, 9.35, 9.59, 9.42 | 9.56 | 8.06, 8.15, 8.13, 8.20, 8.07 | 8.12 | **−15.0%**, entirely below |

Sitting 2:

| Source | Before (`6f5a5b1`), rounds 1–5 | mean | After, rounds 1–5 | mean | Change |
|---|---|---|---|---|---|
| `bench_loop.ns` | 10.52, 10.59, 10.39, 10.48, 10.51 | 10.50 | 7.15, 7.30, 7.14, 7.15, 7.27 | 7.20 | **−31.4%**, entirely below |
| `bench_send.ns` | 11.58, 11.63, 11.78, 11.58, 11.83 | 11.68 | 8.57, 8.56, 8.50, 8.58, 8.57 | 8.55 | **−26.8%**, entirely below |
| `bench_block.ns` | 20.39, 20.65, 20.64, 20.33, 20.57 | 20.52 | 18.27, 18.30, 18.34, 18.36, 18.34 | 18.32 | **−10.7%**, entirely below |
| `bench_megamorphic.ns` | 16.72, 16.78, 17.05, 16.77, 16.98 | 16.86 | 11.66, 11.81, 11.95, 11.72, 11.91 | 11.81 | **−30.0%**, entirely below |
| `bench_do.ns` | 7.05, 7.16, 7.21, 7.14, 7.05 | 7.12 | 5.75, 5.89, 5.95, 5.70, 5.89 | 5.84 | **−18.0%**, entirely below |
| `bench_new.ns` | 6.20, 6.31, 6.19, 6.26, 6.30 | 6.25 | 4.99, 5.00, 5.01, 5.00, 5.02 | 5.01 | **−19.9%**, entirely below |
| `bench_new2.ns` | 7.30, 7.33, 7.35, 7.32, 7.34 | 7.33 | 6.11, 6.08, 6.12, 6.14, 6.18 | 6.12 | **−16.4%**, entirely below |
| `bench_newarg.ns` | 9.23, 9.30, 9.25, 9.24, 9.40 | 9.28 | 8.01, 8.11, 8.01, 8.07, 8.14 | 8.07 | **−13.1%**, entirely below |

**Every one of the eight sources is faster, by 11.5% to 31.0%, and every
after-group lies entirely below its before-group in both sittings** — the
protocol's gate, met by all eight. The only earlier step to clear it across
its whole suite was 3b, the inline caches, at 4.3–11.3% over seven sources
("What the inline caches moved" above). Both sittings pooled:

| Source | Before mean | After mean | Change |
|---|---|---|---|
| `bench_loop.ns` | 10.46 | 7.22 | **−31.0%** |
| `bench_megamorphic.ns` | 16.84 | 11.78 | **−30.0%** |
| `bench_send.ns` | 11.67 | 8.60 | **−26.3%** |
| `bench_new.ns` | 6.26 | 5.00 | **−20.1%** |
| `bench_do.ns` | 7.09 | 5.82 | **−17.8%** |
| `bench_new2.ns` | 7.33 | 6.09 | **−16.9%** |
| `bench_newarg.ns` | 9.42 | 8.09 | **−14.1%** |
| `bench_block.ns` | 20.50 | 18.14 | **−11.5%** |

**The expectation's ends were right and its middle was not, and every
magnitude was under-predicted by three to five times.** `bench_loop.ns` did
gain most and `bench_block.ns` least, as named in advance. But the whole
"small" group — `bench_block.ns`, `bench_new.ns`, `bench_new2.ns` and
`bench_newarg.ns` — landed between 11.5% and 20.1%, well inside and above
the band predicted for the "moderate" group, and `bench_new.ns` at −20.1%
came out ahead of `bench_do.ns` at −17.8%, which had been predicted
moderate. `bench_megamorphic.ns` came second overall rather than in the
middle of its group. The prediction's error was in its premise, not its arithmetic:
it assumed an instantiation or a block activation dwarfs three arithmetic
sends, and the aggregate timing deltas read as three arithmetic sends being
of the same order as an instantiation -- an inference from those deltas, not
something isolated; no profile was taken of this step.

**What the ordering does track is the share of a source's per-iteration
sends the step removed, discounted by how expensive the sends left behind
are.** Counting the send instructions `ns-next-analyzer disasm` prints in each
source's loop, on the path an iteration actually takes:

| Source | sends per iteration before | after | share removed | measured |
|---|---|---|---|---|
| `bench_loop.ns` | 9 | 6 | 33% | −31.0% |
| `bench_megamorphic.ns` | 12 | 7 | 42% | −30.0% |
| `bench_send.ns` | 9 | 6 | 33% | −26.3% |
| `bench_new.ns` | 8 | 5 | 38% | −20.1% |
| `bench_do.ns` | 33 | 21 | 36% | −17.8% |
| `bench_new2.ns` | 8 | 5 | 38% | −16.9% |
| `bench_newarg.ns` | 8 | 5 | 38% | −14.1% |
| `bench_block.ns` | 10 | 7 | 30% | −11.5% |

`bench_loop.ns` is the clean case: its nine sends are six slot sends and
three arithmetic leaves, all of the same order of cost, and removing a third
of them takes off 31%. Wherever the remaining sends are dearer than a leaf
the gain falls away from the share — `bench_do.ns` keeps 50,000 block
activations through `do:`, the `bench_new*` sources an instantiation and a
getter method each, and `bench_block.ns`, the extreme, keeps a method
activation, a `MakeVector`, a `MakeClosure` and a block activation per
iteration for 30% of its sends removed and 11.5% of its time. That reading
is an inference from the instruction counts and the timings; no profile was
taken of this step.

**The `Jump` past the fallback does not show, and the data cannot bound it
better than this.** Every successful operation executes `IntegerBinary` and
then an unconditional `Jump` over the fallback send, so the hot path is two
dispatches where an out-of-line fallback would leave one
([compiler.md](compiler.md), "Arithmetic inlining"). `bench_loop.ns` removed
33% of its sends and gained 31.0%, which is the tightest the suite gets: the
2.3 points between them are everything the extra dispatches plus the guard
plus any second-order effect could account for, and they are not separable
from the difference between a slot send's cost and a leaf send's. Placing
the fallback out of line is therefore a follow-up worth trying and not one
this measurement makes a case for.

### What the core library's move cost

Every builtin method now lives in `crates/ns-runtime/core-library/Kernel.ns`
as a forwarder over the one VM mirror ([core-library.md](core-library.md),
"The core library as Newspeak source over a VM mirror"). A kernel method whose
whole body forwards to one mirror row is a primitive method: a send of it runs
the row's leaf in `act`, without an activation, and activates the body only
when the leaf declines ([dispatch.md](dispatch.md), "Primitive methods").
`NS_NEXT_NO_PRIMITIVE_METHODS=1` compiles the kernel without them, for
measuring only, so the same binary shows what they recover.

**The expectation, recorded before any binary was built.** With primitive
methods on, against `e2bb31e`: level on every source. A leaf send that
`IntegerBinary` does not absorb costs one cached send plus one field test on
the method, where it cost one cached send; the rest of the path is the same
primitive. The one new cost on every method send is that field test; if a
source that sends no leaf moves, that is where to look. Under
`NS_NEXT_NO_PRIMITIVE_METHODS=1`, against `e2bb31e`: slower on every source
that sends a leaf the fast path does not absorb, since each such send becomes
an activation of the kernel method, a lexical send of `vmMirror` and a
`SendMirror`. From their text those are `bench_block.ns` (`value:`, 50,000 a
run), `bench_megamorphic.ns` (`at:`, 50,000 a run) and `bench_do.ns` (`do:`,
5,000 a run, each running ten block activations the leaf makes);
`bench_loop.ns`, `bench_send.ns` and the three `bench_new*` sources send
`printString`, `show:` and `newLine` only once, after their loop, and should
be level. Start-up, against `e2bb31e`: slower by the kernel's parse,
resolution, compilation and instantiation, which did not exist.

**Measured 2026-09-22, against `e2bb31e` (the merge of leaf inlining into
`master`) and `be3eb40`, by the protocol of "What removing the re-entries
moved" above.** Every binary -- `e2bb31e` and `be3eb40` built in throwaway
worktrees, the head `52c242b` in the current tree -- built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
none imports `VCRUNTIME140.dll` (`dumpbin -dependents`); each checked to
print the expected answer for all eight sources first, the head in both
configurations; `ns-next bench <source> --iterations 30`, five rounds per
sitting, the four configurations -- `e2bb31e`, `be3eb40`, the head, the head
under `NS_NEXT_NO_PRIMITIVE_METHODS=1`, the same binary with the variable
set for its runs only -- rotating by one position from round to round, two
whole sittings, nothing else running.

**`vm::run`'s step is still one function.** A release build with debug
information of the head and of `e2bb31e` (the timed build's `RUSTFLAGS`
plus `CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`), then
`dumpbin -disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`) in each, no `vm::run::closure` of its own. `e2bb31e`
measures **5,866 instructions**, the figure leaf inlining recorded, and the
head **6,385** -- 519 more, for the `SendMirror` arm and the primitive-method
test in every inlined `act`. The step did not split, so no remedy was
needed. Two things the head changed on the send path, checked in the same
listing: `vm::deliver`, `#[inline(always)]` and shared by `send` and
`SendMirror`, has no label of its own, so it is inlined into both; and
`act`'s `Primitive` arm, which now builds its error through
`unwrap_or_else`, keeps that error out of line --
`resolve::impl$2::act_with_base::closure$0` and `resolve::unhandled_error`
are small functions of their own, called from three sites each in
`vm::run`, where `e2bb31e` called the cold `act_above` and
`act_handler_primitive` from four each. What the primitive methods add to
the hot path is visible as calls: `vm::run` calls `builtins::execute` from
eight sites, against four in `e2bb31e` (`mirror::execute_method` is inlined
into each method arm, and the leaf it runs stays out of line), and
`mirror::run_leaf` from one, the `SendMirror` arm.

Medians in ms per round, then each group's mean; sitting 1:

| Source | `e2bb31e`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | Head, rounds 1–5 | mean | Head, no primitive methods, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.21, 7.14, 7.09, 7.20, 7.12 | 7.15 | 11.25, 11.10, 11.33, 11.26, 11.23 | 11.23 | 6.97, 7.13, 7.17, 7.09, 6.94 | 7.06 | 6.87, 7.06, 7.04, 7.04, 6.94 | 6.99 |
| `bench_send.ns` | 8.43, 8.43, 8.51, 8.33, 8.51 | 8.44 | 13.73, 13.92, 14.03, 14.01, 13.95 | 13.93 | 8.16, 8.29, 8.40, 8.38, 8.41 | 8.33 | 8.33, 8.24, 8.40, 8.30, 8.41 | 8.33 |
| `bench_block.ns` | 17.58, 17.81, 17.79, 17.78, 17.83 | 17.76 | 22.98, 23.45, 23.21, 23.35, 23.33 | 23.27 | 16.83, 16.94, 16.84, 16.64, 16.95 | 16.84 | 21.82, 21.58, 21.36, 21.60, 21.43 | 21.56 |
| `bench_megamorphic.ns` | 11.59, 11.52, 11.58, 11.46, 11.61 | 11.55 | 17.96, 18.08, 17.97, 17.96, 18.06 | 18.01 | 11.32, 11.34, 11.43, 11.55, 11.42 | 11.41 | 14.32, 14.29, 14.44, 14.21, 14.30 | 14.31 |
| `bench_do.ns` | 5.59, 5.71, 5.59, 5.64, 5.57 | 5.62 | 8.09, 8.02, 8.14, 8.26, 8.61 | 8.23 | 5.76, 5.72, 5.81, 5.66, 6.23 | 5.84 | 5.98, 5.98, 5.98, 6.01, 5.99 | 5.99 |
| `bench_new.ns` | 4.81, 4.96, 4.92, 4.95, 4.98 | 4.92 | 7.42, 7.50, 7.73, 7.53, 7.47 | 7.53 | 4.81, 4.84, 4.82, 4.79, 4.88 | 4.83 | 4.92, 4.93, 4.94, 4.98, 4.98 | 4.95 |
| `bench_new2.ns` | 5.87, 6.03, 5.91, 5.88, 5.93 | 5.92 | 8.34, 8.36, 8.37, 8.41, 8.38 | 8.37 | 5.84, 5.81, 5.80, 5.79, 5.75 | 5.80 | 5.79, 5.97, 5.83, 5.75, 5.78 | 5.82 |
| `bench_newarg.ns` | 8.16, 8.08, 8.11, 8.18, 8.14 | 8.14 | 10.54, 10.84, 10.79, 10.60, 10.67 | 10.69 | 8.06, 8.13, 8.03, 7.95, 8.03 | 8.04 | 8.25, 8.37, 8.33, 8.29, 8.46 | 8.34 |

Sitting 2:

| Source | `e2bb31e`, rounds 1–5 | mean | `be3eb40`, rounds 1–5 | mean | Head, rounds 1–5 | mean | Head, no primitive methods, rounds 1–5 | mean |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.24, 7.03, 7.08, 7.31, 7.20 | 7.17 | 11.17, 11.34, 11.22, 11.22, 11.14 | 11.22 | 7.00, 7.01, 7.10, 6.96, 7.11 | 7.04 | 7.04, 7.15, 7.05, 7.08, 7.03 | 7.07 |
| `bench_send.ns` | 8.65, 8.53, 8.34, 8.38, 8.53 | 8.49 | 13.90, 13.94, 14.10, 13.95, 14.04 | 13.98 | 8.33, 8.26, 8.37, 8.50, 8.33 | 8.36 | 9.38, 8.26, 8.36, 8.34, 8.36 | 8.54 |
| `bench_block.ns` | 17.71, 17.67, 17.81, 17.62, 17.91 | 17.74 | 23.18, 23.31, 23.27, 23.29, 23.33 | 23.28 | 17.85, 16.99, 16.59, 16.86, 17.11 | 17.08 | 21.55, 21.37, 21.56, 21.49, 21.49 | 21.49 |
| `bench_megamorphic.ns` | 11.41, 11.75, 11.59, 11.79, 11.43 | 11.59 | 18.04, 17.95, 18.19, 18.06, 18.02 | 18.05 | 11.44, 11.43, 11.38, 11.38, 11.45 | 11.41 | 14.42, 14.24, 14.33, 14.32, 14.17 | 14.30 |
| `bench_do.ns` | 5.78, 5.74, 5.72, 5.64, 5.66 | 5.71 | 8.00, 8.00, 8.05, 7.94, 8.08 | 8.02 | 5.71, 5.64, 5.85, 5.74, 5.71 | 5.73 | 5.97, 5.92, 5.99, 5.92, 6.00 | 5.96 |
| `bench_new.ns` | 4.91, 4.92, 4.90, 5.00, 4.89 | 4.92 | 7.42, 7.37, 7.55, 7.50, 7.48 | 7.47 | 4.88, 4.81, 4.87, 4.82, 4.81 | 4.84 | 4.91, 4.94, 4.95, 4.98, 4.97 | 4.95 |
| `bench_new2.ns` | 5.99, 5.94, 5.85, 5.94, 5.93 | 5.93 | 8.36, 8.41, 8.36, 8.27, 8.44 | 8.37 | 5.85, 5.81, 5.79, 5.85, 5.88 | 5.84 | 5.72, 5.82, 5.78, 5.86, 5.79 | 5.79 |
| `bench_newarg.ns` | 8.20, 8.19, 8.04, 8.23, 8.15 | 8.16 | 10.61, 10.63, 10.59, 10.53, 10.54 | 10.58 | 7.98, 7.98, 8.01, 8.00, 7.92 | 7.98 | 8.41, 8.31, 8.37, 8.38, 8.36 | 8.36 |

The change of each head configuration's mean against each baseline,
sitting 1 / sitting 2:

| Source | Head against `e2bb31e` | No primitive methods against `e2bb31e` | Head against `be3eb40` | No primitive methods against `be3eb40` |
|---|---|---|---|---|
| `bench_loop.ns` | −1.3% / −1.9%, overlapping | −2.3% / −1.4%, entirely below in the first | −37.2% / −37.3%, entirely below | −37.8% / −37.0%, entirely below |
| `bench_send.ns` | −1.3% / −1.5%, overlapping | −1.3% / +0.6%, overlapping | −40.2% / −40.2%, entirely below | −40.2% / −38.9%, entirely below |
| `bench_block.ns` | −5.2% / −3.7%, entirely below in the first | **+21.4% / +21.1%, entirely above** | −27.6% / −26.6%, entirely below | −7.3% / −7.7%, entirely below |
| `bench_megamorphic.ns` | −1.2% / −1.6%, overlapping | **+23.9% / +23.3%, entirely above** | −36.6% / −36.8%, entirely below | −20.5% / −20.8%, entirely below |
| `bench_do.ns` | +3.8% / +0.4%, overlapping | **+6.5% / +4.4%, entirely above** | −29.1% / −28.5%, entirely below | −27.2% / −25.7%, entirely below |
| `bench_new.ns` | −1.9% / −1.7%, entirely below in the second | +0.5% / +0.6%, overlapping | −35.9% / −35.2%, entirely below | −34.3% / −33.7%, entirely below |
| `bench_new2.ns` | −2.1% / −1.5%, entirely below in the first | −1.7% / −2.2%, overlapping | −30.8% / −30.3%, entirely below | −30.5% / −30.8%, entirely below |
| `bench_newarg.ns` | −1.2% / −2.3%, entirely below in the second | **+2.5% / +2.5%, entirely above** | −24.8% / −24.6%, entirely below | −21.9% / −21.0%, entirely below |

Both sittings pooled, means in ms:

| Source | `e2bb31e` | `be3eb40` | Head | No primitive methods | Head against `e2bb31e` | No primitive methods against `e2bb31e` | No primitive methods against the head |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.16 | 11.23 | 7.05 | 7.03 | −1.6% | −1.8% | −0.2% |
| `bench_send.ns` | 8.46 | 13.96 | 8.34 | 8.44 | −1.4% | −0.3% | +1.1% |
| `bench_block.ns` | 17.75 | 23.27 | 16.96 | 21.52 | −4.5% | **+21.3%** | **+26.9%** |
| `bench_megamorphic.ns` | 11.57 | 18.03 | 11.41 | 14.30 | −1.4% | **+23.6%** | **+25.3%** |
| `bench_do.ns` | 5.66 | 8.12 | 5.78 | 5.97 | +2.1% | **+5.5%** | +3.3% |
| `bench_new.ns` | 4.92 | 7.50 | 4.83 | 4.95 | −1.8% | +0.5% | +2.4% |
| `bench_new2.ns` | 5.93 | 8.37 | 5.82 | 5.81 | −1.8% | −2.0% | −0.1% |
| `bench_newarg.ns` | 8.15 | 10.63 | 8.01 | 8.35 | −1.7% | +2.5% | +4.3% |

**With primitive methods on, moving the core library into Newspeak cost
nothing measurable: the head is −4.5% to +2.1% against `e2bb31e`, and no
head group lies entirely above its `e2bb31e` group in either sitting.** The
expectation said level, and that is what the suite shows: seven sources
between −1.4% and −4.5%, four of them entirely below in one sitting, and
`bench_do.ns` at +3.8% / +0.4%, overlapping, its first-sitting mean carried
by one round at 6.23 ms against four between 5.66 and 5.81. The field test
every method send now makes does not show: `bench_send.ns` activates one
ordinary method per iteration, `tick:`, and is −1.4%. Against `be3eb40` the
head is 24.7% to 40.2% faster on every source, entirely below in both
sittings.

**Without them, the three sources named in advance are the three that got
clearly slower**, entirely above in both sittings: `bench_megamorphic.ns`
+23.6% and `bench_block.ns` +21.3%, each sending one leaf 50,000 times a run
(`at:` and `value:`), and `bench_do.ns` +5.5%, whose `do:` is sent 5,000
times. That is what the primitive methods recover: 26.9% of
`bench_block.ns`'s time, 25.3% of `bench_megamorphic.ns`'s and 3.3% of
`bench_do.ns`'s against the same binary without them. Per send, the
activation, the lexical `vmMirror` send and the `SendMirror` that replace a
primitive method's fast path cost about 58 ns on `bench_megamorphic.ns`
(2.89 ms over 50,000 `at:`) and about 91 ns on `bench_block.ns` (4.56 ms over
50,000 `value:`, whose leaf then activates the block, so the kernel method's
own activation sits beneath the block's on the stack); on `bench_do.ns`, 0.19
ms over 5,000 `do:` is about 38 ns each. Even so, every source without
primitive methods stays 7.5% to 39.5% faster than `be3eb40`, pooled.

**What is left unexplained.** `bench_newarg.ns` is 2.5% slower without
primitive methods in both sittings, entirely above, and `bench_new.ns` 2.4%
slower than the head, entirely above in both, although neither sends a
kernel method in its loop, by its text: `--cache-statistics` counts four more
sites filled without primitive methods on every source, which fits the
`vmMirror` and mirror sends of the activated `printString`, `show:` and
`newLine` each sends once after its loop. Being the same binary, code
placement is not the explanation either; what differs between the two is the
kernel's compiled bodies, and how that reaches an instantiation loop is not
isolated. Nor is why
`bench_block.ns` is 4.5% *faster* with primitive methods than `e2bb31e`,
entirely below in the first sitting: its `value:` went from a primitive
entry of `Closure`'s method table, run by `act`'s `Primitive` arm, to a
kernel method whose leaf runs from its `Method` arm, and nothing on that
path is obviously cheaper. Neither was profiled.

**Two costs the primitive methods carry that the suite does not exercise.**
A leaf that declines runs twice: once in `act`'s fast path, and again from
the activated body's `SendMirror` before its failure block runs. It is paid
only on a failure path: every kernel method's failure block resends its
message to `super`, which for all but a few rows (`Integer`'s `=` reaching
`Object`'s) ends in `doesNotUnderstand:`. No figure was taken; the suite runs
no failing leaf. And a cached site
whose target is a primitive method reserves feedback-table room for that
method's family although the fast path never activates it:
`ns-next run --cache-statistics` counts 23 site entries for `bench_loop.ns`
against 15 in `e2bb31e`, 26 against 16 for `bench_block.ns` and 39 against
29 for `bench_megamorphic.ns`, with the same number filled -- 8 to 10
entries of 24 bytes, 192 to 240 bytes a run, for the `printString`, `show:`
and `newLine` sent once.

**Start-up is 0.5 ms slower**, the kernel's parse, resolution, compilation
and instantiation, as expected. The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, wall-clock from the parent process, 50
runs of each binary alternating, in two sittings: `e2bb31e` median 3.56 ms
and 3.55 ms (10th to 90th percentile 3.31–3.71 and 3.46–3.82), the head
4.08 ms and 4.06 ms (3.84–4.35 and 3.96–4.31) -- +0.51 ms, 14%, the head's
10th percentile above `e2bb31e`'s 90th in both. That is under a
millisecond, so embedding the compiled kernel as an image generated at
build time ([core-library.md](core-library.md), "Where the library source
lives, and how modules are found") is not called for by this figure; it stays the named next step if
the kernel grows until it is.

### What `Top`, `Value` and `Number` cost

Giving the core library the specification's object model
([core-library.md](core-library.md), "What exists"; §3.2 and §3.1.1) changed
three things a release build runs. The hierarchy gained two levels -- `Top`
above `Object`, and `Value`, with `Number` under it, between `Object` and the
value classes -- so a *failed* lookup walks two levels more before it reaches
`doesNotUnderstand:`. `Instruction::InitializeSlot` gained a branch, the
deep-immutability check of §3.1.1's second condition
(`instantiate::check_value_slot`). And `==` on a value is no longer `Object`'s
identity leaf but `Value`'s `^self = other`, two dispatches where there was
one.

**The expectation, recorded before any binary was built.** Level on all eight
sources. No send in any of the eight loops fails, so the two extra levels
cannot show. **None of the eight compares values in its loop** -- no `=` and
no `==` appears in any of them, in the loop or outside it -- so the two
dispatches cannot show either, and this measurement does not bound them; what
a value comparison costs is stated from the source instead. That leaves the
`InitializeSlot` branch, which can show only on `bench_new.ns`, `bench_new2.ns`
and `bench_newarg.ns`, the three that run one or two initializers per
iteration, 20,000 iterations each. Every one of their stores is an immediate,
so the check returns on its first test.

**Measured 2026-09-23, `ab692b7` against `b0a771d`, by the protocol of "What
removing the re-entries moved" above.** `b0a771d` is this branch's head before
the first commit of the work (`1dc548a`) and is itself documentation only, so
its tree is the code as it stood before; `ab692b7` is the last commit of the
work, the one the figures below were taken at. `2ad6f30` landed afterwards --
a value's state is the slots its classes declare, so a nested class's cache
slot is no longer compared or hashed -- and touches no path any of the eight
sources runs: none of them declares a value class, and the change is confined
to `builtins/value.rs`'s walks and the two reflective leaves. The figures
stand for the head. Every binary built with
`RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`;
neither imports `VCRUNTIME140.dll` (`dumpbin -dependents`); each checked to
print the expected answer for all eight sources first;
`ns-next bench <source> --iterations 30`, five rounds a sitting, the binaries
rotating by one position from round to round, two whole sittings, nothing else
running.

**Five binaries rather than two, and why.** The first pairing -- one build of
the baseline against one build of the head -- put `bench_block.ns` 13.4%
*faster* at the head, every group entirely below, which nothing the change
touches can explain: `bench_block.ns` instantiates nothing, and its loop sends
`<`, `+`, `value:` and `bump:`. Two further builds of the baseline and two of
the head, from the same trees with the same flags, showed what it was. On
`bench_block.ns` alone the five builds sit at five separate levels --
17.21 ms, 18.86 (baseline) and 16.49, 19.13, 19.13 (head) -- each stable to
about 1% across ten rounds, with the fastest and the two slowest all being
head builds. The spread is 16%, it straddles the commit boundary, and it is
therefore a property of the build, not of the change. The other seven sources
show 0.3% to 2.3% between builds of the same tree. `bench_block.ns` is the
longest-running source and the only one whose loop activates a block through a
primitive method; what in the layout decides which level a build lands on was
not isolated. **One build per commit is not enough for `bench_block.ns`**, and
the comparison below pools the builds of each commit instead.

**`vm::run`'s step is still one function.** A release build with debug
information of each tree (the timed build's `RUSTFLAGS` plus
`CARGO_PROFILE_RELEASE_DEBUG=true` and a separate `--target-dir`), then
`dumpbin -disasm`: one function label matching `vm::run`
(`ns_runtime::vm::run`) in each, no `vm::run::closure` of its own. The
baseline measures **6,389 instructions** and the head **6,506** -- 117 more,
the inlined `check_value_slot` in the `InitializeSlot` arm. `check_value_slot`
has no label of its own in either listing, so it is inlined rather than
called; the only new function is the `#[cold]`
`instantiate::non_value_slot_error`, which builds the refusal. The step did not
split, so no remedy was needed.

Means in ms per build, over that sitting's five rounds; sitting 1, then
sitting 2. `base1` and `base2` are the two baseline builds, `head1` to `head3`
the three head builds:

| Source | `base1` | `base2` | `head1` | `head2` | `head3` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.16 | 7.10 | 7.14 | 7.08 | 7.14 |
| `bench_send.ns` | 8.45 | 8.45 | 8.49 | 8.74 | 8.45 |
| `bench_block.ns` | 18.68 | 17.17 | 16.44 | 19.01 | 19.05 |
| `bench_megamorphic.ns` | 11.56 | 11.52 | 11.57 | 11.60 | 11.62 |
| `bench_do.ns` | 5.73 | 5.81 | 5.79 | 5.85 | 5.95 |
| `bench_new.ns` | 5.01 | 5.04 | 4.97 | 4.97 | 4.98 |
| `bench_new2.ns` | 5.80 | 5.90 | 6.02 | 6.01 | 6.05 |
| `bench_newarg.ns` | 8.10 | 8.08 | 8.05 | 8.06 | 7.96 |

| Source | `base1` | `base2` | `head1` | `head2` | `head3` |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.14 | 7.14 | 7.18 | 7.17 | 7.15 |
| `bench_send.ns` | 8.53 | 8.45 | 8.54 | 8.50 | 8.51 |
| `bench_block.ns` | 19.05 | 17.25 | 16.54 | 19.24 | 19.21 |
| `bench_megamorphic.ns` | 11.60 | 11.56 | 11.63 | 11.65 | 11.65 |
| `bench_do.ns` | 5.77 | 5.79 | 5.87 | 5.87 | 5.98 |
| `bench_new.ns` | 5.00 | 5.07 | 5.00 | 5.04 | 5.00 |
| `bench_new2.ns` | 5.86 | 5.83 | 6.05 | 6.08 | 6.13 |
| `bench_newarg.ns` | 8.05 | 8.11 | 8.08 | 8.08 | 8.03 |

Both sittings pooled, the baseline's two builds against the head's three:

| Source | Baseline | Head | Change, pooled | Change, sitting 1 / 2 |
|---|---|---|---|---|
| `bench_loop.ns` | 7.13 | 7.14 | +0.1% | −0.1% / +0.3% |
| `bench_send.ns` | 8.47 | 8.54 | +0.8% | +1.3% / +0.3% |
| `bench_block.ns` | 18.04 | 18.25 | +1.2% | +1.3% / +1.0% |
| `bench_megamorphic.ns` | 11.56 | 11.62 | +0.5% | +0.4% / +0.5% |
| `bench_do.ns` | 5.78 | 5.88 | +1.9% | +1.6% / +2.2% |
| `bench_new.ns` | 5.03 | 4.99 | −0.7% | −1.0% / −0.5% |
| `bench_new2.ns` | 5.85 | **6.06** | **+3.6%** | +3.0% / +4.2% |
| `bench_newarg.ns` | 8.08 | 8.05 | −0.5% | −0.8% / −0.2% |

**Seven of the eight are level, and the expectation held for them**: −0.7% to
+1.9%, inside the 0.3–2.3% these sources show between builds of the same tree.
`bench_block.ns`'s +1.2% is not a figure at all -- the build-to-build spread on
that source is 16%, and both the fastest and the slowest builds are head
builds, so this measurement says nothing about it either way.

**Open finding: `bench_new2.ns` is 3.6% slower**, and that is a real
regression, not scatter. All three head builds (6.02, 6.01, 6.05 and 6.05,
6.08, 6.13) lie above both baseline builds (5.80, 5.90 and 5.86, 5.83) in both
sittings, and the source's build-to-build spread is 0.7%. It was one of the
three the expectation named. What does not follow is why it is the only one:
`bench_newarg.ns` runs the same two initializers per instantiation over the
same two levels and is −0.5%, and `bench_new.ns`, with one, is −0.7%. So the
per-store check alone does not explain it -- all three store immediates, where
the check returns on its first test. The other two candidates on that path are
the `Ok`/`Err` match the `InitializeSlot` arm now carries, which is new
control flow inside `vm::run` where there was a straight store, and
`MixinApplication` having grown two fields (`inherits_value` and
the first mutable slot), which `decide_instantiation` reads once per instantiation
for §3.1.1's first condition. Neither was profiled, and nothing was bisected
between them. **What would recover it**: deciding the check once per
activation instead of once per store. The application already knows
`inherits_value`, and the executing activation already reads its level's
application for `slot_base`, so `InitializeSlot` could branch on a flag the
frame carries rather than walking `receiver -> class -> application` on every
store; that is the change to make if the figure ever matters, and it was left
undone because 3.6% on one instantiation benchmark did not warrant it before
`Class`, `Behavior` and `Metaclass` land and change the same path again.

**Start-up did not move measurably.** The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, wall-clock from the parent process, 100 runs
of each binary over two sittings, the four binaries rotating: baseline medians
5.10 ms and 5.13 ms (10th to 90th percentile 4.89–5.49 and 4.91–5.53), head
4.94 ms and 5.26 ms (4.67–5.36 and 5.00–5.70). The two head builds straddle
the two baseline builds, so the three classes and seven methods the kernel
gained cost less than the scatter between builds. These figures are not
comparable with the core library's move above, which was a different sitting on
a differently loaded machine.

### What mixin application cost

§6.2's mixin application -- one class level per `<:` clause, between a
declaration's own class and its superclass (`nested::build_applied_levels`) --
runs on two paths every program takes: the instantiation walk behind every
`new`, and the nested-class accessor's creation of a class object. Neither
`vm.rs`, `act`, the inline caches nor the compiled code of a send changed.
**A program that writes no `<:` pays one test per climbed level, one per
`descend_levels` call and one per nested-class creation for it**, and nothing
else:

- The climb (`instantiate::climb_levels`) is the ordinary walk, with one test
  per level of whether the superclass is an applied level
  (`Heap::class_is_applied_level`, a read of the applied-clause field). The
  first applied level hands the rest of the climb to
  `climb_through_applied_levels`, `#[cold]` and `#[inline(never)]`, which
  keeps the applying declaration's index and defers each `<:` clause's
  message.
- The descent (`descend_levels`) tests whether any message was deferred and
  otherwise is the ordinary descent; `descend_to_deferred_message` and the
  resumption after one (`advance_past_applied_message`) are out of line the
  same way.
- The accessor's `after_clause` tests whether the declaration has a `<:`
  clause; one that does builds its levels in the out-of-line
  `superclass_with_applied_levels`.
- `check_factory_arity`, `clause_message_body` and `argument_list` are
  `#[inline(always)]`, since the cold paths are second callers of each, and
  the applied clause was boxed, so a class object grew by one word (since road
  step 3a it is one packed field of eight).

**Measured 2026-09-23, `06fd024` against `6833234`**, the commit before
mixin application, by the protocol of "What removing the re-entries moved" above:
three builds of each tree with the pinned flags, none importing
`VCRUNTIME140.dll`, each checked to print the expected answer for every
source; `ns-next bench <source> --iterations 30`, five rounds a sitting, the
binaries rotating, three sittings. The sources are the eight of "What `Top`,
`Value` and `Number` cost" and two more throwaway ones: `bench_newarg2.ns` is
`bench_newarg.ns` with the argument forwarded,
`class Cell y: b = Base x: b + 1 ( | v ::= b. | )`, instantiated as `Cell y: 1`;
`bench_nested.ns` creates 20,000 fresh enclosing objects and instantiates a
nested class of each, `Holder new Inner new v`, so every iteration runs the
accessor's creation path once and two instantiations. A release build with
debug information of each tree puts `vm::run` at 6,508 instructions in both,
one function with no closure of its own. Means in ms, all three sittings
pooled:

| Source | `6833234` | `06fd024` | Change, pooled | Sitting 1 / 2 / 3 |
|---|---|---|---|---|
| `bench_loop.ns` | 7.02 | 7.04 | +0.3% | +0.2% / +0.7% / −0.1% |
| `bench_send.ns` | 8.37 | 8.34 | −0.4% | −0.3% / −0.6% / −0.2% |
| `bench_block.ns` | 18.45 | 18.66 | +1.2% | +1.1% / +0.7% / +1.7% |
| `bench_megamorphic.ns` | 11.44 | 11.46 | +0.2% | +0.4% / −0.0% / +0.1% |
| `bench_do.ns` | 5.73 | 5.72 | −0.0% | −0.0% / +0.2% / −0.2% |
| `bench_new.ns` | 4.17 | 4.19 | +0.5% | +0.3% / +0.9% / +0.4% |
| `bench_new2.ns` | 5.18 | 5.23 | +0.9% | +1.1% / +0.6% / +0.9% |
| `bench_newarg.ns` | 7.34 | 7.18 | −2.2% | −1.7% / −1.4% / −3.4% |
| `bench_newarg2.ns` | 8.27 | 8.39 | +1.4% | +1.5% / +1.1% / +1.5% |
| `bench_nested.ns` | 7.87 | 7.95 | +1.0% | +1.0% / +0.8% / +1.3% |

**All ten are within the protocol's noise**: −2.2% to +1.4%, inside the
0.3–2.3% these sources show between builds of one tree. The five send
sources are level, the groups overlapping, except `bench_block.ns`, which
instantiates nothing and is the source whose build-to-build spread was
measured at 16% above.

**Four instantiating sources still sit above the base**: `bench_new.ns`
+0.5%, `bench_new2.ns` +0.9%, `bench_newarg2.ns` +1.4% and `bench_nested.ns`
+1.0%, every build of `06fd024` above every build of `6833234` in every
sitting, while `bench_newarg.ns` sits 2.2% below. By the criterion of "What
`Top`, `Value` and `Number` cost" that is a real difference, if a small one.
Its cause is not isolated. Placement of data is the likely one: two builds of
the base tree with only an unused field of the new fields' sizes added to
`Mixin` and to `ClassData`, in one sitting of four rounds, moved the five
instantiating sources by −1.8% (`bench_newarg.ns`) to +0.6%
(`bench_nested.ns`), the same direction and much of the size. It is unproven,
though -- one sitting, two builds -- and the one added test per level, per
descent and per nested-class creation is work that remains alongside it.

### What the value conditions cost

Enforcing §3.1.1's four conditions on a value
([object-model.md](object-model.md), "A value object's four conditions")
decides them where a class or an application comes into being and refuses them
at instantiation. Neither `vm.rs`, `act`, the inline caches nor the compiled
code of a send changed. What a release build runs that it did not run before:

- `nested::after_clause` tests `application.inherits_value` once per class
  object created, and only a class that inherits `Value` runs
  `value_class::check_value_class`.
- `Mutator::apply_mixin` folds the root clause defect, the mutable slot and the
  identity method into one `MixinApplication::instantiation_refusal`, once per
  application. `decide_instantiation` already tested that `Option`, where it
  had tested `mutable_value_slot`.
- `decide_instantiation` tests whether `ClassData::value_class_refusal`, a new
  field of the boxed class data, holds a refusal
  (`Heap::class_has_value_refusal`), once per instantiation. It does so only
  for a class whose application inherits `Value`, and reads the refusal
  itself only on the cold error path (`value_class_refusal_error`).
- The kernel was `Object <: Value` then, so bootstrap built one applied
  level for it.
- The image decoder has a second pass that recomputes each decoded class
  object's verdict. No source below loads an image.

**The expectation, recorded before any binary was built.** Level on every
source. The one new test on a path the loops run is the `inherits_value` test
on the value-class read in `decide_instantiation`, and only the instantiating
sources reach it. Start-up is level too: one applied level is small beside the
kernel's compilation.

**Measured 2026-09-24, `d773380` against `cdc0257`**, the merge of mixin
application and this branch's start, by the protocol of "What removing the
re-entries moved" above. Every binary was built with the pinned flags. None
imports `VCRUNTIME140.dll` (`dumpbin -dependents`), and each was checked to
print the expected answer for every source first. The timing is
`ns-next bench <source> --iterations 30`, five rounds a sitting, the six
binaries rotating by one position from round to round. The sources are the four
fixtures under `benchmarks/micro/` and the throwaway `bench_do.ns`,
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` of that protocol, and one
more throwaway source, `bench_newvalue.ns`. Its module is `Object <: Value`, and
its loop makes 20,000 instantiations of `class Cell = Value new ( | public v =
1. | )`, each followed by `v`. It is the one source that reads the new field.

**Three builds of one tree are one binary.** Building a tree three times
(separate target directories, or separate checkouts at paths of different
lengths) gave executables that differ in 20 to 24 bytes: the link timestamp
and the debug-information identifier. Their code and data are byte for byte
the same. The first three sittings timed three such copies of each tree, and
each tree's copies agreed to within 0.3–0.5% on `bench_block.ns`. The 16%
spread that "What `Top`, `Value` and `Number` cost" found between builds of one
tree did not appear. For builds that actually differ in layout, the protocol
used here builds each tree three ways: unperturbed, and twice with an extra
`-C metadata=<word>` in `RUSTFLAGS` (`layoutprobe` and `secondprobe`). The
extra metadata changes every symbol hash, and with it the link order. About
1.85 MB of the 2.4 MB binary differs between them. The tables below are three
more sittings over those six binaries: `base`, `base-p` and `base-q` from
`cdc0257`, and `head`, `head-p` and `head-q` from `d773380`.

**`vm::run`'s step is still one function.** A release build with debug
information of each tree has one function label matching `vm::run` in each,
and no `vm::run::closure` of its own. The base has **6,508 instructions**,
the head 6,507. `decide_instantiation` grew from 441 to 503 instructions and
`after_clause` from 231 to 433. The new refusal errors
(`instantiation_refusal_error`, `value_class_refusal_error`) are out of line.
Until the review's fix (below), `decide_instantiation` cloned the class's
`Option<ValueClassRefusal>` to test it, and `value_class_refusal_error` was
inlined into it; since `7b2af64` it is **371 instructions**, below the base's
441.

Means in ms per build, over each sitting's five rounds; sittings 4, 5 and 6:

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.13 | 7.04 | 7.05 | 7.07 | 7.04 | 7.01 |
| `bench_send.ns` | 8.29 | 8.53 | 8.32 | 8.36 | 8.31 | 8.64 |
| `bench_block.ns` | 18.66 | 18.60 | 18.45 | 19.58 | 19.33 | 19.28 |
| `bench_megamorphic.ns` | 11.45 | 11.51 | 11.44 | 11.45 | 11.49 | 11.47 |
| `bench_do.ns` | 5.69 | 5.71 | 5.70 | 5.67 | 5.75 | 5.72 |
| `bench_new.ns` | 4.20 | 4.18 | 4.21 | 4.30 | 4.26 | 4.27 |
| `bench_new2.ns` | 5.23 | 5.23 | 5.29 | 5.26 | 5.22 | 5.24 |
| `bench_newarg.ns` | 7.19 | 7.16 | 7.14 | 7.10 | 7.11 | 7.16 |
| `bench_newvalue.ns` | 3.80 | 3.81 | 3.81 | 3.71 | 3.67 | 3.67 |

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.04 | 7.01 | 7.01 | 7.06 | 7.11 | 7.09 |
| `bench_send.ns` | 8.35 | 8.37 | 8.29 | 8.36 | 8.56 | 8.31 |
| `bench_block.ns` | 18.51 | 18.39 | 18.53 | 19.31 | 19.34 | 19.36 |
| `bench_megamorphic.ns` | 11.44 | 11.47 | 11.48 | 11.50 | 11.48 | 11.54 |
| `bench_do.ns` | 5.68 | 5.74 | 5.71 | 5.70 | 5.69 | 5.69 |
| `bench_new.ns` | 4.18 | 4.21 | 4.21 | 4.30 | 4.31 | 4.30 |
| `bench_new2.ns` | 5.21 | 5.22 | 5.28 | 5.27 | 5.22 | 5.33 |
| `bench_newarg.ns` | 7.17 | 7.18 | 7.14 | 7.13 | 7.15 | 7.09 |
| `bench_newvalue.ns` | 3.81 | 3.80 | 3.80 | 3.66 | 3.71 | 3.66 |

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.02 | 7.02 | 7.07 | 7.00 | 7.06 | 6.99 |
| `bench_send.ns` | 8.34 | 8.35 | 8.28 | 8.41 | 8.32 | 8.31 |
| `bench_block.ns` | 18.55 | 18.55 | 18.71 | 19.39 | 19.45 | 19.34 |
| `bench_megamorphic.ns` | 11.49 | 11.42 | 11.50 | 11.45 | 11.42 | 11.44 |
| `bench_do.ns` | 5.68 | 5.74 | 5.70 | 5.68 | 5.69 | 5.68 |
| `bench_new.ns` | 4.17 | 4.16 | 4.19 | 4.30 | 4.25 | 4.28 |
| `bench_new2.ns` | 5.23 | 5.21 | 5.26 | 5.25 | 5.20 | 5.36 |
| `bench_newarg.ns` | 7.19 | 7.18 | 7.14 | 7.16 | 7.16 | 7.07 |
| `bench_newvalue.ns` | 3.82 | 3.78 | 3.80 | 3.67 | 3.69 | 3.66 |

The three sittings pooled, the base's three layouts against the head's three,
and, in the last column, the first three sittings' three identical copies of
each tree, pooled the same way:

| Source | `cdc0257` | `d773380` | Change, pooled | Sitting 4 / 5 / 6 | Sittings 1–3, identical copies |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.04 | 7.05 | +0.1% | −0.4% / +1.0% / −0.3% | +0.0% |
| `bench_send.ns` | 8.35 | 8.40 | +0.6% | +0.7% / +0.9% / +0.3% | −0.1% |
| `bench_block.ns` | 18.55 | **19.38** | **+4.5%** | +4.5% / +4.7% / +4.3% | +3.3% |
| `bench_megamorphic.ns` | 11.47 | 11.47 | +0.0% | +0.0% / +0.4% / −0.3% | +0.4% |
| `bench_do.ns` | 5.71 | 5.70 | −0.2% | +0.2% / −0.3% / −0.4% | −0.5% |
| `bench_new.ns` | 4.19 | **4.28** | **+2.2%** | +1.9% / +2.4% / +2.4% | +2.7% |
| `bench_new2.ns` | 5.24 | 5.26 | +0.4% | −0.2% / +0.8% / +0.7% | +0.4% |
| `bench_newarg.ns` | 7.17 | 7.12 | −0.6% | −0.6% / −0.6% / −0.5% | −0.7% |
| `bench_newvalue.ns` | 3.80 | 3.68 | −3.3% | −3.2% / −3.2% / −3.4% | −2.7% |

**Six of the nine are level**: −0.6% to +0.6%, the groups overlapping.
`bench_newvalue.ns`, the one source that reads the new field, is 3.3%
*faster*, every head layout below every base layout in every sitting. Nothing
on its path got cheaper, so the change does not explain it.

**Open finding: `bench_block.ns` is 4.5% slower and `bench_new.ns` 2.2%
slower**, and both are real by this protocol's criterion. Every head layout
lies above every base layout in all three sittings, and the three identical
copies in the first three sittings show the same (+3.3% and +2.7%). Neither
source falls inside build noise. The spread between one tree's three layouts
is 0.3–0.6% on each, and `bench_block.ns` did not show its 16% build spread in
any of the twelve binaries. The expectation said level, and it did not hold
for these two. Re-measured after the review's fix (below), `bench_new.ns`'s
gap did not reproduce, before the fix or after it, and `bench_block.ns`'s
did.

The causes are not isolated. `bench_block.ns` creates no class and
instantiates nothing in its loop. It makes lexical sends of its slots and
`bump:`, public sends of `<`, `+` and `value:`, and one block creation per
iteration, and `vm::run` is the same size in both trees. What the head changed
that such a loop can reach is data, not code on its path: the kernel instance
is now a value whose class has one applied level more, which is the enclosing
object of every builtin class, `Closure` and `Integer` included, and every class
object's `ClassData` grew by `value_class_refusal`. `bench_new.ns` instantiates
a class that is no value. On its path the new `inherits_value` test sits behind
a refusal test it already had, inside a `decide_instantiation` that grew by 62
instructions. Yet `bench_new2.ns` and `bench_newarg.ns`, which run the same
function with one level more, are level or faster. A control that would
separate the kernel's applied level from the Rust changes, the head with
`Kernel.ns`'s root put back to `Object new`, does not bootstrap:
`kernel.rs` then asserted the kernel's root was `Object <: Value`. Nothing was
profiled.

**Re-measured 2026-09-24 after the review's fix, `7b2af64`.** The final
review named the clone of `Option<ValueClassRefusal>` on
`decide_instantiation`'s path, and the inlined refusal error behind it, as
the concrete candidate for `bench_new.ns`. The fix tests for the refusal's
presence and keeps both refusal errors `#[cold]` and `#[inline(never)]`. It
was measured in a new session by the same protocol, over `bench_block.ns`
and `bench_new.ns` only: three trees, `cdc0257` (`base`), `d9c3f33` (the
head before the fix, `old`) and `7b2af64` (`head`), each built plain and with
`-C metadata=layoutprobe` and `=secondprobe`, nine binaries rotating by one
position from round to round, five rounds a sitting and six sittings. A
rebuild of the plain base differed from the first in 24 bytes, as before.
`vm::run` is still 6,507 instructions with no `vm::run::closure`.
Means in ms per layout over the six sittings (plain, `-p`, `-q`):

| Source | `cdc0257` | `d9c3f33` | `7b2af64` | `7b2af64` against `cdc0257`, pooled | against `d9c3f33` |
|---|---|---|---|---|---|
| `bench_block.ns` | 17.27, 18.55, 18.53 | 19.43, 19.39, 19.34 | 19.43, 19.32, 19.33 | +6.9% (+6.7 to +7.1 per sitting) | −0.1% |
| `bench_new.ns` | 4.30, 4.22, 4.23 | 4.34, 4.22, 4.22 | 4.34, 4.21, 4.23 | +0.2% (−1.3 to +0.9 per sitting) | +0.0% |

**`bench_new.ns` is level, and the fix is not what made it so.** The head
before the fix measured the same as the head after it, level with the base,
so the +2.2% above did not reproduce in this session for either tree. The
head's plain build lies about 3% above its two other layouts in every
sitting of both measurements here, and the plain build before the fix does
the same; the base's plain build did so in two of the first six sittings and
in none of the second six. The `-C metadata` layouts of all three trees lie
within 0.5% of one another. What the fix did do is shrink
`decide_instantiation` back below the base.

**`bench_block.ns` is still slower, and the fix did not touch it** (−0.1%
against the head before it), as expected of a source that instantiates
nothing. The pooled +6.9% overstates it: the base's plain build is a layout
outlier in this session, 17.27 ms against 18.55 and 18.53 for its other two
layouts, in every one of twelve sittings across both measurements here.
Against the base's two `-C metadata` layouts alone the head is **4.4%**
slower, which is the finding above.

**Control (a), the seeded symbol: refuted as the cause.** The head seeds
`==` as a well-known symbol (`Symbol::IDENTICAL`), which shifts every later
symbol id by one and with it every `IdentityMap<Symbol, …>` bucket. A
throwaway build of the base with the same seed line, in the same three
layouts, measured in six more sittings beside the base and the head:

| Source | `cdc0257` | `cdc0257` + seed | `7b2af64` | head against base + seed, per sitting |
|---|---|---|---|---|
| `bench_block.ns` | 17.30, 18.49, 18.51 | 18.53, 18.50, 18.54 | 19.44, 19.25, 19.26 | +4.3% (+3.6 to +4.6) |
| `bench_new.ns` | 4.23, 4.23, 4.24 | 4.22, 4.23, 4.22 | 4.35, 4.22, 4.24 | +1.0% (+0.1 to +1.9) |

The seeded base's three layouts lie with the base's two `-C metadata` ones,
and the head is as far above them as before, so the shifted symbol ids
explain none of `bench_block.ns`'s gap. The seed did move the base's plain
build from 17.30 to 18.53 ms, a layout change like any other. On
`bench_new.ns` the head's plain build is again the one 3% above the rest.
The review's two other hypotheses remain open ([OPEN-WORK.md](../OPEN-WORK.md)):
the kernel's extra `Value`-level class object,
which shifts every later arena index and dispatch id by one, and hardware
counters.

**Start-up did not move.** The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, timed wall-clock from the parent process.
There were 25 rounds of the six layout binaries rotating, in two sittings, so
75 runs of each tree a sitting. The base's medians were 4.95 ms and 4.99 ms
(10th to 90th percentile 4.83–5.23 and 4.79–5.20). The head's were 4.93 ms and
4.90 ms (4.80–5.11 and 4.73–5.14). The kernel's applied level costs less than
the scatter.

### What the class accessors cost

Answering a predefined class through an accessor on `Object`
([core-library.md](core-library.md); [dispatch.md](dispatch.md), "Primitive
methods") changed what a release build runs where a program names one. A bare
builtin name such as `Array` was a constant load, `LoadBuiltinClass`; it is now
a self send, `SendLexical` at depth 0, whose inline-cache hit reaches `act`,
and `act` answers the class off the method's `MethodFastPath::BuiltinClass`
without an activation. A nested superclass clause such as `= Object new` sends
`Object` the same way, once per class object it creates. `LoadBuiltinClass`'s
arm is gone from `vm::run`. What every send now pays is one more arm in `act`'s
test of `method.fast_path`, which was a test of `primitive` before.

**Which sources can move.** None of the four fixtures under `benchmarks/micro/`
names a builtin class in its loop. Each names `Object` only in its module's
superclass clause, which runs once when the module is instantiated, and
`bench_megamorphic.ns` also in its five nested classes' `= Object new`
clauses, each of which runs once when its class object is first created,
before the loop. The throwaway `bench_new.ns`, `bench_new2.ns`,
`bench_newarg.ns` and `bench_newvalue.ns` name `Object` or `Value` only in the
clauses of classes their loops instantiate, and a class object is created once
per enclosing instance, so their loops send no class accessor either. One more
throwaway source therefore exercises the change: `bench_builtin.ns` runs
`sum:: sum + Array new size + OrderedCollection new size + 1` 20,000 times, so
its loop makes 40,000 class-accessor sends (`SendLexical #Array` and
`#OrderedCollection` in its disassembly) where the base made 40,000
`LoadBuiltinClass`es.

**Measured 2026-09-24, `dd1c70c` against `909bf40`**, the merge of the value
conditions and this branch's start, by the protocol of "What removing the
re-entries moved" above, with the layout variation of "What the value
conditions cost": each tree built plain and with `-C metadata=layoutprobe` and
`=secondprobe`, every build with the pinned flags. The measured tree was the
branch head, `96d5a8d`, whose commits after `dd1c70c` change documentation
only. The plain build and a `-C metadata` build of one tree differ in 1.55 to
1.86 million bytes. None of the six imports `VCRUNTIME140.dll`, and each printed
the expected answer for every source first. The timing is
`ns-next bench <source> --iterations 30`, five rounds a sitting, the six
binaries rotating by one position from round to round, four sittings. The
sources are the four fixtures, the five throwaway sources of "What the value
conditions cost" (`bench_newvalue.ns` with its sum in a local, since its module
is a value and may declare no mutable slot) and `bench_builtin.ns`.

**`vm::run`'s step is still one function, and it grew.** A release build with
debug information of each tree, `dumpbin -disasm`: one function label matching
`vm::run` in each and no `vm::run::closure`. The base has **6,507
instructions**, the head **6,626**, 119 more although an arm was removed. `act`
has no label of its own in either listing, so it is inlined, and the functions
it is inlined into grew alike: `inline_cache::global_hit` from 909 to 926,
`send_lexical_first` from 577 to 601, and each of the three
`inline_cache::miss` instances by 60 to 63. The likely cause of `vm::run`'s
growth is the same one, the new `fast_path` arm at each place `act` is inlined
into it; that was not isolated. Storing the builtin classes in a fixed-size array rather than a
`Vec` would drop `builtin_class`'s bounds check from each of those sites; it
waits on a bootstrap restructuring ([OPEN-WORK.md](../OPEN-WORK.md)).

Means in ms per layout, the four sittings pooled:

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.29 | 7.29 | 7.23 | 7.23 | 7.19 | 7.19 |
| `bench_send.ns` | 8.61 | 8.58 | 8.68 | 8.53 | 8.63 | 8.49 |
| `bench_block.ns` | 19.02 | 19.88 | 19.83 | 19.20 | 19.04 | 19.10 |
| `bench_megamorphic.ns` | 11.83 | 11.80 | 11.82 | 11.83 | 11.78 | 11.86 |
| `bench_do.ns` | 5.91 | 6.12 | 6.13 | 5.85 | 5.82 | 5.85 |
| `bench_new.ns` | 4.28 | 4.28 | 4.27 | 4.28 | 4.31 | 4.32 |
| `bench_new2.ns` | 5.34 | 5.32 | 5.37 | 5.33 | 5.37 | 5.40 |
| `bench_newarg.ns` | 7.33 | 7.30 | 7.27 | 7.26 | 7.34 | 7.40 |
| `bench_newvalue.ns` | 3.76 | 3.74 | 3.79 | 3.80 | 3.79 | 3.81 |
| `bench_builtin.ns` | 3.93 | 3.94 | 3.94 | 4.02 | 4.00 | 3.97 |

The base's three layouts against the head's three:

| Source | `909bf40` | `dd1c70c` | Change, pooled | Sitting 1 / 2 / 3 / 4 |
|---|---|---|---|---|
| `bench_loop.ns` | 7.27 | 7.21 | −0.9% | −1.4% / −1.1% / −0.7% / −0.2% |
| `bench_send.ns` | 8.63 | 8.55 | −0.9% | −1.8% / −0.8% / +0.0% / −0.9% |
| `bench_block.ns` | 19.58 | 19.12 | −2.4% | −2.2% / −2.3% / −2.5% / −2.5% |
| `bench_megamorphic.ns` | 11.81 | 11.82 | +0.1% | −0.0% / +0.3% / −0.1% / +0.2% |
| `bench_do.ns` | 6.05 | 5.84 | −3.5% | −3.6% / −3.2% / −3.9% / −3.4% |
| `bench_new.ns` | 4.28 | 4.30 | +0.5% | +1.1% / +0.2% / +0.7% / +0.2% |
| `bench_new2.ns` | 5.34 | 5.37 | +0.4% | +0.7% / +0.4% / +0.4% / +0.1% |
| `bench_newarg.ns` | 7.30 | 7.33 | +0.5% | −0.2% / −0.1% / +1.6% / +0.5% |
| `bench_newvalue.ns` | 3.76 | 3.80 | +0.9% | +1.0% / +0.4% / +1.0% / +1.3% |
| `bench_builtin.ns` | **3.94** | **3.99** | **+1.5%** | +1.9% / +1.0% / +0.8% / +2.1% |

**`bench_builtin.ns`, the one source whose loop names a builtin, is 1.5%
slower.** Pooled over the four sittings every head layout (3.97–4.02 ms) lies
above every base layout (3.93–3.94 ms), whose spread is 0.2%; within single
sittings the slowest base layout touches the fastest head layout in two of
the four. That is about 0.06 ms over 40,000 sends, some 1.4 ns for each, the
difference between an inline-cache hit followed by `act` and a constant load.
The loop also allocates two collections and sends `new` and `size` to each,
so the naming is a small part of what it runs; a loop that did little but name
classes would show a larger share.

**The rest are level.** `bench_new.ns` to `bench_newvalue.ns` sit 0.4–0.9%
above the base with the groups overlapping in every sitting; `bench_loop.ns`,
`bench_send.ns` and `bench_megamorphic.ns` are level or below. `bench_block.ns`
and `bench_do.ns` show the head 2.4% and 3.5% *faster*, which is layout, not
the change: the base's two `-C metadata` layouts lie 4% above its plain
build on both, while the head's three layouts and the base's plain build lie
together (19.02 against 19.04–19.20 ms, and 5.91 against 5.82–5.85 ms).
Neither source names a builtin in its loop. The open finding of "What the
value conditions cost", `bench_block.ns` 4.4% slower at the value conditions'
head, is the base of this measurement, and this measurement does not bear on
it: here its plain layout is the fast one.

**Start-up did not move.** The whole `ns-next run` of a module whose
`main:args:` answers `0` at once, timed wall-clock from the parent process,
25 rounds of the six binaries rotating, in two sittings, so 75 runs of each
tree a sitting. The base's medians were 5.49 ms and 5.58 ms (10th to 90th
percentile 5.21–5.89 and 5.19–6.08), the head's 5.56 ms and 5.63 ms
(5.27–5.91 and 5.32–6.17): 0.05–0.07 ms apart, inside the scatter.

### What `True` and `False` as quick methods cost

Writing `True` and `False` in Newspeak over quick methods
([core-library.md](core-library.md), "`True` and `False` are quick methods")
changed what a release build runs where a Boolean method is *sent* rather
than inlined: `not`, `&`, `|`, and a conditional, `and:` or `or:` whose
argument is not a literal block. Each was a primitive method whose mirror row
ran a leaf; each is now answered by `act` off the method's
`MethodFastPath::Constant`, `Parameter` or `ValueOfParameter`, with no leaf
and no activation. What every method send now pays is three more arms in
`act`'s test of `method.fast_path`; `vm::run` grew from 6,626 to 6,818
instructions and is still one function, with no `vm::run::closure` of its
own (`dumpbin -disasm` on a release build with debug information, both
trees).

**Which sources can move.** The four fixtures under `benchmarks/micro/` send
no Boolean method in their loops: their conditionals and loops take literal
blocks and are inlined. One throwaway source exercises the change,
`bench_boolean.ns`, a 200,000-iteration `whileTrue:` loop whose body is
`flag:: flag not. flag ifTrue: block. (flag & true) ifTrue: block.
(flag | false) ifFalse: block`, with `block` a variable holding
`[taken:: taken + 1]` — four sent Boolean methods and three sent
conditionals per iteration; it prints `300000` under both trees.

**Measured 2026-09-24, `e8a2884` against `ddd9e4f`**, the `master` it started
from -- before the fix that marks the block a control-flow step only for a
Boolean receiver (`d4e7924`), one test of the receiver on the quick path,
which was not measured separately -- by the protocol of "What removing the
re-entries moved" above: two builds of each tree, every build with the pinned
flags, the four binaries differing byte for byte;
`ns-next bench <source> --iterations 30`, five rounds of all four binaries in
turn, after one sitting of five rounds of one build each. Medians of the five
rounds, in ms:

| Source | before | before, second build | after | after, second build | Pooled |
|---|---|---|---|---|---|
| `bench_boolean.ns` | 55.56 | 55.93 | 43.71 | 43.85 | −21.5% |
| `bench_block.ns` | 19.79 | 19.85 | 19.61 | 19.68 | −0.9% |
| `bench_megamorphic.ns` | 11.66 | 11.73 | 11.63 | 11.66 | −0.6% |
| `bench_send.ns` | 8.42 | 8.44 | 8.59 | 8.53 | +1.3% |
| `bench_loop.ns` | 7.06 | 7.08 | 7.30 | 7.13 | +1.4% |

`bench_boolean.ns` is entirely below: every round of either after-build is
faster than every round of either before-build, 43.2–44.2 ms against
55.0–57.0 in the sitting the table reports, and 43.3–44.2 against 55.0–56.3
in the first. The four fixtures are level by the protocol's gate: for each,
the after-rounds overlap the before-rounds — in the table's sitting,
`bench_send.ns` 8.40–8.67
against 8.31–8.69, `bench_loop.ns` 7.11–7.38 against 7.00–7.28, where the
two after-builds of `bench_loop.ns` alone are 2.4% apart.

### What `Behavior`, `Class` and `Metaclass` cost

Reifying §3.2's class hierarchy ([object-model.md](object-model.md), "Class
sides and metaclasses") changed three things a release build runs.
`resolve_public` and `resolve_lexical` lost their `is_class` branch: a class
object is dispatched on its metaclass's instances id like any object, and the
class-side chains are gone. Every class object is created with its metaclass
(`Mutator::attach_metaclass`), so a nested class object made per enclosing
instance costs two class objects where it cost one. A class object's `==` and
`=` go through `Behavior`'s forwarders to the `value:sameAs:ifFail:` leaf.

**Which sources can move.** The four fixtures under `benchmarks/micro/` create
no class object in their loops and send no class-side message there. One
throwaway source exercises the change, `bench_metaclass.ns`: a
100,000-iteration `whileTrue:` loop whose body is `Outer new Inner make`,
with `Inner` a public nested class of `Outer` whose class side declares
`make = ( ^self new )` -- per iteration one instance of `Outer`, one class
object `Inner` with its metaclass, one class-side send and one factory send.
It prints `100000` under both trees.

**Measured 2026-09-25, `1c1f624` against `3defb78`**, the `master` it started
from, by the protocol of "What removing the re-entries moved" above, with the
layout variation of "What the value conditions cost": each tree built plain
and with `-C metadata=layoutprobe` and `=secondprobe`, every build with the
pinned flags, the six binaries differing byte for byte. None imports
`VCRUNTIME140.dll`, and each printed the expected answer for every source
first. The timing is `ns-next bench <source> --iterations 30`, five rounds a
sitting, the six binaries rotating by one position from round to round, four
sittings. Medians in ms per layout, the four sittings pooled (the median of
the 20 round medians):

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.24 | 7.22 | 7.25 | 7.18 | 7.19 | 7.18 |
| `bench_send.ns` | 8.51 | 8.56 | 8.52 | 8.57 | 8.56 | 8.61 |
| `bench_block.ns` | 16.90 | 16.00 | 16.23 | 16.07 | 18.60 | 18.66 |
| `bench_megamorphic.ns` | 11.68 | 11.73 | 11.66 | 11.37 | 11.37 | 11.44 |
| `bench_metaclass.ns` | 38.92 | 38.62 | 38.51 | 44.34 | 44.07 | 44.22 |

The base's three layouts against the head's three, means of the round
medians, and the range of every round median of each tree:

| Source | `3defb78` | `1c1f624` | Change, pooled | Sitting 1 / 2 / 3 / 4 | Rounds, base / head |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.35 | 7.24 | −1.4% | −1.0% / −1.2% / −2.3% / −1.1% | 7.13–8.36 / 7.07–7.50 |
| `bench_send.ns` | 8.59 | 8.64 | +0.5% | +0.5% / +0.8% / +0.1% / +0.8% | 8.41–9.62 / 8.48–9.63 |
| `bench_block.ns` | 16.42 | 17.81 | see below | +8.2% / +9.0% / +8.7% / +8.0% | 15.82–18.95 / 15.85–19.28 |
| `bench_megamorphic.ns` | 11.70 | 11.45 | −2.1% | −1.4% / −2.5% / −2.0% / −2.7% | 11.45–12.15 / 11.27–12.34 |
| `bench_metaclass.ns` | 38.68 | 44.29 | **+14.5%** | +14.5% / +14.3% / +14.8% / +14.4% | 38.09–39.34 / 43.32–45.79 |

**The fixtures are level.** `bench_send.ns` is within 0.5% and every layout
of the head lies inside the base's range; `bench_loop.ns` and
`bench_megamorphic.ns` are 1.4% and 2.1% faster, in every sitting.
`bench_block.ns` is the bimodal source of "What `Top`, `Value` and `Number`
cost": each tree has a fast layout near 16.0 ms and a slow one near 18.6, and
the lottery gave the base two fast layouts (16.00, 16.23) and the head one
(16.07). Fast layout against fast layout the head is 0.4% slower than
`base-p`, which is the comparison that means anything here; the pooled +8.5%
counts layouts, not the change. None of the four creates a class object or
sends a class-side message in its loop.

**`bench_metaclass.ns` is 14.5% slower**, about 5.6 ms over 100,000
iterations, some 56 ns for each `Outer new Inner make`: the metaclass built
with each class object `Inner`. Every round of the head is above every round
of the base.

**The step `baf6f65` put on a hit, and its cause.** Measured at the branch's
last feature commit, `f6e2b64`, the fixtures were not level: `bench_send.ns`
6.0% slower (8.58 → 9.10 ms), `bench_loop.ns` and `bench_megamorphic.ns`
3.4% of the pooled medians before they are rounded to two places for display
here (7.31 → 7.57, 11.73 → 12.12 -- recomputing from the rounded pair alone
gives 3.6% and 3.3%), by the same protocol; `bench_metaclass.ns` was 17.3%
slower at `f6e2b64` (38.76 → 45.45 ms), against the 14.5% it measures at
`1c1f624` above. A pinned build of every commit put the whole step of the
two send sources on `baf6f65`, "a class object's class is its metaclass".
Nothing on the path of a cache hit changed there -- the size of `Target`,
`CacheEntry`, `Resolved` and `Step` did not move (only `ClassData` and
`DispatchIds` shrank), `--cache-statistics`
and `--collector-statistics` report the same fills and no collection for
both commits, and `vm::run` calls the same set of functions out of line --
so the step was code generation. A sampling profile (`samply` at 8 kHz,
1,000 iterations of `bench_send.ns`, `dumpbin -disasm` of the same build)
put the extra time in `vm::run` (+3,800 of about 74,000 samples), and 2,500
to 3,000 of it on one instruction: the 16-byte load of a send's receiver
that builds `Step::Method` for `tick:`, just after that receiver had been
stored into the same slot as five pieces -- a byte for the tag, a byte for a
Boolean, the two bytes of padding, a handle's doubleword and the payload
word. A load that spans several narrower stores cannot be forwarded from the
store buffer and waits for them to retire.

The pieces came from the `SendKey` a cached send carries. It held the
receiver as a `Value`, and once LLVM's SROA split the key's copy into the
pieces the hit path reads (the tag, a Boolean's byte for its immediate id, an
object's handle), every later whole `Value` -- the temporary a native takes by
value, `Step::Method`'s receiver, the key a megamorphic send hands `miss` --
was stored back piece by piece. `baf6f65` removed `chain_start`'s class-side
arm, which read the key's receiver to find a metaclass; with it gone SROA
split the key where before it had kept one memory copy. Putting a dead arm
of the same shape back restores the base's time (8.61 against 9.13 ms on
`bench_send.ns`, 11.67 against 12.19 on `bench_megamorphic.ns`), which is
what located it; the arm alone, without the read of the receiver, restores
about half. The fix is `1c1f624`: `SendKey` holds the receiver by
reference, to the send's own local, so the local stays one object in memory
and every whole copy of it is a single 16-byte move, whatever SROA does to
the rest of the key. `vm::run` shrank from 6,989 to 6,741 instructions, its
samples in that profile from 46,300 to 42,500, and the stalled load is gone.

**What the profile shows beyond it.** The same kind of stall costs every
tree, the base included, far more than this step did. In `bench_send.ns`
about a quarter of all samples sit on three 16-byte loads in `vm::run` of a
`Value` a callee has just written through its out-parameter with stores that
do not line up with the load: the `Step::Value` `decide_slot_accessor`
answers (a byte for the tag, two overlapping quadwords for the rest), at two
sites, and the literal `vm::make_value` answers. `decide_slot_accessor`
stalls the same way inside (another quarter of the samples, most of them on
two instructions), and so does `push_activation` on the activation
description `push_step` writes as a doubleword and a quadword and reads as
one 16-byte field. None of them moved with this branch. A value handed
back in a form that is written the way it is read is what the one-word value
gives ([object-model.md](object-model.md), "Values and objects"; "What the
eight-byte value moved"). This profile predates `1d7587f` (below,
"What exceptions cost"), which moved `decide_slot_accessor`'s error out of
line and with it the spill inside the accessor; the shares above have not
been profiled again since.

**`vm::run`'s step is still one function.** A release build with debug
information of each tree, `dumpbin -disasm`: one function label
`ns_runtime::vm::run` in each and no `vm::run::closure`. The base has **7,055
instructions**, `f6e2b64` **6,989** and `1c1f624` **6,741**. From the base
to `f6e2b64`, `resolve_public` grew from 148 to 178 instructions and
`resolve_lexical` from 319 to 377, both now reaching `resolved_for`, which
has no label of its own and so is inlined into each,
`inline_cache::global_hit` shrank from 1,040 to 1,004, and each of the three
`inline_cache::miss` instances grew by 135 to 144; `resolve_class_side` is
gone.

### What exceptions cost

Exceptions ([core-library.md](core-library.md), "Exceptions") changed four
things a release build runs whether or not a program uses them. A signal
leaving an activation goes to the one unwinder (`interpreter::unwind`, "Unwinding"
above), which reads each popped activation's unwind marker; a counted push
checks its depth with the one compare it had, the reserve's logic moved to a
cold `push_past_the_limit`; `Continuation` gained the variants an unwind and
a signalled VM error resume with; and a `doesNotUnderstand:` now runs
`Object>>doesNotUnderstand:`, which builds a `MessageNotUnderstood` and a
`Message`, where a mirror row answered it in Rust.

**The sources.** The four fixtures under `benchmarks/micro/` use no exception.
Four throwaway sources, not fixtures, each printing what it accumulates:

- `bench_ensure.ns`: 100,000 iterations of
  `[count:: count + 1] ensure: [total:: total + 1]` and
  `[count:: count + 1] on: Error do: [:e | 0]`, nothing signalled.
- `bench_value.ns`: its control, the same three block bodies each sent
  `value`, runnable on both trees.
- `bench_signal.ns`: 10,000 iterations of
  `[self down: 19] on: Error do: [:e | 1]`, where `down:` recurses twenty
  activations and the last signals `Error signal: 'deep'`.
- `bench_deep.ns`: its control, the same recursion answering `1` instead of
  signalling, sent `value` in a block, runnable on both trees.

`bench_ensure.ns` and `bench_signal.ns` cannot run on the base, which has no
`platform kernel`, so they are measured on the head alone against their
controls.

**Measured 2026-09-26, `3d5f69c` against `3cbfd31`**, the `master` it
started from, by the protocol of "What `Behavior`, `Class` and `Metaclass`
cost" above: each tree built plain and with `-C metadata=layoutprobe` and
`=secondprobe`, every build with the pinned flags, the six binaries
differing byte for byte, none importing `VCRUNTIME140.dll`, each printing
the expected answer for every source it can run first;
`ns-next bench <source> --iterations 30`, five rounds a sitting, the six
binaries rotating by one position from round to round, four sittings.
Medians in ms per layout, the four sittings pooled (the median of the 20
round medians):

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.32 | 7.35 | 7.30 | 7.19 | 7.20 | 7.22 |
| `bench_send.ns` | 8.79 | 8.75 | 8.74 | 8.92 | 8.88 | 8.90 |
| `bench_block.ns` | 16.45 | 19.25 | 19.24 | 20.20 | 20.23 | 20.19 |
| `bench_megamorphic.ns` | 11.62 | 11.64 | 11.66 | 11.50 | 11.50 | 11.45 |
| `bench_value.ns` | 43.90 | 44.07 | 44.00 | 45.88 | 46.09 | 46.07 |
| `bench_deep.ns` | 16.34 | 16.18 | 16.24 | 17.31 | 17.35 | 17.21 |
| `bench_ensure.ns` | -- | -- | -- | 60.66 | 60.36 | 60.40 |
| `bench_signal.ns` | -- | -- | -- | 78.44 | 82.89 | 82.82 |

The base's three layouts against the head's three, means of the layout
medians, and the range of every round median of each tree:

| Source | `3cbfd31` | `3d5f69c` | Change, pooled | Sitting 1 / 2 / 3 / 4 | Rounds, base / head |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.32 | 7.21 | −1.6% | −1.0% / −1.1% / −1.2% / −1.5% | 7.15–8.48 / 7.04–8.30 |
| `bench_send.ns` | 8.76 | 8.90 | +1.6% | +1.8% / +2.4% / +0.7% / +1.3% | 8.59–9.66 / 8.65–9.54 |
| `bench_block.ns` | 18.31 | 20.21 | see below | +10.4% / +10.4% / +10.1% / +10.5% | 16.22–19.70 / 19.76–20.65 |
| `bench_megamorphic.ns` | 11.64 | 11.48 | −1.3% | −1.3% / −1.9% / −1.3% / −1.0% | 11.42–12.05 / 11.34–11.95 |
| `bench_value.ns` | 43.99 | 46.01 | **+4.6%** | +5.0% / +3.7% / +4.3% / +3.7% | 43.05–45.71 / 45.17–47.87 |
| `bench_deep.ns` | 16.26 | 17.29 | **+6.4%** | +6.5% / +6.3% / +6.4% / +6.0% | 16.04–16.93 / 17.08–17.75 |

**Two fixtures are level, one is 1.6% slower.** `bench_loop.ns` and
`bench_megamorphic.ns` are 1.6% and 1.3% faster in every sitting.
`bench_send.ns` is 1.6% slower in every sitting, though the ranges of the
two trees' rounds overlap.

**Block activation and deep returns were slower, beyond noise, and the
cause was two defects, one of them older than the branch.** At `3d5f69c`
`bench_block.ns`, the bimodal source, was 5.0% slower slow layout against
slow layout (the base drew one fast layout, 16.45, the head none),
`bench_value.ns` 4.6% and `bench_deep.ns` 6.4%, with nothing of the
exception protocol running in either control. Bisected over the branch's
commits, two builds of each, the two controls moved at different commits,
and each has its own cause:

- **`bench_deep.ns`: `Signal` grew.** It moved at `8aa49e8`, which gave
  `Signal` a `Terminate(NewspeakError)` beside `Error(NewspeakError)`. With
  two variants holding a `NewspeakError` in place the others no longer fit
  in its niche: `Signal` went from 48 bytes to 56, `EvaluationResult` from
  48 to 56 and every `Result<Step, Signal>` a send decides from 56 to 64
  (`size_of` in release builds of both trees; `Step`, `Flow`,
  `Continuation` and `Popped` did not move). Boxing `Terminate` alone gave
  back all of it (`f518988`), and a release-only test pins the three sizes.
- **`bench_value.ns` and `bench_block.ns`: an arena reallocated at every
  collection.** `bench_value.ns` moved at `85d629d`, which changes no code
  on the path at all -- `vm::run` and `decide_slot_accessor` are the same
  instructions in both builds -- but declares the exception classes, so the
  live set a collection leaves grew from 57 objects to 72. A profile put
  the difference in page faults: kernel-mode samples under
  `Heap::allocate_object` went from 14 to 349, with `Vec<Object>`'s
  `grow_one` and the collector's `memset` beside them. Compaction shrank an
  arena to twice its live set once three quarters of it was idle, while
  the next collection is requested only when the arena reaches
  `max(floor, 2 x live)`; under the 4,096-slot floor the two part, and every
  collection cut the object arena to 144 slots, which the next cycle grew
  back to the floor through five reallocations, each a copy and fresh
  pages. The base did the same with 114 slots and six reallocations, which
  happened to fault less. Compaction now measures against the limit
  (`collection_limit`, which `reset_collection_limits` shares): a buffer
  within twice the limit is kept, one past it is cut back to the limit
  (`c5b1f2b`) -- the old rule wherever the live set is above half the
  floor. That gave back all of `bench_value.ns` and more of
  `bench_block.ns` than the branch had cost it.

What was ruled out on the way: the lead from the first profile, `Heap::reify`
out of line from `vm::run`, is real but small -- splitting it into an inline
check of `reified` and an out-of-line creation measured about 1% on
`bench_value.ns`, within the rounds' spread, and was not kept; the process
environment's size, which moves the native stack, left the `bench_value.ns`
gap where it was at five paddings from 0 to 3,000 bytes; and forty unused
methods added to `Kernel.ns` on the base, a data change with no new class,
moved nothing.

**A third defect, found while profiling what was left.** After those two,
`bench_loop.ns` measured 1.9% slower than the base and `bench_megamorphic.ns`
0.9%. A profile of `bench_loop.ns` put a third of its samples in
`Mutator::decide_slot_accessor` -- every slot read and write -- most of them
on reloading the field from a stack slot it had just been spilled to and
storing it into the step; the function is the same instructions in both
trees, so its cost moves with its surroundings rather than with anything the
branch changed in it. It built its answer through `Option` combinators ending
in an `ok_or_else` whose closure formats the error, and compiled in place that
closure gave the accessor a frame. Written as early returns with the error in
a `#[cold]`, `#[inline(never)]` function (`1d7587f`), the accessor is a leaf
that moves the field straight into the step.

**Measured 2026-09-26, `1d7587f` against `3cbfd31`**, by the same protocol:
the same three base binaries, the head built plain and with
`-C metadata=layoutprobe` and `=secondprobe`, pinned flags, four sittings of
five rounds, rotating. Medians in ms per layout, the sittings pooled:

| Source | `base` | `base-p` | `base-q` | `head` | `head-p` | `head-q` |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 7.28 | 7.28 | 7.26 | 5.51 | 5.50 | 5.50 |
| `bench_send.ns` | 8.71 | 8.71 | 8.68 | 7.09 | 7.07 | 7.09 |
| `bench_block.ns` | 16.36 | 19.08 | 19.19 | 14.82 | 14.78 | 14.79 |
| `bench_megamorphic.ns` | 11.58 | 11.60 | 11.65 | 10.22 | 10.20 | 10.24 |
| `bench_value.ns` | 43.71 | 43.81 | 43.96 | 38.30 | 37.83 | 38.15 |
| `bench_deep.ns` | 16.53 | 16.12 | 16.17 | 15.88 | 15.87 | 15.83 |

| Source | `3cbfd31` | `1d7587f` | Change, pooled | Sitting 1 / 2 / 3 / 4 | Rounds, base / head |
|---|---|---|---|---|---|
| `bench_loop.ns` | 7.27 | 5.50 | −24.3% | −25.0% / −24.0% / −25.7% / −25.1% | 7.16–8.40 / 5.40–6.65 |
| `bench_send.ns` | 8.70 | 7.08 | −18.6% | −18.3% / −19.1% / −18.8% / −19.0% | 8.55–9.02 / 6.91–7.40 |
| `bench_block.ns` | 18.21 | 14.80 | −18.7% | −18.5% / −18.8% / −17.9% / −19.0% | 16.10–19.48 / 14.45–15.03 |
| `bench_megamorphic.ns` | 11.61 | 10.22 | −12.0% | −11.6% / −11.8% / −12.0% / −11.3% | 11.34–12.00 / 9.93–10.46 |
| `bench_value.ns` | 43.82 | 38.09 | −13.1% | −12.9% / −13.5% / −12.5% / −12.7% | 42.60–44.88 / 37.27–40.09 |
| `bench_deep.ns` | 16.27 | 15.86 | −2.5% | −3.0% / −2.6% / −2.7% / −2.4% | 15.82–17.15 / 15.59–16.79 |

Every source is faster than the base in every sitting, and the head's layouts
agree to within 1.3% on each; `bench_block.ns` no longer draws a slow layout
in any of the three builds. Attributed by builds of each commit on its own,
two layouts each, five rounds: the arena accounts for `bench_block.ns` (20.2 →
16.7 ms) and `bench_value.ns` (46.0 → 43.4), `Signal` for `bench_deep.ns`
(17.4 → 16.4), and the slot accessor for most of the rest, `bench_loop.ns` 7.3
→ 5.5, `bench_send.ns` 8.8 → 7.1–7.4 and `bench_megamorphic.ns` 11.7 → 10.2.
Only the `Signal` boxing repairs a cost the branch introduced; the arena
shrink and the slot accessor repair defects `master` has too, so being faster
than the base is not a property of the exceptions branch, and its net cost
against a `master` carrying the same two fixes was not measured -- the
per-commit attribution suggests about zero.

**What the protocol costs when nothing is signalled**, measured at
`3d5f69c`, before the three fixes above. `bench_ensure.ns`
runs the same three block bodies as `bench_value.ns`, one as an `ensure:`
receiver, one as its argument, one as an `on:do:` receiver: 60.47 ms
against 46.01, about **145 ns an iteration for one `ensure:` and one
`on:do:`** that signal nothing -- each is a Newspeak method that marks its
activation and sends `value`, and `ensure:` then sends `value` to its
argument itself.

**What a signal costs.** `bench_signal.ns` takes 81.38 ms (the mean of the
three layouts; 78.44 to 82.89 between them), against 17.29 for
`bench_deep.ns`, the same recursion answering instead of signalling: about
**6.4 µs for each `Error signal:` caught twenty activations up**, which is
`signal`'s search in Newspeak over the activation leaves, the handler's
activation, and the unwind of the twenty activations and the handler's
own to the `on:do:`.

**`vm::run`'s step is still one function, and `push` still inlines.** At
`3d5f69c`, a release build with debug information of each tree, pinned flags,
`dumpbin -disasm`: one function label `ns_runtime::vm::run` in each and no
`vm::run::closure`. The base has **6,741 instructions** and the head
**6,603**. `Mutator::push` has no label in either build: it is inlined into
`push_step` and `push_initializer`, which call `Heap::push_activation` from
the same five sites as the base (four and one); `push_past_the_limit` is a
function of its own (`#[cold]`, `#[inline(never)]`) called from those same
five sites, so the path below the limit is the one compare it was. Other
sizes that moved: `push_step` from 1,361 to 1,214 instructions, `vm::finish`
from 606 to 716, `vm::unwind` from 177 to 1,942 (the unwinder's outcome
handling, with `interpreter::unwind::unwind`, 439, out of line), and
`Heap::reify` (258) now has a label of its own. At `d7a6680`, after the
three fixes above, the same check finds one `ns_runtime::vm::run` label and no
`vm::run::closure`, `vm::run` at 6,486 instructions, `vm::finish` at 664, and
`decide_slot_accessor` at 80 with its error, `no_field_for_slot_accessor`
(84), a function of its own.

### What the platform module cost

The platform module (`docs/design/core-library.md`, "The platform and
collections as core-library modules", plan 1) added §6.3.3's lazy
slots — as first measured below, a lazy getter was a compiled method with
`MethodFastPath::LazySlot`, its arm sitting beside the other
`MethodFastPath` arms in `interpreter/resolve.rs`'s `act_with_base`; since
"Recovering it" below, it has no fast path and answers a filled slot with
`Instruction::ReturnSlotIfFilled` — platform paths in the image (since
replaced by shipped names, [image-format.md](image-format.md), "Shipped
names"),
and a shipped `Platform.ns` (since 2026-09-27 `PlatformFactory.ns`, in the
factory pattern of [core-library.md](core-library.md), "The platform")
compiled and resolved once at bootstrap,
alongside `Kernel.ns`, with `usingVmMirror:kernel:`. `Platform`, `Transcript`
and `KernelNamespace`, previously builtin classes declared inside
`Kernel.ns` (tags 13, 18, 19 and 29, now retired), moved into that module;
`Collections` moved too, still answering the kernel's `OrderedCollection`
for `List`. Combined, the two files grow from `Kernel.ns`'s 587 lines at
`7c6bdbf` to 553 (`Kernel.ns`) plus 44 (`Platform.ns`), 597 in all — the
module split does not shrink what bootstrap compiles, it adds a second
`resolve_shipped_module` call and ten lines net.

**Measured 2026-09-27, `054e980` against `7c6bdbf`** (the plan commit Task 1
started from), by the protocol of "What removing the re-entries moved"
above: `7c6bdbf` built in a throwaway worktree
(`git worktree add`, removed afterwards), the head in the current tree, two
builds of each with `RUSTFLAGS="-C target-feature=+crt-static -C
llvm-args=-align-all-functions=6"`, the four binaries differing byte for
byte; none imports `VCRUNTIME140.dll` (`dumpbin -dependents`); each checked
to print the expected answer first.

**Start-up.** `ns-next run` of a module whose `main:args:` answers `0` at
once (no `platform` member other than the implicit factory parameter),
wall-clock from the parent process (a .NET `Process` under `Measure`-style
`Stopwatch`, not piped through a shell), 200 runs of each of the four
binaries per sitting, five discarded warm-up runs each, the binaries'
turn order rotated between the two sittings; nothing else running. This
machine's per-process overhead is higher than the 3.56/4.08 ms recorded
against `e2bb31e` ("What the core library's move cost" above) — `ns-next
--help`, which never compiles a kernel, medians 2.84 ms here — so the
figures below are not comparable to that entry's across machines, only to
each other.

| Binary | Sitting 1 median | Sitting 1 mean | Sitting 2 median | Sitting 2 mean |
|---|---|---|---|---|
| Head, build a | 5.288 | 5.353 | 5.282 | 5.333 |
| Head, build b | 5.305 | 5.370 | 5.274 | 5.329 |
| `7c6bdbf`, build a | 5.288 | 5.327 | 5.285 | 5.334 |
| `7c6bdbf`, build b | 5.291 | 5.329 | 5.315 | 5.348 |

Pooled across both builds and both sittings (n = 800 runs a side): head mean
**5.346 ms**, `7c6bdbf` mean **5.335 ms** — +0.011 ms, +0.2%, inside the
p10–p90 spread of every single group (about 0.45–0.5 ms wide). **No
measurable start-up cost**: the ten extra lines the module split adds are
far below what this protocol resolves, consistent with the line count
above — the platform's classes moved out of `Kernel.ns` rather than being
added on top of it.

**The four `benchmarks/micro/` fixtures.** `ns-next bench <source>
--iterations 30`, five rounds a sitting, the four binaries rotating by one
position from round to round, two whole sittings, nothing else running.
Means of the five rounds:

Sitting 1:

| Source | Head, build a | Head, build b | `7c6bdbf`, build a | `7c6bdbf`, build b |
|---|---|---|---|---|
| `bench_loop.ns` | 5.1305 | 5.1530 | 5.1257 | 5.1060 |
| `bench_send.ns` | 6.8934 | 6.8777 | 6.5611 | 6.5898 |
| `bench_block.ns` | 14.5002 | 14.4043 | 14.0099 | 14.1108 |
| `bench_megamorphic.ns` | 9.4685 | 9.5302 | 9.3776 | 9.3968 |

Sitting 2:

| Source | Head, build a | Head, build b | `7c6bdbf`, build a | `7c6bdbf`, build b |
|---|---|---|---|---|
| `bench_loop.ns` | 5.1344 | 5.1392 | 5.1487 | 5.1424 |
| `bench_send.ns` | 6.9355 | 6.9221 | 6.6847 | 6.5828 |
| `bench_block.ns` | 14.4286 | 14.5544 | 14.1677 | 14.1145 |
| `bench_megamorphic.ns` | 9.4002 | 9.4250 | 9.4048 | 9.3723 |

Both sittings pooled (n = 20 rounds a side), means in ms, and whether every
round of the head lies outside every round of `7c6bdbf`'s, sitting by
sitting:

| Source | Head | `7c6bdbf` | Change | Sitting 1 | Sitting 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 5.1393 | 5.1307 | +0.17% | overlapping | overlapping |
| `bench_send.ns` | 6.9072 | 6.6046 | **+4.6%** | entirely above | overlapping (one `7c6bdbf` round at 7.03) |
| `bench_block.ns` | 14.4719 | 14.1007 | **+2.6%** | entirely above | entirely above |
| `bench_megamorphic.ns` | 9.4560 | 9.3879 | +0.7% | overlapping | overlapping |

**`bench_loop.ns` is level; `bench_block.ns` is a reproducible regression,
entirely above in both sittings, and `bench_send.ns` agrees in the sitting
its one outlier round does not obscure.** The three that moved all send at
least one method or activate a block inside their timed loop —
`bench_send.ns` a lexical `self tick:`, `bench_block.ns` a block literal's
`value:`, `bench_megamorphic.ns` a polymorphic `value` through an `Array`
— where `bench_loop.ns`'s `+` and `<` never leave `IntegerBinary`'s inlined
fast path. `bench_megamorphic.ns` is the one exception to that split,
overlapping in both sittings despite sending a method every iteration; its
send is megamorphic by design (`docs/design/dispatch.md`'s namesake cache
miss), so it already pays a table walk this step did not touch, and
whatever the other two lost is a smaller fraction of its larger per-send
cost.

**The cause is `vm::run`'s growth, not the platform module's own work.**
A release build with debug information of each tree (the timed build's
`RUSTFLAGS` plus `CARGO_PROFILE_RELEASE_DEBUG=true`, a separate
`--target-dir`), then `dumpbin -disasm`: one `ns_runtime::vm::run` label in
each, no `vm::run::closure` — the step did not split the function.
`7c6bdbf` measures **6,561 instructions**, the head **6,750** — 189 more,
2.9%. `act_with_base` carries no label of its own in either build: like
every other primitive method's fast path, it is inlined into `vm::run`
(`docs/design/dispatch.md`, "Primitive methods"), so the one arm
`MethodFastPath::LazySlot` added to its match is inlined too, on every
method send whether or not it ever reaches a lazy slot. 2.9% more code
in the function every method send and every block activation runs through
is consistent with `bench_send.ns` and `bench_block.ns` losing 2.6–4.6%,
`bench_megamorphic.ns` losing nothing measurable on top of its own bigger
cost, and `bench_loop.ns`, which never enters that growth at all, losing
nothing. No profile beyond the instruction count was taken; distinguishing
an instruction-cache effect from a changed branch layout inside the match
would need one, and is not a case this step's measurement makes on its own.

#### Recovering it

The fix was tried in three steps, in order, each stopping only if it
recovered the loss, all measured by the protocol above against `7c6bdbf`:
the baseline built in a throwaway worktree outside the repository (`git
worktree add`, removed afterwards), the candidates from this tree, every
build with the pinned `RUSTFLAGS` in its own `--target-dir`, every binary
checked to print its fixture's recorded answer and, for the final
comparison, not to import `VCRUNTIME140.dll`. `ns-next bench <source>
--iterations 30`, five rounds a sitting, the binaries rotating by one
position from round to round; each figure is the mean of the rounds'
medians, in ms. "Head" is the tree before the fix (`2fc5b07`, whose hot path
is `8212048`'s).

1. **(a) The arm's body out of line.** `Mutator::lazy_slot_field`,
   `#[inline(never)]` (not `#[cold]`: every filled read takes it), left a
   call and one test at each of the four send sites `act_with_base` is
   inlined into. Pooled over two sittings (n = 20 base, 10-20 candidate):
   `bench_send.ns` +4.5%, `bench_block.ns` +1.9%, the same as the head's
   +4.5% and +1.9% in the same sittings; `bench_send.ns` entirely above
   `7c6bdbf` in both.
2. **(b) The slot base off the activation as well.** `push_step` matched on
   `method.fast_path` on every method push to give a lazy getter its
   defining application's `own_slot_base`; the activation's `slot_base`
   field went instead, `Heap::activation_slot_base` deriving the base from
   the activation -- then from its `current_class`, today from the
   application it was entered at, which is exactly the level whose slots
   `InitializeSlot` writes in both bodies that contain it. Pooled (n = 20):
   `bench_send.ns` +3.6%, `bench_block.ns` +1.2%, `bench_send.ns` still
   entirely above in both sittings. `vm::run` measured 6,758 instructions,
   more than the head's 6,750, with the four inlined reads of the class's
   slot base and `instance_field` calls replaced by four calls to the
   helper.
3. **(c) No fast path.** Built as a diagnostic first, `act_with_base`
   with the `LazySlot` arm removed and the variant kept brought `vm::run`
   to 6,616: the arm itself, even as a call, was ~140 instructions over
   its four inlined copies. So the variant went: a lazy slot's getter
   opens with `Instruction::ReturnSlotIfFilled` (wire tag 34), which returns
   the field from the getter's own activation when it holds anything but
   `nil` (`interpreter::locals::filled_own_slot`, addressed through the same
   derived slot base), and the decoder no longer restores anything for it.
   A read of a filled lazy slot now costs an activation; an ordinary send
   costs nothing for the feature.

**(c), measured 2026-09-27 against `7c6bdbf`**, three builds of each tree,
two sittings. Means of the five rounds:

Sitting 1:

| Source | `7c6bdbf` a | `7c6bdbf` b | `7c6bdbf` c | (c) a | (c) b | (c) c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.1604 | 5.0940 | 5.1340 | 5.1208 | 5.0937 | 5.1083 |
| `bench_send.ns` | 6.5869 | 6.5671 | 6.6040 | 6.5123 | 6.6111 | 6.5309 |
| `bench_block.ns` | 14.0906 | 14.1355 | 14.0696 | 14.1063 | 14.1029 | 14.2168 |
| `bench_megamorphic.ns` | 9.4264 | 9.4336 | 9.3884 | 9.4219 | 9.4060 | 9.4244 |

Sitting 2:

| Source | `7c6bdbf` a | `7c6bdbf` b | `7c6bdbf` c | (c) a | (c) b | (c) c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.1844 | 5.1355 | 5.0598 | 5.1111 | 5.1841 | 5.0750 |
| `bench_send.ns` | 6.6278 | 6.5537 | 6.6398 | 6.5408 | 6.5348 | 6.5835 |
| `bench_block.ns` | 14.0719 | 14.1692 | 14.1405 | 14.0850 | 14.0767 | 14.4189 |
| `bench_megamorphic.ns` | 9.3944 | 9.3866 | 9.3831 | 9.4351 | 9.4417 | 9.3910 |

Both sittings pooled (n = 30 rounds a side):

| Source | (c) | `7c6bdbf` | Change | Sitting 1 | Sitting 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 5.1155 | 5.1280 | -0.24% | overlapping | overlapping |
| `bench_send.ns` | 6.5522 | 6.5966 | -0.67% | overlapping | overlapping |
| `bench_block.ns` | 14.1678 | 14.1129 | +0.39% | overlapping | overlapping |
| `bench_megamorphic.ns` | 9.4200 | 9.4021 | +0.19% | overlapping | overlapping |

A third sitting that ran all four trees side by side (two builds each,
n = 10) agrees: `7c6bdbf` 6.6100 / 14.1607, the head +4.2% / +2.2%, (b)
+3.5% / +1.4%, (c) -0.8% / +0.5% on `bench_send.ns` / `bench_block.ns`.
**The regression is gone**: every fixture is within its own spread of
`7c6bdbf`, and no comparison lies entirely on one side in either sitting.
`bench_block.ns`'s largest single round, 15.14 for one (c) build, is the
kind of build-to-build outlier the protocol's pooling exists for.

`dumpbin /disasm` of a release build with debug information: one
`ns_runtime::vm::run` label, no `vm::run::closure`, and no
`act_with_base` label of its own; **6,691 instructions**, against 6,561 at
`7c6bdbf` and 6,750 at the head. The 130 over `7c6bdbf` are the
`ReturnSlotIfFilled` arm, which no ordinary send runs, and the ~55 the
diagnostic build above still showed without the `LazySlot` arm -- an
instruction count, then, is not what these fixtures measure: (b) was
larger than the head and no slower, (c) larger than `7c6bdbf` and level. What
they measure is the send sites' shape, which (c) restored.

### What the collections module cost

The collections module (`docs/design/core-library.md`, "The collections
module (plan 2)") is a third shipped module, `CollectionsFactory.ns`, whose
`List` is Newspeak over `Array new:` where the builtin `OrderedCollection`
was Rust over a growable payload. It is built on the first `platform
collections`: `PlatformFactory`'s lazy `collectionsModule` asks the mirror
for the module's root class (`shippedModuleClassNamed:`), sends it the
module's factory and records the instance (`recordShippedModule:`), and
each class of it is built and named as its accessor builds it
(`interpreter/core_library.rs`, `interpreter/nested.rs`'s `after_clause`).
An image that names the module or one of its classes has the loader build
them before it decodes the graph (`image-format.md`, "Shipped names").

Plan 2, which the first two measurements below are of, built the module on
the first `platform collections` instead, through the mirror's
`moduleNamed:`, and nothing of it at boot. It made `CollectionsFactory.ns`
99 lines, grew `Kernel.ns` from 553 lines to 568 (`Array new:`, the handler
search collecting into an `Array`) and `PlatformFactory.ns` from 83 to 87:
754 lines of shipped source against 636. Nothing on a send's path changed.
The last measurement is of the boot that builds the whole shipped world.

**Start-up, measured 2026-09-28, `816367e` against `6d9974b`** (the commit
before the plan's first), by the protocol of "What removing the re-entries
moved" above, with a third tree beside them: `ee4bdc8`, the plan's last
commit before `interpreter/core_library.rs` resolved each shipped module on
its first request, when a boot resolved the whole table. Both older trees
were built in throwaway worktrees under the git-ignored `.temp/` (`git
worktree add`, removed afterwards), the head in the current tree, three
builds of each with the pinned `RUSTFLAGS`, the second and third adding `-C
metadata=buildb` and `-C metadata=buildc`, each in its own `--target-dir`;
the nine binaries differ byte for byte, and each ran both start-up sources
to exit status 0 first.

`ns-next run` of two sources, wall-clock from the parent process (a .NET
`Process` under a `Stopwatch`, not through a shell): `startup_zero.ns`,
whose `main:args:` answers `0` at once, the source "What the platform module
cost" measured; and `startup_list.ns`, which sends `platform collections
List new` once and then answers `0` -- at `6d9974b` that answers the builtin
`OrderedCollection`, so the same source runs on every tree and measures what
building the collections module on first use costs. Three interleaved
sittings: one run of each of the eighteen binary-and-source pairs per round,
the order rotated by one position every round, so a drift of the machine
falls on every pair alike; 200 rounds a sitting after five discarded
warm-up rounds, 600 runs per binary and source in all. The per-process
overhead of this machine is high -- the antivirus service scans every
process start -- so the figures compare with each other only.

Means per build over the three sittings, in ms:

| Source | Head a | Head b | Head c | `ee4bdc8` a | `ee4bdc8` b | `ee4bdc8` c | `6d9974b` a | `6d9974b` b | `6d9974b` c |
|---|---|---|---|---|---|---|---|---|---|
| `startup_zero.ns` | 13.277 | 13.273 | 13.313 | 13.404 | 13.375 | 13.340 | 13.279 | 13.253 | 13.252 |
| `startup_list.ns` | 13.500 | 13.480 | 13.492 | 13.518 | 13.514 | 13.543 | 13.221 | 13.205 | 13.210 |

Pooled over the three builds (n = 1,800 runs a tree), in ms, with the
median and the p10–p90 range of the pooled runs:

| Source | Tree | Mean | Median | p10–p90 | Against `6d9974b` |
|---|---|---|---|---|---|
| `startup_zero.ns` | head | 13.288 | 13.191 | 12.763–13.907 | +0.03 ms, +0.2% |
| | `ee4bdc8` | 13.373 | 13.283 | 12.839–14.003 | +0.11 ms, +0.8% |
| | `6d9974b` | 13.262 | 13.164 | 12.732–13.891 | |
| `startup_list.ns` | head | 13.491 | 13.401 | 12.992–14.123 | +0.28 ms, +2.1% |
| | `ee4bdc8` | 13.525 | 13.428 | 13.002–14.149 | +0.31 ms, +2.4% |
| | `6d9974b` | 13.212 | 13.122 | 12.705–13.858 | |

**Under plan 2, a start that never sent `platform collections` cost what it
did before the module existed; the first `platform collections` cost about
0.28 ms.** The head's `startup_zero.ns` is level with `6d9974b`'s: its builds' means lie
among `6d9974b`'s (13.27–13.31 against 13.25–13.28), a difference of 0.03 ms
where one build's p10–p90 range is over 1 ms wide. `ee4bdc8`'s lay above
both in every sitting, every one of its builds above every head build, by
0.085 ms pooled: that is what resolving `CollectionsFactory.ns`'s 99 lines
at every boot cost, and resolving each shipped module on its first request
(`interpreter/core_library.rs`'s `RESOLVED_SHIPPED_MODULES`, one cell per
table entry) is what removed it then; a boot from source today asks for
`PlatformFactory`'s entry alone, and for another only on its module's first
demand.
The 15 lines `Kernel.ns` grew by do not show. `startup_list.ns` is where the work went: the head and `ee4bdc8` are
level there (-0.03 ms), since both resolve the module by the time the
program ends, and both lie 0.28–0.31 ms above `6d9974b`, whose `List` was a
builtin class that was there anyway. That first-use cost was what plan 2
expected to pay: parsing and resolving `CollectionsFactory.ns`, compiling it,
building its root class, and instantiating it and its facet.

**The four `benchmarks/micro/` fixtures, measured 2026-09-28, `f9d953a`
against `6d9974b`**, three builds of each by the same protocol; what the
branch changed after `f9d953a` runs only at boot and on the first request
for a shipped module, never inside a timed loop. `ns-next bench <source>
--iterations 30`, five rounds a sitting, the six binaries rotating by one
position from round to round, two sittings, nothing else of this work
running; each printed every fixture's recorded answer. Means of the five
rounds' medians:

Sitting 1:

| Source | `6d9974b` a | `6d9974b` b | `6d9974b` c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.0054 | 4.9705 | 4.9458 | 4.9758 | 5.0059 | 4.9711 |
| `bench_send.ns` | 6.4439 | 6.4290 | 6.3627 | 6.4214 | 6.4699 | 6.3869 |
| `bench_block.ns` | 13.9434 | 13.9328 | 13.8841 | 13.7780 | 13.8448 | 13.9391 |
| `bench_megamorphic.ns` | 9.0513 | 9.0736 | 9.0105 | 9.0756 | 9.1582 | 9.1128 |

Sitting 2:

| Source | `6d9974b` a | `6d9974b` b | `6d9974b` c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.0197 | 5.0049 | 5.0035 | 5.0135 | 5.0097 | 5.0304 |
| `bench_send.ns` | 6.4133 | 6.4194 | 6.3839 | 6.4107 | 6.4782 | 6.4264 |
| `bench_block.ns` | 13.9237 | 13.9686 | 13.8974 | 14.0602 | 13.9364 | 13.8294 |
| `bench_megamorphic.ns` | 9.1510 | 9.0505 | 9.0947 | 9.1189 | 9.1161 | 9.1089 |

Both sittings pooled (n = 30 rounds a side), means in ms, and whether every
round of the head lies outside every round of `6d9974b`'s, sitting by
sitting:

| Source | Head | `6d9974b` | Change | Sitting 1 | Sitting 2 |
|---|---|---|---|---|---|
| `bench_loop.ns` | 5.0011 | 4.9917 | +0.19% | overlapping | overlapping |
| `bench_send.ns` | 6.4323 | 6.4087 | +0.37% | overlapping | overlapping |
| `bench_block.ns` | 13.8980 | 13.9250 | -0.19% | overlapping | overlapping |
| `bench_megamorphic.ns` | 9.1151 | 9.0719 | +0.48% | overlapping | overlapping |

**All four fixtures are level with `6d9974b`**, as they should be: none
sends a collection message in its timed loop, and nothing on a send's path
changed. `bench_block.ns`'s largest round, 14.80 for one head build, is the
build-to-build outlier kind the pooling is for.

**The whole shipped world at boot, measured 2026-10-12** on this
workstation (Windows 11): `ns-next run` of a source whose `main:args:`
shows `hello` on the transcript, release builds without the pinned
`RUSTFLAGS`, each sample 50 runs timed by PowerShell's `Measure-Command`
with the output discarded, its median taken; the three binaries were
sampled in turn, three rounds. Three trees: `e2e1695`, before the boot
built any shipped module but `PlatformFactory`; `1ff9a33`, whose boot
compiles and instantiates `PlatformFactory` and `CollectionsFactory` and
builds only the classes a run asks for; and `2d35951`, whose boot then
builds every class the three roots declare and records each shipped
object under its name (`complete_shipped_world`), as every tree did until
the shipped modules were built on first demand ("What the precompiled
shipped code moved" below). Medians in ms:

| Round | `e2e1695` | `1ff9a33` | `2d35951` |
|---|---|---|---|
| 1 | 12.54 | 16.24 | 16.10 |
| 2 | 12.28 | 15.92 | 16.07 |
| 3 | 12.06 | 15.86 | 15.72 |

A first sample of `1ff9a33` alone, before `2d35951` existed, gave 16.11 and
16.06 ms, and two more of `2d35951` against `1ff9a33` 15.94 against 16.17
and 16.06 against 15.87 ms. **Building every shipped class and naming the
world does not show**: `2d35951` and `1ff9a33` lie within each other's
round-to-round spread of some 0.3 ms. **Building `CollectionsFactory` at
every boot costs about 3.7 ms**, `1ff9a33` against `e2e1695`: parsing,
resolving and compiling its 1,710 lines of source, against the 0.28 ms plan
2 measured for its first use when it had 99: a hello program's median
start went from about 12.3 ms to about 16.0 ms. The per-process overhead of
this machine is high (above), so the figures compare with each other only.

**The kernel's fifth sub-project added about 4.6 ms more** (measured
2026-10-13, at `1906a81d` against `b9149044`, release builds, the same
hello program): `ByteArray`, `StringBuilder`, the typed arrays and
`DataView` grew `Kernel.ns` from 4,089 lines to 6,586, all of it compiled
at every boot, and the median start went from about 15.4 ms to about
20.1 ms ([core-library.md](core-library.md), "`ByteArray`,
`StringBuilder` and the views"). `cc2aaebe`, after it, added 53 lines
of `StringBuilder` state checks, unmeasured.

The embedded shipped code took the cost back ("What the precompiled
shipped code moved" below).

### What the precompiled shipped code moved

The shipped modules' compiled code is generated ahead of time by `cargo xtask
shipped-code` and embedded in the binary; an optimized boot decodes it instead
of parsing, resolving and compiling the roughly 9,000 lines of the six modules
(`shipped_code/`, `interpreter/core_library.rs`). The world is built by
running the binary's own code on the decoded mixins. It was measured in two
stages: the first decoded instead of compiling and still built every module at
boot, so its instantiations and `complete_shipped_world` were what they had
been; the second builds only `Kernel` and `PlatformFactory` at boot and every
other module on its first demand (`core-library.md`, "The module table and the
boot").

**The first stage, measured 2026-10-14, `eb2716b8` against `f60f6782`**, the
first stage's head against the head of `master`, at which the start-up was
measured the same day before the branch began, on the same machine, the
measuring workstation (Windows 11). Both are release builds with the pinned
`RUSTFLAGS` of the protocol above, each in its own target directory,
`f60f6782` in a throwaway worktree under `.temp/`; the branch's artifact was
generated first, and the temporary marks (below) show its decode phases where
`master`'s show parse, resolve and compile, which confirms the artifact is
used. One build per side: the effect is a boot of some 13.6 ms against some
1.6 ms, far above the build-to-build spread of percent-level run-loop timings
the protocol guards against. The program is a hello (`main:args:` showing
`'hello'` on the transcript). The marks are `Instant` readings at the start of
`bootstrap_with` and around each module's parse, resolve, compile (decode on
the branch), class, instantiation, `complete_shipped_world` and the platform's
`build`, printed to `stderr`; they were in both trees, are not committed, and
cost a few microseconds each. 30 runs, the median per label; the whole process
is 40 runs of the same binaries with the marks switched off, timed from
outside.

| Module | Parse | Resolve | Compile | Class and instance | Sum, `f60f6782` | Decode | Class and instance | Sum, `eb2716b8` |
|---|---|---|---|---|---|---|---|---|
| `Kernel` | 3.44 | 0.67 | 4.51 | 0.17 | 8.9 ms | 0.90 | 0.08 | 0.98 ms |
| `CollectionsFactory` | 0.93 | 0.20 | 1.57 | 0.02 | 2.7 ms | 0.27 | 0.01 | 0.29 ms |
| `TypedArraysFactory` | 0.58 | 0.10 | 0.63 | 0.02 | 1.3 ms | 0.11 | 0.02 | 0.14 ms |
| `PlatformFactory`, `TranscriptFactory`, `TimeFactory` | | | | | 0.52 ms | | | 0.14 ms |
| `complete_shipped_world`, the platform's `build` | | | | | 0.09 ms | | | 0.08 ms |
| **The boot** | | | | | **13.6 ms** | | | **1.6 ms** |

The module sums include a few microseconds of set-up between marks (0.1 ms for
the kernel's). The baseline repeats the measurement of `f60f6782` made earlier
the same day, on the same machine, with the same marks: parsing 5.26 ms,
resolving 0.98 ms, compiling 6.98 ms, building classes, instantiating,
`complete_shipped_world` and the platform 0.33 ms, the boot 13.7 ms; the whole
process 20.0 ms, `ns-next --version` 2.5 ms.

| Process | `f60f6782` | `eb2716b8` |
|---|---|---|
| The boot, summed medians of the marks | 13.6 ms | 1.6 ms |
| `ns-next run` of the hello, median of 40 | 20.8 ms | 5.2 ms |
| `ns-next --version`, median of 40 | 2.6 ms | 2.5 ms |

**The boot is 12 ms shorter and the whole process 15.5 ms shorter.** The boot
is an eighth of what it was; the process, with the 2.5 ms a process that boots
nothing costs, a quarter. The whole process fell by more than the boot (15.5
ms against 12.0 ms) and this measurement does not decompose the remaining 3.5
ms: it falls outside what the marks cover, and its cause is not established.
The measurement did not decompose that difference, took one build per side,
one program and one machine.

**Decoding costs 1.3 ms in all**: the kernel's 0.90 ms, `CollectionsFactory`'s
0.27 ms, `TypedArraysFactory`'s 0.11 ms and 10 to 24 microseconds for each of
the other three. That is about a ninth of the 12 ms the source path spent on
the same modules, and the kernel alone is more than half of it. What remains
of the boot beyond decoding is the instantiations and the class building, 0.2
ms.

**The second stage builds a module on its first demand, and a boot builds only
`Kernel` and `PlatformFactory`.** By the first stage's numbers it could save
only what the boot spent on modules a program may never touch:
`CollectionsFactory` and `TypedArraysFactory`, decoded and instantiated, 0.27
+ 0.01 and 0.11 + 0.02 ms, 0.40 ms together, under the 0.5 ms set as the bar
for the machinery. It is built anyway (decided with the user on 2026-10-15)
because that cost is not fixed: the artifact grows with every module and every
member the core library gains -- 211,731 bytes today -- and an eager boot pays
for all of it in every process, whatever the program uses, where a boot on
demand pays for the kernel, the platform and what the program reaches. The
kernel's decode, 0.9 ms, is no candidate: it is always needed.

**The second stage, measured 2026-10-15, `e178ac5c` against `740adb25`**, the
head that builds on demand against the head before its first commit, on the
measuring workstation (Windows 11), by the first stage's method: release
builds with the pinned `RUSTFLAGS`, one per side, `740adb25` in a throwaway
worktree under `.temp/`, each after `cargo xtask shipped-code` and its
`--check` saying the artifact is current. Two programs: the hello, and one
whose `main:args:` sends `platform collections List new` once before showing
`'hello'`, which pays the first demand of `CollectionsFactory` and the build
of its `List`. The marks are a set of their own, the same on both sides but
where noted, and not the first stage's per-step ones: at the start of `main`,
the start of `bootstrap_with`, the end of the kernel phase, around each
module's decode and around `complete_shipped_world`, the end of the
shipped-module phase and the process's exit, and on this side also at a lazy
module's demand (`shipped_module_class`'s entry) and its record
(`record_pending`'s end); not committed. 30 marked runs of each program and
side, interleaved, the median per span; 40 whole-process runs of the same
binaries with the marks switched off, interleaved, timed from outside
(Python's `subprocess.run` under `time.perf_counter`).

The eager boot measures 1.77 ms here and 1.6 ms in the first stage although
the code it boots is the same -- `eb2716b8` to `740adb25` changed only
comments under `crates/`. The two figures are taken differently: the first
stage's is a sum of per-step medians, which leaves out the set-up between its
marks, and this one is the median of one span from `bootstrap_with`'s start to
the boot's end, from another build on another day. Each compares only within
its own measurement.

| Span, median, ms | Hello, `740adb25` | Hello, `e178ac5c` | `List new`, `740adb25` | `List new`, `e178ac5c` |
|---|---|---|---|---|
| The kernel phase (its decode) | 1.08 (0.88) | 1.08 (0.88) | 1.05 (0.86) | 1.03 (0.83) |
| The shipped-module phase | 0.68 | 0.15 | 0.68 | 0.15 |
| of which `complete_shipped_world` | 0.08 | 0.03 | 0.07 | 0.03 |
| **The boot** | **1.77** | **1.22** | **1.73** | **1.18** |
| The boot's end to the exit | 0.49 | 0.46 | 0.49 | 0.84 |
| of which `TranscriptFactory`'s demand | | 0.04 | | 0.03 |
| of which `CollectionsFactory`'s demand (its decode) | | | | 0.31 (0.29) |
| `main` to the exit | 2.60 | 2.01 | 2.56 | 2.37 |
| `ns-next run`, median of 40 | 5.24 | 4.74 | 5.31 | 5.04 |
| `ns-next --version`, median of 40 | 2.60 | 2.61 | | |

**The boot is 0.55 ms shorter, and a hello's whole process 0.50 ms.** The boot
no longer decodes, instantiates or completes the four lazy modules; what it
keeps is the kernel phase's 1.08 ms and the shipped-module phase's 0.15 ms --
`PlatformFactory`'s decode and instantiation, the kernel's and the platform's
classes completed (0.03 ms) and the platform's `build`. The saving is more
than the 0.40 ms the first stage's numbers allowed, since `TranscriptFactory`
and `TimeFactory` and the classes of all four modules are no longer built
either; a hello builds `TranscriptFactory` on its first `platform transcript`,
for 0.04 ms.

**A first demand of `CollectionsFactory` costs 0.31 ms, almost all of it the
module's decode (0.29 ms)**, and the run after the boot of the `List new`
program takes 0.35 ms longer than at `740adb25`, where the module was already
built: the demand, the transcript's and the `List` class. Its whole process is
still 0.27 ms shorter than before, since it never builds `TimeFactory` or
`TypedArraysFactory`. This measurement took one build per side, two programs
and one machine; the boot's spans, 0.5 ms apart, lie far outside their own
run-to-run spread (min to max 1.70–1.98 against 1.19–1.34 ms for the hello).

### What simultaneous slots cost

Simultaneous slots ([object-model.md](object-model.md), "Simultaneous
slots") put three things on or beside the paths every program runs: four
instructions in `vm::run`'s dispatch and a refusal branch in
`InitializeSlot`'s arm; a `Continuation::Future` arm in `vm::finish`; and
the search for a future among a leaf's operands. None of the four fixtures
makes a future, so none runs any of it past a flag test. What they can pay
is the shape of the code around it.

**How it is built, after a first version measured 1–3% slower.** The
first version wrapped every leaf: `builtins::execute` ran the leaf, then
tested its answer and the flag before handing it back, which put a call and
a 56-byte copy on every leaf. The search is now on the callers' failure
paths only -- `mirror::execute_sent` and `send_inlined` before the failure
block, `execute_method` before the method is activated -- behind
`futures_made`: `future::after_a_failed_leaf`, inlined into each caller,
is the flag test, and the search is the cold function behind it
(`see_through_a_failed_leaf`; `send_inlined` tests the flag itself and goes
to the cold `mirror::see_through_a_failed_row`); the leaves that answer
`false` to a stranger decline instead when an operand is a future
(`future::declines_for_a_future`), so the same path sees through it. The
four instructions share one arm of `vm::run` that makes one
out-of-line call (`simultaneous_clause_instruction`), and `InitializeSlot`'s
refusal is one call too (`initialize_slot_refused`); each reads its
instruction back rather than passing its operands. Neither is marked
`#[cold]`: that measured slower (below).

**Measured 2026-09-28, `b0edda5` against `0b1f474`** (the plan commit
before Task 1), by the protocol of "What removing the re-entries moved":
three release builds of each with the pinned `RUSTFLAGS`, the second and
third adding `-C metadata=buildb` and `-C metadata=buildc`, each in its own
`--target-dir`; the base tree exported with `git archive` under the
git-ignored `.temp/`. `ns-next bench <source> --iterations 30`, five rounds
a sitting, the six binaries rotating by one position from round to round,
two sittings; each printed every fixture's recorded answer. Means of the
five rounds' medians, in ms:

Sitting 1:

| Source | `0b1f474` a | `0b1f474` b | `0b1f474` c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.2492 | 5.0058 | 5.2723 | 5.0955 | 5.0963 | 5.0526 |
| `bench_send.ns` | 6.4715 | 6.5001 | 6.4662 | 6.4657 | 6.4646 | 6.6394 |
| `bench_block.ns` | 13.9962 | 13.9744 | 14.0102 | 14.0767 | 14.0102 | 14.1150 |
| `bench_megamorphic.ns` | 9.1590 | 9.1069 | 9.2045 | 9.5094 | 9.5205 | 9.6675 |

Sitting 2:

| Source | `0b1f474` a | `0b1f474` b | `0b1f474` c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.0241 | 5.0530 | 5.0411 | 5.0489 | 5.0393 | 5.0936 |
| `bench_send.ns` | 6.4690 | 6.5186 | 6.4848 | 6.5131 | 6.4569 | 6.4525 |
| `bench_block.ns` | 14.0660 | 14.1051 | 14.0209 | 14.0611 | 14.1068 | 14.0048 |
| `bench_megamorphic.ns` | 9.1352 | 9.1699 | 9.1412 | 9.5171 | 9.5157 | 9.5463 |

Both sittings pooled (n = 30 rounds a side), in ms, and whether every
round of the head lies outside every round of `0b1f474`'s, sitting by
sitting:

| Source | Head mean | `0b1f474` mean | Change | Head median | `0b1f474` median | Change | Sitting 1 | Sitting 2 |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.0710 | 5.1076 | -0.72% | 5.0818 | 5.0386 | +0.86% | overlapping | overlapping |
| `bench_send.ns` | 6.4987 | 6.4850 | +0.21% | 6.4640 | 6.4813 | -0.27% | overlapping | overlapping |
| `bench_block.ns` | 14.0624 | 14.0288 | +0.24% | 14.0692 | 14.0374 | +0.23% | overlapping | overlapping |
| `bench_megamorphic.ns` | 9.5461 | 9.1528 | +4.30% | 9.5400 | 9.1528 | +4.23% | separated | separated |

**Three fixtures are level; `bench_megamorphic.ns` is about 4% slower.**
`dumpbin /disasm` of a release build with debug information
(`CARGO_PROFILE_RELEASE_DEBUG=limited`, the same pinned flags): one
`ns_runtime::vm::run` label, no `vm::run::closure`; **7,083 instructions
against 6,983** at `0b1f474`. `builtins::execute` is one function again,
6,240 instructions against 5,964 (the equality leaves' decline test);
`vm::finish` 1,047 against 679.

**What the remaining 4% is.** Each variant below is three builds by the
same protocol, measured in one sitting beside `0b1f474`'s three (means
against it; the head's own sittings above are the reference):

| Variant | loop | send | block | megamorphic |
|---|---|---|---|---|
| the four arms and the refusal each out of line, marked `#[cold]` | +1.70% | +0.88% | +2.13% | +5.97% |
| the same, not marked `#[cold]` | -1.49% | +1.42% | +0.28% | +3.78% |
| the head, but the one arm answering `Flow::Next` and calling nothing (not a working build) | +0.30% | -0.89% | -0.82% | +1.56% |
| the same, in four more sittings | -0.79% to +0.37% | -1.93% to +0.39% | -0.88% to +1.42% | -0.19% to +1.11% |
| the head, its arm calling and discarding the answer (not a working build) | -0.58% | +1.24% | +1.08% | +3.04% |
| the head with every jump target aligned to 64 bytes (`-align-all-nofallthru-blocks=6` on both sides) | +1.85% | +0.38% | -0.54% | +4.23% |
| `InitializeSlot` and the four instructions in one arm, one call | +2.48% | +1.57% | +2.61% | +5.71% |
| `0b1f474` with an unused function added to `vm.rs` | +0.55% | -0.56% | +0.30% | -0.16% |

Every other part of the change -- the leaves, the callers' failure paths,
`InitializeSlot`'s refusal call, `vm::finish` -- is present in the
"calling nothing" build, and it is level on all four. What costs
`bench_megamorphic.ns` 3–5% is the one arm of `vm::run`'s dispatch that
calls anything at all, whatever it calls and wherever it stands in the
match. It is not alignment -- aligning every jump target leaves it -- and
not the function's size. The jump table shows why it is so sensitive:
the arm reorders LLVM's placement of every block of `vm::run` (the offsets
of `SendLexical`'s and `SendPublic`'s arms move from 0x3e3 and 0x51d to
0xc66 and 0x12a3), and the register assignment with it. An `xperf` profile
(0.12 ms sampling, 300 runs of the fixture a binary) puts the difference
inside `vm::run` (19,022 samples against 18,362 for the "calling nothing"
build and 18,258 for `0b1f474`) and in `decide_slot_accessor` (775 against
516 and 652). In the slower build the block that tests the `Flow` a send
answered -- read back from the stack, where every send writes it -- is a
chain of compares that tests `Next` last, where the faster one tests
`Switch` and branches away; every send of this fixture's loop answers
`Next`. Why one more calling arm moves that block, and how to keep it in
place -- a `Flow` answered in registers, for one -- was open until the loop
over locals took the `Flow` away: `bench_megamorphic.ns` is now about 20%
faster than at `5fbb2cb`, the merge of simultaneous slots ("What the loop
over locals moved" below).

### What the loop over locals moved

"The loop over locals" was measured whole against `5fbb2cb`, the merge of
simultaneous slots and the last commit before its plan, on 2026-09-28, at
`3e515b9`, by the protocol of "What removing the re-entries moved": three
release builds of each with the pinned `RUSTFLAGS`, the second and third
adding `-C metadata=buildb` and `-C metadata=buildc`, each in its own
`--target-dir`; both trees exported with `git archive` under the
git-ignored `.temp/`; no binary imports `VCRUNTIME140.dll`.
`ns-next bench <source> --iterations 30`, five rounds a sitting, the six
binaries rotating by one position from round to round, two sittings; each
printed every fixture's recorded answer. Means of the five rounds' medians,
in ms:

Sitting 1:

| Source | `5fbb2cb` a | `5fbb2cb` b | `5fbb2cb` c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 5.0576 | 5.0800 | 5.0696 | 3.5653 | 4.0068 | 4.0245 |
| `bench_send.ns` | 6.5027 | 6.4392 | 6.4183 | 4.9869 | 4.9614 | 5.0141 |
| `bench_block.ns` | 14.0483 | 13.9870 | 14.0446 | 12.3427 | 12.0347 | 12.0519 |
| `bench_megamorphic.ns` | 9.5076 | 9.4006 | 9.5467 | 7.6219 | 7.7042 | 7.6404 |

Sitting 2:

| Source | `5fbb2cb` a | `5fbb2cb` b | `5fbb2cb` c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_loop.ns` | 4.9839 | 5.0315 | 4.9765 | 3.5303 | 3.5666 | 3.6663 |
| `bench_send.ns` | 6.4298 | 6.3690 | 6.4750 | 5.0443 | 4.9505 | 4.9946 |
| `bench_block.ns` | 13.9500 | 13.9904 | 14.1214 | 12.2500 | 12.0601 | 12.1056 |
| `bench_megamorphic.ns` | 9.5640 | 9.4803 | 9.4788 | 7.5022 | 7.5181 | 7.6583 |

Both sittings pooled (n = 30 rounds a side), in ms, and whether every
round of the head lies outside every round of `5fbb2cb`'s, sitting by
sitting:

| Source | Head mean | `5fbb2cb` mean | Change | Head median | `5fbb2cb` median | Change | Sitting 1 | Sitting 2 |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 3.7266 | 5.0332 | -25.96% | 3.5401 | 5.0368 | -29.72% | overlapping | separated |
| `bench_send.ns` | 4.9919 | 6.4390 | -22.47% | 4.9583 | 6.4631 | -23.28% | separated | separated |
| `bench_block.ns` | 12.1408 | 14.0236 | -13.43% | 12.1163 | 14.0218 | -13.59% | separated | separated |
| `bench_megamorphic.ns` | 7.6075 | 9.4964 | -19.89% | 7.5784 | 9.4659 | -19.94% | separated | separated |

**All four fixtures are faster, by 13–30%.** `bench_loop.ns`'s one
overlapping sitting is two rounds, one of head b and one of head c, at 5.94
and 5.96 ms, above every round of `5fbb2cb`; its other 28 rounds lie between
3.49 and 3.78 ms, and thirty further runs of the three head builds, ten
each, measured 3.47 to 3.75 ms, none near 5.9 -- an event of the machine,
not a mode of the build, and the reason the median is the better figure for
that fixture. `bench_megamorphic.ns` recovers the 4% "What simultaneous
slots cost" left open and gains another 16%.

`dumpbin /disasm` of a release build with debug information
(`CARGO_PROFILE_RELEASE_DEBUG=limited`, the same pinned flags): one
`ns_runtime::vm::run` label, no `vm::run::closure` and no `run_stretch` --
the stretch and every helper it forces inline are in `run`; **7,143
instructions against 7,083** at `5fbb2cb`. Its stack frame grows from
0x378 to 0x3e8 bytes. `vm::finish` is 1,686 instructions against 1,552,
`vm::unwind` 2,157 against 2,127, and `Handoff`'s drop glue, 73
instructions, is a function of its own.

**Robustness: one more calling arm.** The 4% was found measuring
simultaneous slots ("What simultaneous slots cost" above), and the
bisection there ("What the remaining 4% is") located it to the one arm of
`vm::run`'s dispatch that calls anything. A scratch copy of the head, never
committed, checks that finding with a second calling arm: it added
`Instruction::RobustnessProbe`, which no lowering emits, whose arm in
`run_stretch` calls an
`#[inline(never)]` function (it reads its instruction back, writes the
receiver to a register and answers `Transfer::Next`, or `finish`es on an
impossible operand) and answers its `Transfer`; every other exhaustive
match over `Instruction` got an `unreachable!` arm. Three builds by the same
protocol, measured beside the head's three in two sittings. Means of the
five rounds' medians, in ms:

| Source | Sitting | Head a | Head b | Head c | Probe a | Probe b | Probe c |
|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 1 | 3.6184 | 3.5460 | 3.5990 | 3.7224 | 3.6793 | 3.7442 |
| `bench_loop.ns` | 2 | 3.6683 | 3.5261 | 3.5821 | 3.6877 | 3.6930 | 3.6680 |
| `bench_send.ns` | 1 | 4.9723 | 4.9422 | 4.9083 | 5.1409 | 5.0649 | 5.1124 |
| `bench_send.ns` | 2 | 4.9317 | 4.9534 | 4.9902 | 5.0853 | 5.1148 | 5.1440 |
| `bench_block.ns` | 1 | 12.2761 | 12.2707 | 12.1793 | 12.1951 | 12.1504 | 12.0472 |
| `bench_block.ns` | 2 | 12.2593 | 12.0661 | 12.0287 | 12.1192 | 12.1118 | 12.1087 |
| `bench_megamorphic.ns` | 1 | 7.6714 | 7.7576 | 7.6870 | 7.6672 | 8.0558 | 7.7013 |
| `bench_megamorphic.ns` | 2 | 7.5696 | 8.0299 | 7.4985 | 7.5944 | 7.6355 | 7.6024 |

Pooled (n = 30 rounds a side):

| Source | Probe mean | Head mean | Change | Probe median | Head median | Change | Sitting 1 | Sitting 2 |
|---|---|---|---|---|---|---|---|---|
| `bench_loop.ns` | 3.6991 | 3.5900 | +3.04% | 3.6771 | 3.5650 | +3.14% | overlapping | overlapping |
| `bench_send.ns` | 5.1104 | 4.9497 | +3.25% | 5.1277 | 4.9349 | +3.91% | overlapping | overlapping |
| `bench_block.ns` | 12.1221 | 12.1800 | -0.48% | 12.1137 | 12.2017 | -0.72% | overlapping | overlapping |
| `bench_megamorphic.ns` | 7.7094 | 7.7023 | +0.09% | 7.6513 | 7.5937 | +0.76% | overlapping | overlapping |

**`bench_megamorphic.ns` and `bench_block.ns` do not move; `bench_loop.ns`
and `bench_send.ns` are about 3% slower.** No round is separated, but on
`bench_send.ns` every probe build's mean lies above every head build's in
both sittings, and on `bench_loop.ns` in the first and all but touching in
the second, so the claim that one more calling arm moves no fixture does
not hold: the fixture that paid the `Flow` test is immune now, and two
others are not. In the probe build `vm::run` is 7,149 instructions, and
its stack frame 0x408 bytes against the head's 0x3e8 -- the arm changes the
register assignment of the whole function, as it did before.

**Where the 3% goes: a store-forwarding stall on the send path.** Probed
2026-09-28 with `xperf` (0.12 ms sampling), 400 runs of `bench_loop.ns` on
a head build and on the probe build, both with debug information, the
samples attributed through `vm::run`'s jump table
(`.temp/loop/`, not committed). The fixture's loop is lexical slot sends
(`n`, `sum`, `sum:`, `n:`), `IntegerBinary` and a backward jump, and its
cost is not where the dispatch is:

- Every send's decision reaches the loop as a `Result<Step, Signal>` in a
  stack slot (`[rbp+1A0h]` in the head build): the inline cache's hit
  paths and its out-of-line `inline_cache::miss` join in one block that 22
  jumps reach, so the result cannot stay in registers. That block reads the
  16-byte `Value` back with one `movups` from a slot the paths wrote with
  narrower stores -- a load the store buffer cannot forward -- and the
  store after it, where the stall's samples land, holds **28% of all
  samples in `vm::run`** (2,946 of 10,319). Two more sites of the same
  shape, a `Value` copied between stack slots in pieces (a `word`, a
  `dword`, a `qword`) and then read whole, hold about 20% more.
- The same block then calls the out-of-line `drop_glue<Step>` for every
  send, a `Step::Value` included: 4.7% of all samples.
- In the probe build that one store holds 3,272 samples, +326 of the +360
  `vm::run` gained (the fixture +3.9% in this run). The extra arm adds no
  work; it moves the stall. Why the timing shifts is not established; that
  the loss sits at the stall is.
- Beside it: `program_counter` and the code pointer live in stack slots in
  the head build (`[rbp+2F8h]`, `[rbp+338h]`), since `vm::run` is large
  enough that its allocator spills -- about 1,900 stack references in
  7,143 instructions -- and the probe build happens to keep
  `program_counter` in `r13`; and every dispatch decodes `Instruction`'s
  discriminant through a niche (`xor`, `cmovns`) before its jump.

A decision that is a `Step::Value` must not pass through a joined stack
slot, and must not reach `Step`'s drop glue. Delivering it where it is
decided was tried and halted ("A send's value delivered where it is decided:
tried and halted" below): the stall belonged to the 16-byte enum `Value`
itself, and a value is now one tagged word, read and written whole
([object-model.md](object-model.md), "Values and objects"); what that moved
is "What the eight-byte value moved". The dispatch findings -- the spilled
`program_counter` and code pointer, the discriminant decoded through a
niche -- are the input of "The compact instruction encoding" below, which
replaced the stored enum with eight-byte slots; what that moved is "What
the compact instruction encoding moved".

### What frames as words moved

Road step 7a ("Representation" above) was measured whole against `master`
(`d135623a`, the tree before it) at `9c11b943` on 2026-10-24, by the
protocol of "What suspending and resuming moved" below with one
extension: `master` exported with `git archive` under the git-ignored
`.temp/`, the branch exported afresh, `cargo xtask shipped-code` in each
tree before its builds; the eight micro sources (the seven of 7b's
measurement and `bench_do.ns`) at `--iterations 30`, five rounds a
sitting, and the sixteen Are We Fast Yet programs, three rounds a sitting,
the ten binaries of both protocols below rotating by one position from
round to round; two sittings, every run printing its recorded answer or
`<Name>: ok`. Every binary has one `vm::run` and no `vm::run::closure` or
`run_stretch` of its own, and none imports `VCRUNTIME140.dll`.

**The extension: the three builds a side vary the block layout inside
`vm::run`.** Build a carries the pinned `RUSTFLAGS`; build b adds `-C
metadata=buildb -C llvm-args=-align-all-nofallthru-blocks=4`, build c `-C
metadata=buildc -C llvm-args=-align-all-nofallthru-blocks=5`, the same
three on both sides. The step's first measurement, with the three builds
told apart by `-C metadata` alone, found `bench_loop.ns` 1.3% slower after
the fix of its one real cost, with the builds separated -- yet the hot
path's machine code was then instruction for instruction what it was
before the regression, the one block sampled more was the run loop's
shared indirect dispatch jump on identical instructions, and with every
block not reached by fall-through aligned to 32 bytes both trees measured
the same. `-C metadata` changes the order of functions and so their
addresses, but not the layout inside one: a disassembly
(`CARGO_PROFILE_RELEASE_DEBUG=limited`, each build's flags) shows `vm::run`
of the three `-C metadata` builds of one tree identical instruction for
instruction at the same offsets from its start, only the addresses of
other functions and data differing, so such a pool samples one layout of
the run loop three times. The two alignments give it two more: on the
branch, `vm::run` is 38,847 bytes with 513 padding instructions in build a,
41,919 with 903 in build b and 45,311 with 1,128 in build c (`master`:
39,420, 42,556, 44,988), the same 7,546 instructions apart from the
padding in all three (`master`: 7,587), and the measured binaries' `vm::run`
matching their debug-information builds' at the same offsets.

Means of each build's medians, pooled over both sittings (n = 10 rounds a
build for the micro sources, 6 for Are We Fast Yet), in ms; "builds" says
whether the three build means of one side all lie above or below the
other side's:

| Source | `master` | 7a | change | `master` a, b, c | 7a a, b, c | builds |
|---|---|---|---|---|---|---|
| `bench_block.ns` | 7.841 | 7.399 | -5.6% | 7.806, 7.863, 7.854 | 7.398, 7.432, 7.369 | faster |
| `bench_loop.ns` | 2.365 | 2.374 | +0.4% | 2.350, 2.368, 2.377 | 2.388, 2.375, 2.358 | overlapping |
| `bench_megamorphic.ns` | 5.223 | 4.925 | -5.7% | 5.183, 5.242, 5.244 | 4.940, 4.915, 4.922 | faster |
| `bench_send.ns` | 3.623 | 3.445 | -4.9% | 3.621, 3.633, 3.616 | 3.460, 3.434, 3.442 | faster |
| `bench_new.ns` | 2.983 | 2.776 | -6.9% | 2.986, 2.971, 2.992 | 2.790, 2.755, 2.782 | faster |
| `bench_new2.ns` | 3.849 | 3.673 | -4.6% | 3.853, 3.834, 3.861 | 3.686, 3.643, 3.691 | faster |
| `bench_newarg.ns` | 3.871 | 3.716 | -4.0% | 3.910, 3.853, 3.848 | 3.733, 3.681, 3.734 | faster |
| `bench_do.ns` | 2.692 | 2.663 | -1.1% | 2.683, 2.726, 2.666 | 2.667, 2.663, 2.660 | overlapping |
| Bounce | 0.961 | 0.938 | -2.4% | 0.959, 0.969, 0.956 | 0.957, 0.930, 0.927 | overlapping |
| CD | 3635.4 | 3560.5 | -2.1% | 3602.2, 3659.6, 3644.2 | 3579.5, 3540.4, 3561.7 | faster |
| DeltaBlue | 660.10 | 634.86 | -3.8% | 658.68, 658.21, 663.42 | 639.80, 631.55, 633.22 | faster |
| DeltaBlueTon80 | 7.356 | 7.055 | -4.1% | 7.385, 7.282, 7.402 | 7.064, 7.031, 7.072 | faster |
| Havlak | 3447.9 | 3322.0 | -3.7% | 3460.4, 3439.3, 3444.0 | 3336.8, 3286.5, 3342.9 | faster |
| Json | 10.633 | 10.304 | -3.1% | 10.599, 10.666, 10.634 | 10.253, 10.342, 10.318 | faster |
| List | 1.006 | 0.957 | -4.9% | 1.006, 1.017, 0.994 | 0.961, 0.950, 0.959 | faster |
| Mandelbrot | 3020.7 | 2990.8 | -1.0% | 3023.6, 3004.1, 3034.5 | 2985.2, 3006.6, 2980.5 | overlapping |
| NBody | 2241.1 | 2223.0 | -0.8% | 2215.9, 2248.5, 2259.0 | 2230.9, 2223.3, 2214.7 | overlapping |
| Permute | 2.498 | 2.377 | -4.8% | 2.509, 2.474, 2.511 | 2.354, 2.373, 2.403 | faster |
| Queens | 2.110 | 2.019 | -4.3% | 2.108, 2.112, 2.108 | 2.028, 2.013, 2.016 | faster |
| Richards | 68.178 | 64.832 | -4.9% | 68.211, 68.646, 67.676 | 64.965, 64.559, 64.971 | faster |
| RichardsTon80 | 4.618 | 4.488 | -2.8% | 4.646, 4.610, 4.600 | 4.508, 4.518, 4.440 | faster |
| Sieve | 1.374 | 1.322 | -3.8% | 1.381, 1.360, 1.382 | 1.314, 1.334, 1.317 | faster |
| Storage | 1.502 | 1.426 | -5.1% | 1.495, 1.495, 1.517 | 1.409, 1.444, 1.425 | faster |
| Towers | 3.343 | 3.078 | -7.9% | 3.347, 3.350, 3.331 | 3.084, 3.115, 3.035 | faster |

**No program is slower with its builds separated, so 7a is taken.**
Nineteen of the twenty-four are faster with the builds separated; the
other five overlap, `bench_loop.ns` the only one above `master`, by 0.4%
(+0.1% and +0.7% in the two sittings, overlapping in both). Its builds
show what the extension samples: against `master`'s build of the same
layout the branch is +1.6% in build a, +0.3% in build b and -0.8% in
build c.

The same sittings also ran the three `-C metadata` builds a side of the
step's first measurement, builds b and c without the alignments: there
`bench_loop.ns` is +1.7% (2.349 against 2.388 ms, the builds 2.348-2.350
against 2.387-2.389, separated in both sittings) and NBody +0.5% (2221.0
against 2231.6 ms, separated in the first sitting only), and every other
program is level or faster as above. Under that protocol the rule would
fail on those two; it is the extended one that decides, since the other
samples a single layout of the run loop.

Not every gain is the frames'. `ce7cdd31` and `9c11b943` changed
`ns-memory`'s `prefix` and `accessible` -- one comparison to lend the
committed prefix, its end kept in words -- and every object access in the
object space goes through them as every stack word does; the measurement
does not separate the two, and the allocation- and object-heavy programs
(Towers, Storage, Permute, the `bench_new*` sources) are where the second
could show. What the measurement did not cover: one machine (Intel Core
Ultra 9 285K) and operating system, not idle, two sittings back to back the
same afternoon; data layout (symbol ids, object offsets, dispatch ids),
which neither `-C metadata` nor the block alignment varies; the alignment
of a function's blocks other than `vm::run`'s, which the two flags vary
too but which was not checked; `master`'s build a and its two `-C
metadata` builds were built earlier the same day from the same tree with
the same flags, and cargo found them current; no program signalling
exceptions in its inner loop.

### What suspending and resuming moved

Road step 7b ("Suspending and resuming" above) was measured whole against
`master` (`637db0bb`, the tree before it) at `b454d37c` on 2026-10-24, by
the protocol of "What `new` as compiled code moved" below: `master` checked
out under the git-ignored `.temp/`, `cargo xtask shipped-code` in each tree
before its builds, three release builds a side with the pinned `RUSTFLAGS`,
each in its own `--target-dir`, all six binaries distinct and none
importing `VCRUNTIME140.dll`; the seven micro sources at `--iterations 30`,
five rounds a sitting, and the sixteen Are We Fast Yet programs, three
rounds a sitting, the six binaries rotating by one position from round to
round; two sittings, every run printing its recorded answer or
`<Name>: ok`. The branch's binary has no `vm::run::closure` and no
`run_stretch` of its own, and `vm::return_to_suspended` is a function of
its own, out of the return path.

Means of each build's medians, pooled over both sittings, in ms:

| Source | `master` | 7b | change | `master` per build | 7b per build |
|---|---|---|---|---|---|
| `bench_block.ns` | 7.634 | 7.688 | +0.7% | 7.649, 7.597, 7.655 | 7.693, 7.736, 7.635 |
| `bench_loop.ns` | 2.373 | 2.354 | -0.8% | 2.372, 2.378, 2.368 | 2.355, 2.351, 2.356 |
| `bench_megamorphic.ns` | 5.162 | 5.153 | -0.2% | 5.168, 5.141, 5.175 | 5.153, 5.166, 5.139 |
| `bench_send.ns` | 3.577 | 3.582 | +0.1% | 3.597, 3.572, 3.563 | 3.579, 3.593, 3.575 |
| `bench_new.ns` | 2.940 | 2.947 | +0.2% | 2.958, 2.949, 2.913 | 2.941, 2.961, 2.939 |
| `bench_new2.ns` | 3.837 | 3.846 | +0.3% | 3.866, 3.822, 3.822 | 3.849, 3.847, 3.843 |
| `bench_newarg.ns` | 3.825 | 3.864 | +1.0% | 3.843, 3.840, 3.791 | 3.849, 3.862, 3.883 |
| Bounce | 0.957 | 0.957 | 0.0% | 0.954, 0.961, 0.956 | 0.955, 0.957, 0.959 |
| CD | 3571.7 | 3551.1 | -0.6% | 3565.8, 3575.6, 3573.7 | 3556.6, 3549.3, 3547.6 |
| DeltaBlue | 645.67 | 646.43 | +0.1% | 645.92, 645.98, 645.10 | 643.67, 648.63, 646.98 |
| DeltaBlueTon80 | 7.200 | 7.222 | +0.3% | 7.193, 7.167, 7.240 | 7.190, 7.242, 7.233 |
| Havlak | 3404.8 | 3382.8 | -0.6% | 3401.1, 3386.3, 3427.0 | 3377.5, 3373.8, 3397.0 |
| Json | 10.421 | 10.491 | +0.7% | 10.431, 10.374, 10.457 | 10.501, 10.433, 10.540 |
| List | 1.009 | 0.995 | -1.4% | 1.007, 1.010, 1.009 | 0.994, 0.994, 0.997 |
| Mandelbrot | 2958.8 | 2978.9 | +0.7% | 2969.1, 2943.1, 2964.2 | 2989.0, 2966.9, 2980.9 |
| NBody | 2196.6 | 2201.9 | +0.2% | 2197.9, 2198.7, 2193.2 | 2207.4, 2191.9, 2206.5 |
| Permute | 2.476 | 2.477 | +0.1% | 2.475, 2.469, 2.483 | 2.476, 2.475, 2.481 |
| Queens | 2.104 | 2.102 | -0.1% | 2.105, 2.097, 2.112 | 2.100, 2.109, 2.098 |
| Richards | 66.108 | 66.366 | +0.4% | 66.292, 66.025, 66.006 | 66.613, 66.230, 66.254 |
| RichardsTon80 | 4.501 | 4.547 | +1.0% | 4.488, 4.529, 4.487 | 4.499, 4.569, 4.573 |
| Sieve | 1.376 | 1.377 | +0.1% | 1.369, 1.396, 1.363 | 1.381, 1.383, 1.368 |
| Storage | 1.480 | 1.489 | +0.6% | 1.479, 1.487, 1.474 | 1.482, 1.487, 1.499 |
| Towers | 3.337 | 3.362 | +0.8% | 3.343, 3.372, 3.297 | 3.325, 3.381, 3.382 |

Every program is within -1.4% to +1.0% of `master`, and every group's
builds overlap the other side's but List's and `bench_loop.ns`'s, both
faster. The changes 7b makes to paths that run without a suspension -- a
cold arm in `vm::finish` after `Deliver`, the program counter stored when
a stretch leaves for a safepoint, the handler leaves reading through the
frame, the unwinder deciding its destination before every pop -- cost
nothing measurable; the measurement does not cover a program that signals
exceptions in its inner loop, since none of the twenty-three does.

### What contiguous arguments moved

The first stage of road step 7a′ ("Overlapping register windows"
below) -- a send's arguments in contiguous slots or read in place from
consecutive sources, decisions reading them through an argument window, the
argument buffer gone, the push copying once -- was measured whole against
`master` (`3a67072f`, the tree before it) at `95d421a8` on 2026-10-25, by
the protocol of "What frames as words moved" above: `master` checked out
and the branch exported with `git archive` under the git-ignored `.temp/`,
`cargo xtask
shipped-code` in each before its builds, three release builds a side with
the pinned `RUSTFLAGS` varying the block layout inside `vm::run` (build a
the pin alone, b and c adding the two alignments), each in its own
`--target-dir`; the eight micro sources at `--iterations 30`, five rounds a
sitting, and the sixteen Are We Fast Yet programs, three rounds a sitting,
the six binaries rotating by one position from round to round; two
sittings, every run printing its recorded answer or `<Name>: ok`. The two
trees' `benchmarks/` and core library are the same, and both sides ran one
copy of it. Every binary has one `vm::run` and one `vm::run_wide`, no
`vm::run::closure` and no `run_stretch` of its own, and none imports
`VCRUNTIME140.dll`; each measured binary's `vm::run` is its
debug-information build's instruction for instruction at the same offsets,
64-byte aligned. `vm::run` is 44,736 bytes with 498 padding instructions in
build a, 47,872 with 891 in build b and 50,880 with 1,089 in build c
(`master`: 38,848, 41,920, 45,312 with 513, 903, 1,128), the same 8,296
instructions apart from the padding in all three (`master`: 7,546, so +750),
and its stack frame is `0x368` bytes (`master`: `0x398`).

Means of each build's medians, pooled over both sittings (n = 10 rounds a
build for the micro sources, 6 for Are We Fast Yet), in ms; "builds" says
whether the three build means of one side all lie above or below the
other side's:

| Source | `master` | stage 1 | change | `master` a, b, c | stage 1 a, b, c | builds |
|---|---|---|---|---|---|---|
| `bench_block.ns` | 7.300 | 7.184 | -1.6% | 7.314, 7.313, 7.274 | 7.189, 7.162, 7.200 | faster |
| `bench_loop.ns` | 2.333 | 2.330 | -0.1% | 2.350, 2.322, 2.328 | 2.335, 2.325, 2.330 | overlapping |
| `bench_megamorphic.ns` | 4.824 | 4.779 | -0.9% | 4.839, 4.809, 4.825 | 4.787, 4.771, 4.780 | faster |
| `bench_send.ns` | 3.375 | 3.289 | -2.5% | 3.371, 3.370, 3.383 | 3.296, 3.283, 3.288 | faster |
| `bench_new.ns` | 2.736 | 2.696 | -1.5% | 2.752, 2.722, 2.735 | 2.688, 2.668, 2.733 | overlapping |
| `bench_new2.ns` | 3.627 | 3.584 | -1.2% | 3.646, 3.593, 3.641 | 3.557, 3.550, 3.644 | overlapping |
| `bench_newarg.ns` | 3.647 | 3.600 | -1.3% | 3.675, 3.607, 3.660 | 3.590, 3.561, 3.649 | overlapping |
| `bench_do.ns` | 2.615 | 2.670 | +2.1% | 2.629, 2.614, 2.601 | 2.626, 2.654, 2.731 | overlapping |
| Bounce | 0.920 | 0.909 | -1.2% | 0.929, 0.926, 0.903 | 0.900, 0.903, 0.924 | overlapping |
| CD | 3505.2 | 3437.7 | -1.9% | 3530.7, 3488.1, 3496.9 | 3435.7, 3406.6, 3470.7 | faster |
| DeltaBlue | 626.92 | 614.12 | -2.0% | 630.38, 622.70, 627.69 | 614.40, 609.53, 618.44 | faster |
| DeltaBlueTon80 | 6.912 | 6.821 | -1.3% | 6.920, 6.918, 6.900 | 6.848, 6.768, 6.847 | faster |
| Havlak | 3262.7 | 3238.4 | -0.7% | 3278.1, 3247.5, 3262.6 | 3236.3, 3221.1, 3257.7 | overlapping |
| Json | 10.162 | 9.965 | -1.9% | 10.161, 10.188, 10.138 | 9.963, 9.899, 10.032 | faster |
| List | 0.938 | 0.949 | +1.2% | 0.939, 0.924, 0.952 | 0.946, 0.949, 0.953 | overlapping |
| Mandelbrot | 2954.1 | 2814.6 | -4.7% | 2952.6, 2940.9, 2968.8 | 2802.9, 2816.9, 2824.1 | faster |
| NBody | 2194.8 | 2105.6 | -4.1% | 2198.9, 2195.4, 2190.1 | 2104.4, 2102.0, 2110.2 | faster |
| Permute | 2.347 | 2.308 | -1.7% | 2.350, 2.347, 2.345 | 2.300, 2.302, 2.321 | faster |
| Queens | 1.998 | 1.975 | -1.1% | 1.999, 2.011, 1.984 | 1.972, 1.965, 1.990 | overlapping |
| Richards | 63.929 | 63.210 | -1.1% | 64.022, 63.694, 64.072 | 62.644, 63.549, 63.436 | faster |
| RichardsTon80 | 4.426 | 4.357 | -1.6% | 4.457, 4.421, 4.400 | 4.370, 4.332, 4.369 | faster |
| Sieve | 1.309 | 1.322 | +1.0% | 1.308, 1.333, 1.286 | 1.327, 1.314, 1.325 | overlapping |
| Storage | 1.414 | 1.418 | +0.3% | 1.426, 1.414, 1.401 | 1.437, 1.409, 1.410 | overlapping |
| Towers | 3.082 | 3.095 | +0.4% | 3.044, 3.082, 3.120 | 3.100, 3.103, 3.082 | overlapping |

**No program is slower with its builds separated, so the first stage is
kept.** Twelve of the twenty-four are faster with the builds separated,
Mandelbrot and NBody by 4-5%; the other twelve overlap, five of them above
`master`. `bench_do.ns` is the nearest: +2.1%, its fastest stage-1 build
0.1% below `master`'s slowest, separated in the second sitting (+2.7%) and
overlapping in the first (+1.6%). Sieve, +1.0%, was separated in the first
sitting (+3.4%) and below `master` in the second (-1.4%); List, Storage and
Towers overlap in both. `bench_do.ns` is level, not faster: what remains of
it sits in functions whose code is `master`'s -- `vm::finish`,
`array_element`, `Heap::pop_activation` -- as layout, beside one register
more in its block, the one-argument fallback's slot, allocated and never
written, which the second stage's push onto the slots decides about.

What the stage taught, as reasons the current design still stands on:

- **A sixteen-byte argument window built by narrow stores and passed by
  reference stalls on store forwarding.** Every read of it waits for the
  stores to retire, since the store buffer cannot forward narrow stores to
  a wider load: the stage measured Mandelbrot +24% in the step that
  introduced it, +28% for the whole stage, NBody +15-19%, until the window
  travelled in registers. So the window is a tag and one word, its slots
  packed into a `u64` by arithmetic (`heap::ArgumentSlots`; "No re-entry
  from Rust" below).
- **A placement loop inlined into every send path costs `vm::run` its
  registers.** With each send copying its sources into its slots inline,
  `vm::run` was 8,512 instructions with a `0x3B8`-byte frame, against
  8,296 and `0x368` with the placement out of line, and `bench_loop.ns`,
  `bench_send.ns` and Richards were 2-8% slower than `master`, level or
  faster once it went. So a send reads consecutive sources in place -- none or
  one always are -- tests a two-argument send's with one comparison, and
  places only non-consecutive sources, out of line (`vm::place_arguments`).
- **A push that indexes the whole frame stack again after taking its frame
  pays reloads and bounds checks on every push.**
  Indexing `words_mut()` with computed offsets after storing the
  continuation row reloaded the stack's base, length and top and checked
  three ranges per activation: about 2 ns per block pushed
  (`bench_do.ns`'s `do:`, a `whileTrue:` the compiler did not inline), independent of the
  block's body and of where the frames sit, `Mutator::push_step` holding a
  quarter to a third more samples. So `FrameStack::push` answers the words
  below the frame and the frame's own as two slices, and
  `Heap::push_activation_in` writes through them ("Representation" above).

What the measurement did not cover: one machine (Intel Core Ultra 9 285K)
and operating system, not idle, two sittings back to back; data layout
(symbol ids, object offsets, dispatch ids), which neither `-C metadata` nor
the block alignment varies; the alignment of a function's blocks other than
`vm::run`'s; no program signalling exceptions in its inner loop. `master`'s
six builds were made the day before from its checkout with the same flags,
and checked unchanged by a fresh disassembly; both sides were timed in the
same sittings. Two costs the stage leaves are open
([OPEN-WORK.md](../OPEN-WORK.md), "Planned: performance and
representation"): `vm::run` reloads the `Mutator` pointer from its frame
where `master` keeps it in a register, and `builtins::execute`'s result
comes back through a stack copy written by narrow stores, which `master`
pays as well.

### What overlapping windows moved

The second stage of road step 7a′ ("Overlapping register windows" below)
-- a send's own callee laid on its sender's argument slots, the header 13
words in every build and left out of the register numbering, the pop
restoring the top it was pushed at, suspension, frame objects and image
records without the header's numbers, and the decoder's checks -- was
measured against the first by the protocol of "What contiguous arguments
moved" above, on 2026-10-25, in three rounds.

**As first built (`0037350d`), every program but `bench_loop.ns` was
slower than the first stage with its builds separated**, by up to +19.2%
(`bench_new.ns` +18.8%, `bench_send.ns` +12.7%, Richards +12.2%). What
it taught, as reasons the current design stands on:

- **A 72-byte header moved by value through the push's inlined layers is
  copied into each of them.** `Mutator::push_sent` grew from 276 to 7,265
  instructions, each move of the `ActivationHeader` a copy LLVM did not
  elide, since the header escapes into the refusal's box, and each copy
  read sixteen bytes wide right after the narrow stores that built it, a
  store-forwarding stall on every send. So the header is lent to the
  heap's push, and only the cold refusal takes it by value
  ("Representation" above; "Keeping the hot path in registers" below).
- **With the header's numbers left out, a register read through an
  `ActivationReference` reads the frame's argument count**, from its
  flags. So an indirection vector's loads and stores take the run loop's
  register base, which it holds already.
- **A larger push pushes small helpers past LLVM's inlining budget**:
  `Continuations::insert`, `row_of` and `Heap::fill_copied_values` became
  calls. They are `#[inline(always)]`.
- **A send a primitive or an accessor answers dropped its `Step` through
  out-of-line drop glue**, in both stages. `deliver` forgets it instead.
  Since that removes a cost the first stage has too -- the first stage
  with that change alone measured 0% to 5.5% faster than without, on one
  build a side -- the second stage is judged against the first stage
  with it, "stage 1 + forget" below.

With those, and the caller word packing the top a frame was pushed at so
that a pop reads no caller header, the second stage (`d7488e20`) was 14 of
24 programs slower than stage 1 + forget with the builds separated, by
+1.1% to +2.3%, and faster on none. A return that wrote its value through
the register base the switch finds, so that it read the flags once, grew
`vm::run` from 8,269 to 8,351 instructions and its frame by sixteen bytes
and measured 0.5-3% slower on seven of nine programs, and was reverted.
What remained was mostly the pop's nil fill over the words a callee laid
on its sender's slots gives back: a build without it, unsafe and only
measured, was 0.4-2.8% faster. Decided with the user on 2026-10-25: the
fill goes, replaced by the leftover analysis, rather than the cost being
accepted or the stage reverted ("What the leftover analysis moved"
below).

**With the analysis (`f2faa8f4`), against stage 1 + forget**, three builds
a side, both sittings pooled, in ms; "builds" compares the three build
means as above:

| Source | stage 1 + forget | second stage | change | builds |
|---|---|---|---|---|
| `bench_block.ns` | 7.121 | 7.156 | +0.5% | overlapping |
| `bench_loop.ns` | 2.155 | 2.145 | -0.5% | overlapping |
| `bench_megamorphic.ns` | 4.691 | 4.634 | -1.2% | faster |
| `bench_send.ns` | 3.223 | 3.226 | +0.1% | overlapping |
| `bench_new.ns` | 2.736 | 2.690 | -1.7% | faster |
| `bench_new2.ns` | 3.597 | 3.584 | -0.3% | overlapping |
| `bench_newarg.ns` | 3.625 | 3.645 | +0.5% | overlapping |
| `bench_do.ns` | 2.599 | 2.616 | +0.7% | overlapping |
| Bounce | 0.890 | 0.888 | -0.2% | overlapping |
| CD | 3379.8 | 3394.7 | +0.4% | overlapping |
| DeltaBlue | 609.08 | 609.22 | +0.0% | overlapping |
| DeltaBlueTon80 | 6.686 | 6.790 | +1.6% | slower |
| Havlak | 3215.0 | 3211.1 | -0.1% | overlapping |
| Json | 9.763 | 9.811 | +0.5% | overlapping |
| List | 0.931 | 0.915 | -1.7% | overlapping |
| Mandelbrot | 2712.1 | 2725.2 | +0.5% | slower |
| NBody | 1972.4 | 1976.5 | +0.2% | overlapping |
| Permute | 2.325 | 2.311 | -0.6% | overlapping |
| Queens | 1.938 | 1.949 | +0.6% | overlapping |
| Richards | 61.578 | 61.697 | +0.2% | overlapping |
| RichardsTon80 | 4.279 | 4.273 | -0.1% | overlapping |
| Sieve | 1.317 | 1.334 | +1.3% | overlapping |
| Storage | 1.406 | 1.372 | -2.4% | faster |
| Towers | 3.032 | 3.029 | -0.1% | overlapping |

**The geometric mean over the twenty-four is -0.09%**: three programs
faster with the builds separated, nineteen overlapping, and two slower,
DeltaBlueTon80 by 1.6% and Mandelbrot by 0.5% (diagnosed below).
`vm::run` is 8,269 instructions apart from the padding with a
`0x318`-byte frame, against the first stage's 8,296 and `0x368`. So
overlapping the windows costs the interpreter nothing on the geometric
mean; what it removes -- the copy of every argument into the callee's
window -- is what compiled frames would otherwise pay too, and the
header between the arguments and the temporaries is the layout compiled
frames keep too ("Planned: the deoptimization contract").

What the measurement did not cover is what "What contiguous arguments
moved" lists; stage 1 + forget is a tree built for the comparison, not a
commit of the branch.

**DeltaBlueTon80's gap, diagnosed** (2026-10-26). Profiled with xperf on
the debug-information twins, 3,000 iterations, builds a and c, the second
stage at `6ff8890a` against stage 1 + forget: the pushes together were
faster (`push_sent` and `push_step` 13,784 samples against stage 1's
14,481), and three costs were the stage's own:

- **The pop's two counts, decremented as one 16-byte load.** The live and
  the counted count lie side by side, and LLVM subtracted both with one
  `movdqu`/`psubq`/`movdqu` over the push's two narrower stores of them,
  which no store forwards. Stage 1 has the same instruction, but the
  second stage's pop also read the frame's flags and caller words again
  after the divorce check to find the top it was pushed at, and
  `Heap::pop_activation` rose from 2,862 samples to 4,005. A branch on
  the counted flag now keeps the two subtractions apart, and the top is
  computed from the words the pop read first (`72de5ac0`): 2,782
  samples, below stage 1's.
- **A setter of an inlined conditional's value paid a `Move`.**
  `direction:: (...) ifTrue: [...] ifFalse: [...]`, in `chooseMethod:`,
  left its value in the conditional's result register above the slot,
  which the second stage copies into the slot by a `Move` of its own
  where the first stage's send copied it. The conditional, and
  `and:`/`or:`, now answer in the slot they are lowered into
  (`ca5ce8b3`; [compiler.md](compiler.md), "Registers"). Over
  DeltaBlueTon80's methods the two stages now compile to the same number
  of instructions.
- **The register base reads the frame's flags on every switch.** The
  frame's start lies `argument_count` words below its header, so the
  loop's register base after a push or a return
  (`Heap::activation_register_base`) is a load of the flags word where
  the first stage added 13 to the offset; `resume_caller` reads it once
  more to write the value. The loop's
  switch handler took 2,189 samples against 1,619, about 0.35% of the
  run. Three ways of not reading it were built and measured against the
  fix, three builds a side, DeltaBlueTon80, `bench_send.ns` and Richards,
  six rounds (and DeltaBlueTon80 six more for the third), and none was
  kept:
  - **The base in the `Handoff`**, `resume_caller` reading the flags
    once: DeltaBlueTon80 +0.8%, builds separated; every switch site
    computes the base then, and the handoff passes through memory.
  - **The base in `Transfer::Switched`**, answered in a register:
    DeltaBlueTon80 +0.6% and `bench_send.ns` +1.1%, both separated, with
    every switch site computing it (`vm::run` 8,269 to 8,418
    instructions). Carried only from a return, the loop reading the flags
    after a push as before: `bench_send.ns` +1.6% and Richards +0.9%,
    separated, since a `Transfer` with a payload is no longer one byte on
    every send's path.
  - **The base from the code's argument count**, which the loop holds,
    instead of the frame's: every push takes as many arguments as its
    body -- an argument list longer than `u16::MAX`, which a saturated
    body stands for, is refused before it pushes -- and a frame resumed
    from its object or decoded from an image takes its body's
    (`Heap::frame_argument_count`, which the decoder's frame-shape check
    holds), and every golden and suite test but the heap's own fixtures,
    which push code of another arity, ran with that asserted. It
    measured level: DeltaBlueTon80 -0.0% over twelve rounds, builds
    overlapping, `bench_send.ns` +0.5%. The read is a load either way,
    the code's field behind the handoff's code as the flags behind the
    handoff's activation.

After the first two, three builds a side, two sittings pooled (n = 12),
in ms:

| Source | stage 1 + forget | `6ff8890a` | fixed (`ca5ce8b3`) | fixed vs stage 1 + forget | builds |
|---|---|---|---|---|---|
| DeltaBlueTon80 | 6.643 | 6.733 | 6.700 | +0.9% | slower |
| Mandelbrot | 2698.1 | 2691.6 | 2690.9 | -0.3% | overlapping |

**Mandelbrot's +0.5% did not reproduce**: `6ff8890a` itself measured
-0.2% against stage 1 + forget, builds overlapping. **DeltaBlueTon80's
remaining +0.9% is the return path**: the flags read of the third point,
which the frame layout makes, and `vm::finish` and `vm::resume_caller`,
about 550 and 340-600 samples above stage 1 in both builds profiled,
their samples on the loads after the pop -- the top it stored, the
popped continuation ([OPEN-WORK.md](../OPEN-WORK.md), "Unexplained measurements and
unmeasured decisions"). Against `6ff8890a`, the twenty-four programs,
three builds a side, two sittings pooled, measured a geometric mean of
-0.17% (-0.12% and -0.21% in the sittings), no program slower with its
builds separated, Queens and Storage faster in the second sitting.

### What the leftover analysis moved

The leftover analysis, `bytecode::leftovers`, finds from a body's code
alone the scratch registers it may read before it writes them: what a
callee laid on its slots left there, or what the stack held before the
push ([compiler.md](compiler.md), "What a callee leaves", one pass over
bitsets, bounded by the construct limits of "Limits" there). The decoder
refuses image code that reads one, both lowerings check every body in a
debug build, and every trace of the stack nils them in each frame before
it traces the frame's words (`Heap::trace_stack`; [memory.md](memory.md),
"What the collector traces"). That is what lets neither a push nor a pop
write a scratch register: a push fills only its named locals, a pop
writes nothing ("Representation" above). The tree before it, `c6a126b3`,
filled them in both: a push every register above its header, a pop of a
callee laid on its sender's slots every word from the callee's header to
the sender's window end, the fill that kept those words from being traced
stale. A profile of that tree put the two fills at 1.6-1.8% of a run on
average and 3.0% at most (DeltaBlueTon80), the push's 0.7-0.8 points of
it and the pop's 0.9-1.0. The fill also kept image code from reading what
a callee left, the VM mirror among it; the decoder's refusal is that
guard now ([image-format.md](image-format.md), "Verification").

It was measured at `f2faa8f4` against `c6a126b3` on 2026-10-25, by the
protocol of "What contiguous arguments moved" above: three release builds
a side, the eight micro sources five rounds a sitting and the sixteen Are
We Fast Yet programs three, two sittings, every run printing its recorded
answer. `vm::run` is the same on both sides, 8,269 instructions apart from
the padding and a `0x318`-byte frame in all three builds; what changed is
in the functions it calls, the pushes, the pop and the collector's walk.
Means of each build's medians, pooled over both sittings (n = 10 rounds a
build for the micro sources, 6 for Are We Fast Yet), in ms:

| Source | `c6a126b3` | the analysis | change | `c6a126b3` a, b, c | the analysis a, b, c | builds |
|---|---|---|---|---|---|---|
| `bench_block.ns` | 7.235 | 7.156 | -1.1% | 7.225, 7.229, 7.249 | 7.184, 7.163, 7.122 | faster |
| `bench_loop.ns` | 2.126 | 2.145 | +0.9% | 2.123, 2.113, 2.143 | 2.139, 2.145, 2.150 | overlapping |
| `bench_megamorphic.ns` | 4.702 | 4.634 | -1.4% | 4.729, 4.694, 4.682 | 4.619, 4.635, 4.649 | faster |
| `bench_send.ns` | 3.308 | 3.226 | -2.5% | 3.307, 3.295, 3.322 | 3.204, 3.219, 3.255 | faster |
| `bench_new.ns` | 2.745 | 2.690 | -2.0% | 2.730, 2.744, 2.761 | 2.716, 2.675, 2.680 | faster |
| `bench_new2.ns` | 3.646 | 3.584 | -1.7% | 3.654, 3.658, 3.627 | 3.660, 3.545, 3.547 | overlapping |
| `bench_newarg.ns` | 3.689 | 3.645 | -1.2% | 3.674, 3.711, 3.682 | 3.705, 3.611, 3.619 | overlapping |
| `bench_do.ns` | 2.657 | 2.616 | -1.6% | 2.643, 2.653, 2.676 | 2.621, 2.613, 2.613 | faster |
| Bounce | 0.902 | 0.888 | -1.5% | 0.901, 0.897, 0.908 | 0.899, 0.875, 0.890 | overlapping |
| CD | 3451.8 | 3394.7 | -1.7% | 3454.6, 3460.9, 3439.8 | 3373.7, 3378.4, 3431.9 | faster |
| DeltaBlue | 619.28 | 609.22 | -1.6% | 621.51, 616.99, 619.35 | 608.61, 608.29, 610.75 | faster |
| DeltaBlueTon80 | 6.816 | 6.790 | -0.4% | 6.801, 6.812, 6.835 | 6.798, 6.758, 6.814 | overlapping |
| Havlak | 3252.6 | 3211.1 | -1.3% | 3250.1, 3256.6, 3251.0 | 3212.9, 3203.7, 3216.6 | faster |
| Json | 10.023 | 9.811 | -2.1% | 10.013, 9.989, 10.066 | 9.817, 9.779, 9.838 | faster |
| List | 0.933 | 0.915 | -2.0% | 0.927, 0.941, 0.932 | 0.930, 0.907, 0.908 | overlapping |
| Mandelbrot | 2763.9 | 2725.2 | -1.4% | 2719.3, 2847.4, 2724.9 | 2723.4, 2730.1, 2722.1 | overlapping |
| NBody | 1980.5 | 1976.5 | -0.2% | 1972.5, 1988.3, 1980.8 | 1970.3, 1975.8, 1983.3 | overlapping |
| Permute | 2.323 | 2.311 | -0.5% | 2.333, 2.306, 2.331 | 2.277, 2.328, 2.328 | overlapping |
| Queens | 1.973 | 1.949 | -1.2% | 1.975, 1.963, 1.980 | 1.961, 1.942, 1.943 | faster |
| Richards | 62.878 | 61.697 | -1.9% | 62.613, 62.915, 63.106 | 61.708, 61.696, 61.686 | faster |
| RichardsTon80 | 4.330 | 4.273 | -1.3% | 4.343, 4.302, 4.344 | 4.257, 4.287, 4.274 | faster |
| Sieve | 1.325 | 1.334 | +0.7% | 1.314, 1.319, 1.341 | 1.314, 1.359, 1.328 | overlapping |
| Storage | 1.402 | 1.372 | -2.2% | 1.393, 1.410, 1.405 | 1.373, 1.377, 1.365 | faster |
| Towers | 3.115 | 3.029 | -2.8% | 3.121, 3.124, 3.100 | 2.992, 3.020, 3.074 | faster |

**No program is slower with its builds separated, and the geometric mean
over the twenty-four is -1.34%** (-1.13% in the first sitting, -1.55% in
the second), **so the analysis is kept**, the rule it was built under.
Fourteen programs are faster with the builds separated, Towers by 2.8% and
`bench_send.ns` by 2.5%; ten overlap. Of the two above `c6a126b3`,
`bench_loop.ns` sends nothing -- an inlined `whileTrue:` over two slots,
which pushes and pops no frame and collects nothing -- and its three
functions are `c6a126b3`'s instruction for instruction, at addresses
moved by about `0x2840`, and its debug-information twins run it equally
fast: layout. Against the first stage of road step 7a′ with only the
value-answered send's step forgotten (`f0bf0d00`, which removes a cost
the first stage has too), the geometric mean is -0.09%.

**The analysis's one cost is in the collector, and only a deep stack
shows it.** A probe that recurses to 99,000 frames and allocates about
240 MB at the bottom, so that about 30 scavenges each meet the whole
stack, measured, with three builds a side:

- **+7.9% against `c6a126b3`, builds separated, with a pre-pass of its
  own** before a linear trace of the stack: it walked the frames twice,
  once to fill the sets not yet kept and once to nil, 10.1% of the run,
  at about 8-10 ns a frame;
- **+3.4% with one walk**, which nils a frame whose set is kept as it
  meets it and defers the others, keeps the set it looked up last and
  skips a frame whose every writable word the window above it covers.
  What was left was the walk itself: a variant that only read each
  frame's header words and nilled nothing measured the same;
- **+0.05%, the builds overlapping, with the nilling folded into the
  stack's trace** (`51f1050e`), which nils each frame and then traces
  it, so that each frame's header lines are loaded once a trace. At 1,000
  and 10 frames the same probe is -0.5% and -1.0%.

The twenty-four programs measured neutral at both steps: a geometric mean
of +0.00% for the one walk against `f2faa8f4`, and -0.05% for the fold
against the one walk, no program slower with its builds separated once
re-timed. **`Heap::trace_stack` stays out of line** (`#[inline(never)]`,
one instance per tracer): inlined, it grew `World::scavenge` from 1,528
to 2,763 instructions and changed what `World::collect` inlines, which
moved the send and return functions linked after them --
`pop_activation`, `resume_caller`, `polymorphic_hit` -- by `0x3380`, to
other offsets within their 4 KiB pages, and RichardsTon80, which spends
no sample in the collector, measured about 1.3% slower (+0.6% to +1.6%
in four timings); out of line it is level. That is placement, not code
the change runs, and a later growth of the collector's code can move those
functions again ([OPEN-WORK.md](../OPEN-WORK.md), "Unexplained
measurements and unmeasured decisions").

Under `NS_NEXT_STRESS_SCAVENGE`, which scavenges at nearly every
safepoint, the walk costs about what the scavenge's own trace of the
stack does: an optimized build with debug assertions runs a 99,000-frame
probe that allocates on the way down and up in 127 ms against
`c6a126b3`'s 88, 1.45x. Only the debug suite's stress runs pay it; a
release build has no stress mode.

What the measurement did not cover: one machine and operating system, not
idle; data layout, which neither `-C metadata` nor the block alignment
varies; the deep probes are one source, five runs a build, and the stress
figure comes from a build with debug assertions, so it includes their
checks.

### What `new` as compiled code moved

"`new` as compiled code" (a synthesized factory body, three initializer
bodies per mixin with the clause messages lowered inline, a body-less
declaration the mixin application it writes; [object-model.md](object-model.md),
"Instantiation") was measured whole against `master` (`f613cb49`, the tree
before it) on 2026-10-23, twice: at `1dc3a7bb`, the head before the
performance round, and at `9280bd91`, the head after it. The protocol is
that of "What the loop over locals moved": both trees exported with `git
archive` under the git-ignored `.temp/`, `cargo xtask shipped-code` in each
before its builds, three release builds a side with the pinned `RUSTFLAGS`
(`-C target-feature=+crt-static -C llvm-args=-align-all-functions=6`), the
second and third adding `-C metadata=buildb` and `-C metadata=buildc`, each
in its own `--target-dir`; all six binaries distinct, none importing
`VCRUNTIME140.dll`. Micro: the four `benchmarks/micro/*.ns` and
`bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` of "What
spec-conformant lookup moved", `ns-next bench <source> --iterations 30`,
five rounds a sitting, the six binaries rotating by one position from round
to round; every run printed its recorded answer. Are We Fast Yet: `ns-next
bench benchmarks/are-we-fast-yet --iterations N -- <Name>`, N = 3 for CD,
Havlak, Mandelbrot, NBody and DeltaBlue and 10 for the rest, three rounds
rotating; every run printed `<Name>: ok`.

**The first measurement found `new` slower.** At `1dc3a7bb` (two sittings)
all seven micro sources were slower than `master`: `bench_new.ns` +31.0%,
`bench_new2.ns` +30.1%, `bench_newarg.ns` +11.0%, and the send path
`bench_block.ns` +5.6%, `bench_loop.ns` +9.5%, `bench_send.ns` +5.5%,
`bench_megamorphic.ns` +3.9%; of the Are We Fast Yet programs Bounce,
Havlak and Json (+4.5% to +5.8%) and DeltaBlue and CD (+2% to +4.7%) were
slower, NBody and List about 3% faster, the rest level. `vm::run` was 8,331
instructions against 7,670 (one `vm::run`, no `vm::run::closure`, no
`run_stretch` in either), its stack frame 0x368 bytes in both. A bisection
with one pinned build per commit, and `xperf` profiles of `bench_new.ns`
(1,500 runs, debug-information builds, samples mapped onto `dumpbin /disasm`
per instruction), found these causes, each of which is a reason still true:

- **The header read in the lexical origin.** Whether a lexical send starts
  at the class object's own declaration scope or at its mixin was asked of
  the class object, whose header the read had to fetch and size
  (`Heap::pointer_words`), on every lexical send's cached path: about 8 of
  `bench_loop.ns`'s 9 points and 4.5 of `bench_send.ns`'s 5, and half of
  Bounce's, Json's, DeltaBlue's and Havlak's loss. The answer is now decided
  from the code that runs: `CompiledCode::is_declaration_code`, a derived
  flag on a body-less declaration's full initializer and every block created
  in it, set by `Mixin::mark_declaration_code` wherever such a mixin's bodies
  are attached (the compiler, the image's mixin decoding, the embedded
  shipped code's decoder), never on a wire and outside `CompiledCode`'s
  equality. `vm.rs` reads it off the running code beside `family_size` and
  hands it to `sends::lexical_origin`. Declaration code runs once per
  instantiation, so its lexical sends go uncached and every cached lexical
  send starts at its current class exactly as on `master`; a debug build
  asserts on every such send that the code's answer equals the
  class-object answer. A `super` send reads no flag: its S is the
  superclass of its current class for every code.
- **The factory activation's first two instructions.** A factory costs an
  activation, its push and pop, three dispatches, `InitializeInstance`'s
  `send` wrapper and a switch: about 19% of `bench_new.ns` (a probe that ran
  the old Rust path instead, unsound, measured 10% below `master` where the
  head was 9% above). `Mutator::push_sent`, used by `vm::deliver` only,
  does the factory's `AllocateInstance` and `InitializeInstance` at the push
  (`enter_factory_body`) and pushes the initializer above it, or, for a class
  that runs no code, writes the answer and starts the factory at its
  `Return`. The frame is kept, so reflection, the depth count, unwinding and
  stack traces see what they saw; no program code runs between the push and
  those two instructions, so the only difference is a skipped safepoint. The
  initializer is decided before the allocation, and any error returns the
  factory unexecuted at program counter 0 so the loop raises it in the
  body's order (`an_allocation_refused_is_raised_before_the_initializers_error`).
- **One class-object read per level.** `decide_instance_initializer`,
  `decide_superclass_initializer` and `enter_or_pass` read a class object's
  application, applied clause and superclass up to four times each, every
  read an out-of-line `class_field`, and `class_factory` read the
  metaclass twice (+10 points on `bench_new.ns` and `bench_new2.ns`, +14 on
  `bench_newarg.ns` at the commit that made a body-less declaration its
  application). `Heap::class_level` answers all three with one look at the
  object's words.
- **No `Rc<Mixin>` clone per level.** `push_initializer` found a body again
  through `initializer_mixin`, an `Rc<Mixin>` and an `Rc<MixinApplication>`
  clone per level, and the allocation cloned the application. `Step::Initializer`
  carries the body its decision found (`Step` keeps its size) and the
  allocation borrows the application.
- **Store-forwarding stalls on push and pop**, the dominant cost inside every
  activation push, `master` included. `Heap::push_activation` assembled the
  96-byte `StackEntry` on the stack around `Vec::push`'s growth call and
  copied it with 16-byte loads of fields just written by narrower stores
  (one instruction held about 9% of all samples), `pop_activation` moved
  the whole entry out the same way, and `enter_factory_body` moved the
  decided `Step` out of the callee's return slot so (3%). `push_activation`
  and `Mutator::push` are `#[inline(always)]` and write each header word
  straight into the frame the stack lends, with the growth path out of
  line, and `pop_activation` reads the header words it needs in place
  ("Representation").

Tried and dropped, measured with three builds a side: returning straight
through a caller whose next instruction is `Return`, no gain beyond noise;
carrying a pushed initializer through a first argument-free
`InitializeSuperclass`, 4 to 5 points slower on `bench_new*`; forcing
`decide_instance_initializer` and `enter_or_pass` inline, reserving the
entry before computing it and building it through `iter::once_with`, no
gain or slower.

**The final measurement**, `9280bd91` against `master`, one sitting. Means
of the five rounds' medians, in ms:

| Source | master a | master b | master c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| `bench_block.ns` | 7.8456 | 7.8856 | 7.8001 | 7.6099 | 7.6286 | 7.6464 |
| `bench_loop.ns` | 2.4248 | 2.4156 | 2.4033 | 2.3580 | 2.3522 | 2.3632 |
| `bench_megamorphic.ns` | 5.3309 | 5.3055 | 5.3144 | 5.2132 | 5.1882 | 5.2309 |
| `bench_send.ns` | 3.9661 | 3.8206 | 3.8093 | 3.5870 | 3.5808 | 3.6099 |
| `bench_new.ns` | 3.0107 | 3.0697 | 2.9801 | 2.9233 | 2.9494 | 2.9418 |
| `bench_new2.ns` | 3.9292 | 3.9043 | 3.9216 | 3.8281 | 3.8281 | 3.8478 |
| `bench_newarg.ns` | 4.7958 | 4.7971 | 4.8202 | 3.8363 | 3.8447 | 3.8293 |

Pooled (n = 15 rounds a side), in ms; "Rounds" compares every round,
"Build means" the three build means a side:

| Source | Head mean | master mean | Change | Head median | master median | Change | Rounds | Build means |
|---|---|---|---|---|---|---|---|---|
| `bench_block.ns` | 7.6283 | 7.8438 | -2.75% | 7.6295 | 7.8626 | -2.96% | separated | separated |
| `bench_loop.ns` | 2.3578 | 2.4146 | -2.35% | 2.3590 | 2.4169 | -2.39% | overlapping | separated |
| `bench_megamorphic.ns` | 5.2108 | 5.3169 | -2.00% | 5.1943 | 5.3220 | -2.40% | overlapping | separated |
| `bench_send.ns` | 3.5926 | 3.8653 | -7.06% | 3.5897 | 3.8102 | -5.79% | overlapping | separated |
| `bench_new.ns` | 2.9382 | 3.0202 | -2.72% | 2.9297 | 2.9887 | -1.97% | overlapping | separated |
| `bench_new2.ns` | 3.8346 | 3.9184 | -2.14% | 3.8319 | 3.9233 | -2.33% | overlapping | separated |
| `bench_newarg.ns` | 3.8368 | 4.8044 | -20.14% | 3.8345 | 4.8011 | -20.13% | separated | separated |

**All seven sources are faster than `master`**, every head build mean below
every master build mean (at `1dc3a7bb`: +3.9% to +31%). Are We Fast Yet,
per build the median of its three values, in ms:

| Benchmark | master a | master b | master c | Head a | Head b | Head c |
|---|---|---|---|---|---|---|
| Bounce | 0.9685 | 0.9842 | 0.9895 | 0.9491 | 0.9391 | 0.9449 |
| CD | 3700 | 3714 | 3684 | 3545 | 3523 | 3539 |
| DeltaBlue | 672 | 676.7 | 670.7 | 644.8 | 643.2 | 644.9 |
| DeltaBlueTon80 | 7.63 | 7.588 | 7.507 | 7.215 | 7.301 | 7.308 |
| Havlak | 3458 | 3455 | 3453 | 3372 | 3366 | 3369 |
| Json | 11.03 | 11.23 | 11.17 | 10.62 | 10.56 | 10.45 |
| List | 1.024 | 1.028 | 1.027 | 0.9995 | 0.9979 | 0.9969 |
| Mandelbrot | 3048 | 3058 | 3055 | 3021 | 2995 | 2978 |
| NBody | 2279 | 2296 | 2281 | 2206 | 2187 | 2206 |
| Permute | 2.631 | 2.565 | 2.642 | 2.487 | 2.433 | 2.441 |
| Queens | 2.204 | 2.116 | 2.154 | 2.03 | 2.031 | 2.041 |
| Richards | 70.86 | 70.66 | 70.16 | 66.34 | 66.24 | 66.67 |
| RichardsTon80 | 4.981 | 4.866 | 4.91 | 4.562 | 4.579 | 4.636 |
| Sieve | 1.335 | 1.325 | 1.375 | 1.372 | 1.333 | 1.366 |
| Storage | 1.527 | 1.52 | 1.532 | 1.469 | 1.496 | 1.479 |
| Towers | 3.429 | 3.383 | 3.408 | 3.266 | 3.252 | 3.233 |

Pooled, the median of each tree's nine values with their range, and whether
the nine values and the three per-build medians separate:

| Benchmark | master, ms | Range | Head, ms | Range | Change | Separated (9 / builds) |
|---|---|---|---|---|---|---|
| Bounce | 0.9842 | 0.938-1.048 | 0.9449 | 0.9254-0.9788 | -4.0% | no / yes |
| CD | 3,700 | 3,677-3,728 | 3,539 | 3,521-3,571 | -4.4% | yes / yes |
| DeltaBlue | 671.1 | 665.9-687.5 | 644.8 | 639.6-645.7 | -3.9% | yes / yes |
| DeltaBlueTon80 | 7.588 | 7.428-7.763 | 7.278 | 7.064-7.395 | -4.1% | yes / yes |
| Havlak | 3,457 | 3,442-3,496 | 3,369 | 3,347-3,382 | -2.5% | yes / yes |
| Json | 11.17 | 10.96-11.48 | 10.56 | 10.39-10.69 | -5.5% | yes / yes |
| List | 1.027 | 0.9999-1.073 | 0.9979 | 0.9771-1.042 | -2.8% | no / yes |
| Mandelbrot | 3,054 | 3,012-3,111 | 2,995 | 2,936-3,023 | -1.9% | no / yes |
| NBody | 2,284 | 2,274-2,302 | 2,206 | 2,176-2,221 | -3.4% | yes / yes |
| Permute | 2.631 | 2.535-2.675 | 2.441 | 2.392-2.527 | -7.2% | yes / yes |
| Queens | 2.154 | 2.068-2.234 | 2.031 | 1.985-2.09 | -5.7% | no / yes |
| Richards | 70.6 | 70.14-72.03 | 66.34 | 65.81-66.94 | -6.0% | yes / yes |
| RichardsTon80 | 4.91 | 4.785-5.001 | 4.579 | 4.522-4.654 | -6.7% | yes / yes |
| Sieve | 1.371 | 1.301-1.406 | 1.366 | 1.304-1.377 | -0.3% | no / no |
| Storage | 1.527 | 1.493-1.567 | 1.479 | 1.418-1.522 | -3.2% | no / yes |
| Towers | 3.408 | 3.32-3.53 | 3.252 | 3.141-3.336 | -4.6% | no / yes |

**Fifteen of sixteen are faster** with the per-build medians separated, and
Sieve is level (at `1dc3a7bb`: Bounce, Havlak and Json +4.5% to +5.8%,
DeltaBlue and CD +2% to +4.7%). `dumpbin /disasm` of a release build with
debug information (`CARGO_PROFILE_RELEASE_DEBUG=limited`, the pinned flags):
one `ns_runtime::vm::run`, no `vm::run::closure` and no `run_stretch`; `vm::run`
is **8,090 instructions** (8,331 at `1dc3a7bb`, 7,670 at `master`), its stack
frame 0x368 bytes as before.

What is still open: the factory's `FrameKind` is written as a byte and a word
into a stack slot that the merged push code reads back in sixteen bytes, a
store-forwarding stall that holds about 6% of `bench_new.ns`'s samples and
that removing would speed every push; and the extra word of `SendKey`
([dispatch.md](dispatch.md), "A send, step by step") is not isolated by a
measurement ([OPEN-WORK.md](../OPEN-WORK.md)).

What the measurement did not cover: the final measurement is one sitting,
one machine and operating system, not idle; data layout (symbol ids, object
offsets, dispatch ids), which `-C metadata` does not vary; the `master`
builds are those of the first measurement, built the same way the same day.
No micro source instantiates a class whose levels run no code (a factory
started at its `Return`), nor exercises `super` sends or body-less
declarations: their correctness is held by the goldens and the debug
cross-check, their speed is unmeasured.

### What the handler-search leaf moved

**Measured 2026-10-27, `67a10605` against `master` (`31970511`)**, by
the protocol of the `measuring-performance` skill: three release builds a
side with the pinned alignment, builds a, b and c varying the block
layout inside `vm::run`, the same three on both sides; five rounds in a
rotating order, two sittings; `bench --iterations 30` for the micro
sources, `--iterations 3` for CD, Havlak, Mandelbrot, NBody and DeltaBlue
and `--iterations 10` for the other Are We Fast Yet programs. Each cell
is the mean of a tree's medians over its builds, rounds and sittings, in
milliseconds; "builds" is the range of its three builds' means, and
"separated" says whether the two trees' ranges do not overlap. The
builds carried no debug information, so the check that the three
layouts of `vm::run` differ was not made; the change does not touch
`vm::run`.

| Source | master (ms) | master builds | branch (ms) | branch builds | Change | Builds separated |
|---|---|---|---|---|---|---|
| `bench_block` | 7.155 | 7.120–7.206 | 7.128 | 7.110–7.162 | -0.4% | no |
| `bench_loop` | 2.166 | 2.155–2.183 | 2.158 | 2.144–2.174 | -0.4% | no |
| `bench_megamorphic` | 4.670 | 4.665–4.678 | 4.688 | 4.680–4.698 | +0.4% | slower |
| `bench_send` | 3.226 | 3.213–3.251 | 3.221 | 3.198–3.250 | -0.2% | no |
| `bench_signal` | 49.985 | 49.469–50.372 | 21.012 | 20.897–21.145 | -58.0% | faster |
| `Bounce` | 0.902 | 0.900–0.905 | 0.903 | 0.894–0.916 | +0.1% | no |
| `CD` | 3383.721 | 3322.989–3433.578 | 3406.675 | 3372.417–3440.982 | +0.7% | no |
| `DeltaBlue` | 606.241 | 601.632–609.916 | 609.853 | 605.364–613.565 | +0.6% | no |
| `DeltaBlueTon80` | 6.861 | 6.825–6.898 | 6.875 | 6.861–6.899 | +0.2% | no |
| `Havlak` | 3191.588 | 3158.358–3226.197 | 3195.889 | 3174.248–3219.063 | +0.1% | no |
| `Json` | 9.852 | 9.829–9.864 | 9.891 | 9.887–9.898 | +0.4% | slower |
| `List` | 0.928 | 0.920–0.940 | 0.929 | 0.925–0.932 | +0.1% | no |
| `Mandelbrot` | 2696.295 | 2690.258–2699.594 | 2704.211 | 2696.012–2714.511 | +0.3% | no |
| `NBody` | 1973.949 | 1962.681–1986.873 | 1976.676 | 1967.654–1988.002 | +0.1% | no |
| `Permute` | 2.375 | 2.363–2.387 | 2.382 | 2.374–2.395 | +0.3% | no |
| `Queens` | 1.944 | 1.939–1.952 | 1.949 | 1.939–1.956 | +0.3% | no |
| `Richards` | 61.706 | 61.299–62.296 | 61.846 | 61.061–62.799 | +0.2% | no |
| `RichardsTon80` | 4.274 | 4.230–4.326 | 4.329 | 4.317–4.338 | +1.3% | no |
| `Sieve` | 1.359 | 1.351–1.373 | 1.365 | 1.364–1.365 | +0.4% | no |
| `Storage` | 1.407 | 1.397–1.415 | 1.411 | 1.403–1.421 | +0.3% | no |
| `Towers` | 3.017 | 2.972–3.047 | 3.023 | 2.995–3.044 | +0.2% | no |

The geometric mean over the twenty-one sources is -3.8%, all of it
`bench_signal.ns`'s: a signal caught fifty activations down takes 58%
less time, since the search no longer reifies the activations it passes
-- a frame object for every activation it passed before, made by its
first walk and found again by its second
(`a_handler_search_reifies_no_frame_it_passes`). Every other source is
level. `bench_megamorphic.ns` and Json come out +0.4% with their pooled
ranges apart, but within either sitting their builds overlap the other
tree's: the separation is the two sittings' levels pooled, not the
change, and neither runs the handler search in its loop (Json signals
only on a parse error).

### What Newspeak loop bodies moved

**Measured 2026-10-27, `ab0bff29` against `master` (`be211fe8`)**, by
the protocol of the `measuring-performance` skill: three release builds a
side with the pinned alignment, builds a, b and c varying the block
layout inside `vm::run`, the same three on both sides; five rounds in a
rotating order, two sittings; `bench --iterations 30` for the micro
sources and `bench_do.ns`, `--iterations 3` for CD, Havlak, Mandelbrot,
NBody and DeltaBlue and `--iterations 10` for the other Are We Fast Yet
programs. `bench_do.ns` is spelled as "What spec-conformant lookup moved"
describes it, today's syntax, with `{1. 2. … 10}` and `= Object new`;
`bench_while.ns` is in the tree and ran under both. Each cell is the mean
of a tree's medians over its builds, rounds and sittings, in
milliseconds; "builds" is the range of its three builds' means, and
"separated" says whether the two trees' ranges do not overlap.

| Source | master (ms) | master builds | branch (ms) | branch builds | Change | Builds separated |
|---|---|---|---|---|---|---|
| `bench_block` | 7.220 | 7.207–7.234 | 7.157 | 7.119–7.192 | -0.9% | faster |
| `bench_loop` | 2.171 | 2.159–2.188 | 2.157 | 2.146–2.179 | -0.7% | no |
| `bench_megamorphic` | 4.677 | 4.649–4.695 | 4.671 | 4.658–4.678 | -0.1% | no |
| `bench_send` | 3.234 | 3.220–3.252 | 3.240 | 3.228–3.248 | +0.2% | no |
| `bench_signal` | 21.037 | 20.878–21.232 | 21.716 | 21.657–21.834 | +3.2% | slower |
| `bench_while` | 33.984 | 33.713–34.400 | 50.107 | 50.022–50.166 | +47.4% | slower |
| `bench_do` | 2.638 | 2.636–2.643 | 4.242 | 4.236–4.246 | +60.8% | slower |
| `Bounce` | 0.896 | 0.895–0.897 | 1.051 | 1.044–1.055 | +17.3% | slower |
| `CD` | 3386.693 | 3339.742–3410.251 | 3496.958 | 3460.322–3515.720 | +3.3% | slower |
| `DeltaBlue` | 607.833 | 602.170–610.753 | 607.400 | 603.153–610.745 | -0.1% | no |
| `DeltaBlueTon80` | 6.886 | 6.837–6.933 | 6.913 | 6.861–6.960 | +0.4% | no |
| `Havlak` | 3191.442 | 3170.070–3218.323 | 3201.297 | 3177.402–3222.702 | +0.3% | no |
| `Json` | 9.934 | 9.870–9.972 | 9.839 | 9.799–9.887 | -1.0% | no |
| `List` | 0.925 | 0.922–0.926 | 0.923 | 0.920–0.926 | -0.1% | no |
| `Mandelbrot` | 2702.151 | 2695.867–2712.388 | 2717.818 | 2713.844–2720.059 | +0.6% | slower |
| `NBody` | 1975.922 | 1963.546–1988.244 | 2002.760 | 1999.071–2009.600 | +1.4% | slower |
| `Permute` | 2.396 | 2.382–2.414 | 2.383 | 2.374–2.393 | -0.6% | no |
| `Queens` | 1.973 | 1.963–1.980 | 1.971 | 1.959–1.987 | -0.1% | no |
| `Richards` | 61.626 | 61.072–62.357 | 61.837 | 61.557–62.238 | +0.3% | no |
| `RichardsTon80` | 4.349 | 4.326–4.364 | 4.335 | 4.319–4.356 | -0.3% | no |
| `Sieve` | 1.373 | 1.364–1.379 | 1.375 | 1.365–1.381 | +0.1% | no |
| `Storage` | 1.423 | 1.421–1.427 | 1.424 | 1.414–1.438 | +0.1% | no |
| `Towers` | 3.070 | 3.043–3.100 | 3.074 | 3.035–3.101 | +0.1% | no |

The geometric mean over the twenty-three sources is +4.8%, nearly all of
it the programs that walk an `Array` with `do:` or loop over blocks held
in variables: `bench_do.ns` +61%, `bench_while.ns` +47%, Bounce +17%,
CD +3.3%, `bench_signal.ns` +3.2% and NBody +1.4%; the programs that
loop only through inlined literals are level. It is the price the
decision to write these bodies in Newspeak named before the JIT ("Kernel
method bodies forward" in [core-library.md](core-library.md), and its
exception): an element now costs a cached `at:` and a cached `value:`
send and about ten instructions of the loop around them, some 32 ns more
than the leaf's one step in `bench_do.ns`, and a turn of a loop that is
not inlined a cached `value` to each block, about 39 ns more in
`bench_while.ns`. Nothing in the code was found to be cheaper: an interim
measurement (one pinned build a side, three rounds) of the two
alternatives a kernel method has -- the element read through the
`collection:at:ifFail:` leaf instead of `at:`, and also the blocks run
through the `blockValue:ifFail:` and `block:value:ifFail:` leaves instead
of `value` and `value:` -- measured both slower than the sends,
`bench_do.ns` 4.77 and 5.14 ms against 4.25, Bounce 1.08 and 1.14 ms
against 1.05, since a hit in a send's inline cache reaches the leaf for
less than a mirror send's own dispatch. The cost is the baseline JIT's to
recover, by inlining the loop's sends into it (road step 8,
[OPEN-WORK.md](../OPEN-WORK.md)).

### What the baseline JIT's foundation moved

**Measured 2026-10-28, `56c6e8ac` against `master` (`5d970d1a`)**, by
the protocol of the `measuring-performance` skill: three release builds a
side with the pinned alignment, builds a, b and c varying the block
layout inside `vm::run` (checked in a `dumpbin /disasm` of each with debug
information: `vm::run` spans 40,118, 43,190 and 46,198 bytes on the
branch, built at `90fb6ae3` -- `56c6e8ac` changed nothing `vm::run`
inlines -- and 44,543, 48,063 and 50,751 on `master`, and neither tree
has a `run_stretch` or a `vm::run` closure of its own); five rounds in a
rotating order, two sittings; `bench --iterations 30` for the micro
sources, `--iterations 3` for CD, Havlak, Mandelbrot, NBody and DeltaBlue
and `--iterations 10` for the other Are We Fast Yet programs. The same
three branch binaries ran a third time with `NS_NEXT_NO_JIT=1`, which
reserves no zone and refuses every body at its first entry, so that what
they measure is the push's added check alone. Each cell is the mean of a
tree's medians over its builds, rounds and sittings, in milliseconds;
"builds" is the range of its three builds' means, and "separated" says
whether the two trees' ranges do not overlap.

| Source | master (ms) | master builds | branch (ms) | branch builds | Change | Builds separated |
|---|---|---|---|---|---|---|
| `CD` | 3518.433 | 3484.347–3541.613 | 3490.952 | 3471.058–3512.047 | -0.8% | no |
| `Havlak` | 3204.296 | 3185.281–3226.334 | 3201.647 | 3195.178–3206.840 | -0.1% | no |
| `Mandelbrot` | 2730.548 | 2729.757–2730.961 | 2709.835 | 2706.465–2714.718 | -0.8% | faster |
| `NBody` | 2011.961 | 2006.366–2022.940 | 2009.661 | 2008.259–2011.497 | -0.1% | no |
| `DeltaBlue` | 608.524 | 604.286–613.027 | 607.038 | 602.346–611.931 | -0.2% | no |
| `bench_block` | 7.138 | 7.052–7.204 | 6.990 | 6.982–6.995 | -2.1% | faster |
| `bench_loop` | 2.178 | 2.161–2.193 | 2.115 | 2.113–2.116 | -2.9% | faster |
| `bench_megamorphic` | 4.645 | 4.633–4.657 | 4.642 | 4.626–4.654 | -0.1% | no |
| `bench_send` | 3.211 | 3.209–3.213 | 3.209 | 3.205–3.215 | -0.1% | no |
| `bench_signal` | 21.674 | 21.537–21.948 | 21.508 | 21.397–21.644 | -0.8% | no |
| `bench_while` | 50.326 | 50.223–50.409 | 50.382 | 50.265–50.464 | +0.1% | no |
| `Bounce` | 1.047 | 1.046–1.049 | 1.052 | 1.049–1.056 | +0.5% | no |
| `DeltaBlueTon80` | 6.817 | 6.746–6.873 | 6.847 | 6.803–6.905 | +0.4% | no |
| `Json` | 9.777 | 9.721–9.846 | 9.805 | 9.783–9.820 | +0.3% | no |
| `List` | 0.926 | 0.925–0.928 | 0.920 | 0.918–0.921 | -0.7% | faster |
| `Permute` | 2.352 | 2.342–2.364 | 2.348 | 2.336–2.356 | -0.2% | no |
| `Queens` | 1.968 | 1.965–1.972 | 1.961 | 1.949–1.969 | -0.4% | no |
| `Richards` | 61.763 | 61.580–61.857 | 61.186 | 60.971–61.362 | -0.9% | faster |
| `RichardsTon80` | 4.210 | 4.199–4.220 | 4.177 | 4.140–4.220 | -0.8% | no |
| `Sieve` | 1.335 | 1.326–1.347 | 1.344 | 1.341–1.349 | +0.6% | no |
| `Storage` | 1.393 | 1.383–1.402 | 1.397 | 1.391–1.405 | +0.4% | no |
| `Towers` | 3.059 | 3.042–3.077 | 3.096 | 3.032–3.222 | +1.2% | no |

No program is slower than `master` with its builds separated; five are
faster -- `bench_block`, `bench_loop`, List, Mandelbrot and Richards --
and the geometric mean over the twenty-two sources is -0.34%. With the
JIT off the same binaries measure -0.55%:

| Source | master (ms) | master builds | `NS_NEXT_NO_JIT=1` (ms) | its builds | Change | Builds separated |
|---|---|---|---|---|---|---|
| `CD` | 3518.433 | 3484.347–3541.613 | 3483.135 | 3472.269–3490.735 | -1.0% | no |
| `Havlak` | 3204.296 | 3185.281–3226.334 | 3199.104 | 3197.860–3200.273 | -0.2% | no |
| `Mandelbrot` | 2730.548 | 2729.757–2730.961 | 2716.580 | 2699.172–2728.004 | -0.5% | faster |
| `NBody` | 2011.961 | 2006.366–2022.940 | 2010.158 | 2005.578–2018.778 | -0.1% | no |
| `DeltaBlue` | 608.524 | 604.286–613.027 | 606.810 | 603.297–608.711 | -0.3% | no |
| `bench_block` | 7.138 | 7.052–7.204 | 7.037 | 6.995–7.069 | -1.4% | no |
| `bench_loop` | 2.178 | 2.161–2.193 | 2.120 | 2.117–2.124 | -2.7% | faster |
| `bench_megamorphic` | 4.645 | 4.633–4.657 | 4.641 | 4.599–4.673 | -0.1% | no |
| `bench_send` | 3.211 | 3.209–3.213 | 3.200 | 3.173–3.226 | -0.3% | no |
| `bench_signal` | 21.674 | 21.537–21.948 | 21.524 | 21.425–21.597 | -0.7% | no |
| `bench_while` | 50.326 | 50.223–50.409 | 50.261 | 50.171–50.344 | -0.1% | no |
| `Bounce` | 1.047 | 1.046–1.049 | 1.049 | 1.045–1.054 | +0.2% | no |
| `DeltaBlueTon80` | 6.817 | 6.746–6.873 | 6.810 | 6.796–6.830 | -0.1% | no |
| `Json` | 9.777 | 9.721–9.846 | 9.779 | 9.766–9.798 | +0.0% | no |
| `List` | 0.926 | 0.925–0.928 | 0.918 | 0.917–0.920 | -0.8% | faster |
| `Permute` | 2.352 | 2.342–2.364 | 2.335 | 2.321–2.353 | -0.7% | no |
| `Queens` | 1.968 | 1.965–1.972 | 1.957 | 1.951–1.961 | -0.6% | faster |
| `Richards` | 61.763 | 61.580–61.857 | 61.220 | 61.007–61.395 | -0.9% | faster |
| `RichardsTon80` | 4.210 | 4.199–4.220 | 4.172 | 4.147–4.207 | -0.9% | no |
| `Sieve` | 1.335 | 1.326–1.347 | 1.337 | 1.330–1.342 | +0.1% | no |
| `Storage` | 1.393 | 1.383–1.402 | 1.386 | 1.377–1.391 | -0.5% | no |
| `Towers` | 3.059 | 3.042–3.077 | 3.046 | 3.034–3.068 | -0.4% | no |

So the push's check, one load and one compare of the body's `JitSlot`,
costs nothing measurable, and nor does the JIT on: the few bodies whose
code can loop before their first exit run their loops in machine code,
and the rest are never entered. `bench_loop` and `bench_block` are
nearly as much faster with the JIT off, so they are the run loop's new
layout, not machine code.

**Entering every body cost `bench_block` 31.5%.** The first measurement of this
branch, before `56c6e8ac` (one round, three builds a side), entered every
body whose first instruction had a template. Such a body -- `tick: x`'s
`^x + 1`, an `IntegerBinary` and a return -- runs one or two templates,
exits at its return or first send, and the interpreter runs the rest, so
every push of it paid the cold call, the entry block, the entry stub and
the exit for nothing: `bench_block` +31.5%, `bench_send` +21.7%,
`bench_megamorphic` +18.9%, Sieve +13.8%, `bench_signal` +10.3%, the
geometric mean over seventeen sources +7.3%, while the same binaries with
`NS_NEXT_NO_JIT=1` measured -1.3%. `bench_send`'s 0.7 ms over its
50,000 entries of `tick:` is about 14 ns an entry and exit. Since then a
body is entered only when its code can take a backward jump before its
first exit ("The baseline JIT's foundation"). An interim measurement
of `56c6e8ac` in the same form (one round of the seventeen sources that
take milliseconds, three builds a side) found no program slower with its
builds separated -- `bench_block` -0.6%, `bench_send` -1.7%,
`bench_megamorphic` -2.3%, Sieve +1.3%, `bench_signal` -1.0% -- and the
geometric mean -1.0%; the final measurement above confirms it.

**The machine words moved.** Measured 2026-10-28, `39a9113b` -- the
safepoint request in the machine words, the entry block of thirty-two
words and the leaf call's stack pointer in it -- against `cc534eac`, by
the same protocol: three builds a side (`vm::run` spans 40,118, 43,190
and 46,198 bytes at `cc534eac`, 39,990, 43,062 and 46,262 at
`39a9113b`, no `run_stretch` or closure of its own in either), five
rounds in a rotating order over two sittings, `bench_loop`, `bench_send`
and the Are We Fast Yet programs.

| Source | `cc534eac` (ms) | its builds | `39a9113b` (ms) | its builds | Change | Builds separated |
|---|---|---|---|---|---|---|
| `bench_loop` | 2.108 | 2.104–2.111 | 2.073 | 2.071–2.075 | -1.7% | faster |
| `bench_send` | 3.235 | 3.226–3.243 | 3.184 | 3.182–3.185 | -1.6% | faster |
| `CD` | 3502.313 | 3493.504–3512.912 | 3493.578 | 3459.110–3520.224 | -0.2% | no |
| `Havlak` | 3209.688 | 3203.428–3215.764 | 3192.161 | 3181.536–3198.648 | -0.5% | faster |
| `Mandelbrot` | 2721.996 | 2701.464–2738.962 | 2698.567 | 2678.484–2721.228 | -0.9% | no |
| `NBody` | 2012.372 | 2009.262–2017.896 | 1989.395 | 1983.920–1999.260 | -1.1% | faster |
| `DeltaBlue` | 607.923 | 605.425–612.200 | 604.966 | 598.330–609.102 | -0.5% | no |
| `Bounce` | 1.057 | 1.050–1.066 | 1.035 | 1.034–1.037 | -2.0% | faster |
| `DeltaBlueTon80` | 6.958 | 6.914–6.982 | 6.877 | 6.832–6.943 | -1.2% | no |
| `Json` | 9.922 | 9.870–9.964 | 9.980 | 9.786–10.353 | +0.6% | no |
| `List` | 0.919 | 0.916–0.923 | 0.912 | 0.900–0.921 | -0.8% | no |
| `Permute` | 2.373 | 2.352–2.394 | 2.371 | 2.368–2.373 | -0.1% | no |
| `Queens` | 1.946 | 1.934–1.953 | 1.931 | 1.917–1.948 | -0.8% | no |
| `Richards` | 61.862 | 61.668–62.009 | 61.150 | 60.589–61.482 | -1.2% | faster |
| `RichardsTon80` | 4.260 | 4.197–4.300 | 4.252 | 4.234–4.263 | -0.2% | no |
| `Sieve` | 1.360 | 1.357–1.365 | 1.373 | 1.349–1.387 | +1.0% | no |
| `Storage` | 1.404 | 1.393–1.416 | 1.404 | 1.386–1.434 | +0.1% | no |
| `Towers` | 3.025 | 2.998–3.070 | 3.035 | 2.989–3.071 | +0.4% | no |

No program is slower with its builds separated; six are faster --
`bench_loop`, `bench_send`, Bounce, Havlak, NBody and Richards -- and the geometric mean over the eighteen sources is -0.60%. The
poll's extra load costs nothing measurable; what is faster is the run
loop's new layout, since nothing machine code runs in these programs
changed.

What it does not cover: no benchmark program spends its time in a loop
the templates cover whole, so the measurement shows that the foundation
costs nothing, not what machine code gains; that is stage 3's and later
stages' to measure, once sends and returns run in machine code. The
counting loop of `goldens/runtime/jit/counting_loop.ns` is the one such
loop in the tree, and it is a golden, not a benchmark.

## The trampoline

Sends do not recurse on the Rust stack. Every running activation lives on the
activation stack, and the run loop ("The VM" above) is the only place a running
computation lives, and nothing re-enters it from Rust ("No re-entry from Rust"
below). This was step 1b of Milestone 4, and it
landed the way the stack did: against a green suite, with its own
before-and-after measurement ("What the trampoline moved" above).

### Deciding, then running

**The trampoline falls out of the stack rather than driving it.** The
activation stack is what a trampolined run loop needs, so replacing the
Rust-stack recursion with a loop that pushes is the same change viewed from
the other side. Getting there split every send so that it *decides* without
*executing*: lookup, `doesNotUnderstand:`, native dispatch and the
nested-class accessor stay in `interpreter/` (`inline_cache::send_public`,
`send_lexical` and `send_super`, over the lookups in `resolve.rs`), unchanged
in what they produce, but they answer a `Step` — a value, a method to
activate, or a block to activate and what to do with its value — instead of
activating it. That keeps `vm.rs` free of dispatch logic: what moved into the
loop is the act of running a body, never the decision of which body. A
send's decision reads its arguments in the caller's registers, through an
argument window, and a push copies them from there into the callee's
window, so a send allocates no vector of its own ("No re-entry from Rust"
below).

**Instantiation decides the same way, and runs as code.** A factory send is
an ordinary send: the lookup finds the metaclass's factory entry and `act`
answers `Step::Factory`, the entry's body to activate on the class object.
That body and the initializers it reaches run in the loop like any method,
and their instructions decide without executing: `AllocateInstance` and
`CheckAppliedClause` answer a value or an error, and `InitializeInstance`,
`InitializeSuperclass` and `InitializeAppliedLevel` a `Step` -- the next
level's initializer to activate, or the instance when that level runs
nothing -- each from one call into `interpreter/instantiate.rs`
([object-model.md](object-model.md), "Instantiation"). A factory's first
two of them, between which nothing can run, are carried out where the
factory is pushed (`Mutator::enter_factory_body`), deciding and running
exactly as the loop would. So no state machine
carries an instantiation between its bodies: the activation stack is its
whole state, as it is a method's.

Three things the trampoline settles on its own:

- **`Closure value` is not a callback.** It is a native that answers the closure
  as a `Step::Block`; activating a closure is what the loop already does, so no
  Rust frame sits in the middle, and nothing was rewritten in Newspeak.
- **Nor are the control-flow steps** (decided 2026-09-12).
  `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:` and
  `or:` on a Boolean answer the block to activate, exactly as `value` does,
  and its value is the send's (`Continuation::Deliver`) -- `True`'s and
  `False`'s `^<parameter> value` quick methods answer it, with no leaf
  ([core-library.md](core-library.md), "`True` and `False` are quick
  methods"). `whileTrue:` and `whileFalse:` need no step: `Closure`'s
  methods are the loop the compiler inlines, `^[self value] whileTrue:
  [body value]`, whose condition and body are ordinary `value` sends that
  deliver to it ([core-library.md](core-library.md), "Real bodies for
  control flow and `do:`"). This
  is Primordial Soup's model with one activation
  fewer: there `True>>ifTrue: then = (^then value)` is a Newspeak method whose
  `value` is primitive 156, `ActivateClosure`, which pushes a frame and returns
  to `Interpret` (`vm/interpreter.cc`), so nothing in that VM ever re-enters the
  interpreter from C. What decided it is the depth rule below: a callback native
  costs native stack per level under the reference lowering only, growing with
  the static nesting of `and:` and `while` blocks, so no fixed native stack
  would guarantee that the count trips before the native stack under both
  lowerings. `Array>>do:` is a Newspeak loop of `value:` sends as well.
- **Recursion depth is a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded
  artifact. The limit counts every activation except a block activation that a
  builtin control-flow step, or the reference lowering's loop, performs for a
  `SendInlinable`, the send only the
  reference lowering emits where the optimized one inlines. That makes the
  count agree under both lowerings at every point of every program, so the
  reference lowering checks an overflow too ([compiler.md](compiler.md), "How
  the limit counts"). `SendInlinable` and the counter arrived together.
  Measured on Windows's 1 MB main-thread stack before the trampoline, the
  reachable recursion depth was 818 inside an `ifFalse:` block (release
  build), and 307 through `do:` — below the 1000 that Newspeak's own
  `KernelWeakTests.ns` (`deepStack: 1000`) expects. The golden
  `goldens/runtime/depth-limit/thousand_deep_recursion_runs.ns` runs exactly that
  recursion, under both lowerings.

### The depth limit

Decided 2026-09-12. Once sends and control flow no longer recurse
and nothing re-enters the run loop from Rust ("No re-entry from Rust" below),
a recursion runs out of one resource only, the memory its activations take,
and one counted bound limits it:

| Bound | Counts | Value | Error text |
|---|---|---|---|
| L | activations, by the rule in [compiler.md](compiler.md), "How the limit counts" | 100,000 | `stack overflow: recursion too deep` |

It is `MAX_ACTIVATION_DEPTH` in `crates/ns-runtime/src/interpreter/mod.rs`.
L bounds memory: an activation is a frame of words -- its header and its
registers -- in one reservation of the heap's, so L can be far larger than
any native stack would allow. It
bounds it up to a factor: above each counted activation sit at most as many
exempt ones as blocks nest — each exempt block was created by the activation
below it, so a chain of them descends the lexical nesting — and blocks nest at
most 128 deep, the parser's ceiling in source and the decoder's, derived from
it, in an artifact ([image-format.md](image-format.md), "Verification"). So the
stack holds at most 129 × (L + `DEPTH_RESERVE`) frames, the reserve below
included, 13.0 million, whose headers alone take about 1.36 GB
(`frame_header::COUNT`, 13 words, 104 bytes, each, in every build), before
their registers, a suspended activation's frame object counted in its frame's
place; the reservation the frames lie in is the net beneath L (below). Only
code that carries `SendInlinable` gets there — the reference lowering's, or an
artifact's — recursing through 128 nested control-flow blocks at every level;
the optimized lowering never emits it, so under it the stack holds at most L
frames. The limit agrees under both lowerings, because the control-flow
natives, instantiation and the nested-class accessor are all trampoline
steps, and the loops and `do:` are Newspeak: no recursion costs native stack
under one lowering and not the other.
The error text does not name its number, as Lua's, CPython's, V8's and Nova's
do not, so that changing the value later changes no recorded answer. It is
signalled as an `Error` like every other Newspeak error, so `on: Error do:`
catches it, and an uncaught one ends the run as before.

**The reservation is a net beneath L.** A body may have up to 65,535
registers, so the activation count does not bound the words the frames
take, and a push the stack's reservation cannot hold -- 16 GiB on a 64-bit
target, or what its fallback reserved ("Representation" above) -- is
refused with L's error too, whose text names no number. Taking a frame is
one comparison against the committed end, as Soup's
`checked_stack_limit_` is, committing more on its cold path. The net keeps
a reserve as L does: the last 1/16th of the reservation
(`RESERVE_FRACTION`, `heap/frames.rs`; 1 GiB of 16) is refused to an
ordinary push and left to the pushes made while the depth reserve is held
(`Heap::push_reserved_activation`, which a push tries
(`Mutator::push_into_the_stack_reserve`) when an ordinary push is refused and the reserve is held, or is being taken by
the push of the kernel's `doesNotUnderstand:`), and an error raised with
the stack that close to its end takes the depth reserve for its `signal`
("Who takes it" below). Without it the `signal` of the overflow is itself
refused and the run ends with the error instead of the handler running;
the `signal`, its handler search and a handler of a few frames need
between 256 and 1,024 words of it
(`a_recursion_the_reservation_cannot_hold_is_a_catchable_stack_overflow`,
in a 1 MiB reservation). A push the whole reservation cannot hold while
the reserve is held ends the run as an overrun of the reserve does
("What an overrun ends with" below).

**The limit keeps a reserve for signalling it** (decided with the user on
2026-09-25): `DEPTH_RESERVE`, 1,000 activations, as CPython allows a little
headroom above its recursion limit for the `RecursionError`'s handler and
HotSpot unprotects its stack's yellow zone for the `StackOverflowError`'s.
`signal`, the handler search and the handler are activations too, so
without it an error raised a few activations below L — L's own included —
could only end the run as L, uncatchable and under the wrong name.

- **Who takes it.** `unwind::start_signalling`, signalling any VM error
  within `DEPTH_RESERVE` counted activations of L, and `act_handler_method`,
  activating the kernel's own `Object>>doesNotUnderstand:` there
  (`Mutator::take_depth_reserve_for_does_not_understand`) — §5.5 makes a
  message nothing understood the VM's error as much as any other. Both
  also take it near the end of the reservation, with the stack's top
  within the reservation's reserve of the room ordinary pushes have
  (`Heap::stack_is_within_the_reserve_of_its_limit`; the net above). A
  program's own `signal` takes none: one signalled at L is refused as any
  push is, and the refusal is L's error, signalled and catchable. An error
  far from L takes nothing, so its handler meets L where any code does.
- **What holds it.** `Mutator.depth_reserve`, the `FrameReference` of the
  activation the error arose in, counts as held while that frame is running
  (`Heap::frame_is_running`, suspended included) and the activation directly
  above it is the error's `signal`, marked by
  `Continuation::VmErrorSignalled`, or the kernel's `doesNotUnderstand:`
  (`Mutator::holds_the_depth_reserve`): above a married frame
  `Heap::activation_above`, above a suspended one the activation a walk down
  from the top finds returning to it (`WalkedActivation`, "Suspending and
  resuming" above). A frame and not a stack offset: a
  frame is divorced on its activation's pop and married to nothing again,
  where an offset is reused, and a later signal at the same offset — possibly
  far from L — must not inherit the reserve
  (`interpreter::unwind`'s `a_later_signal_at_the_same_index_does_not_inherit_the_reserve`).
  `depth_reserve_held` forgets a reserve it finds no longer held, so the pop
  that ended it pays nothing, and `Interpreter::run_from_host` releases it
  when a host's run ends.
- **What it allows.** `Mutator::push`'s check is the one compare it always
  was; only a counted push at L reaches the cold `push_past_the_limit`,
  which refuses it with L's error unless the reserve is held, and allows it
  up to L + `DEPTH_RESERVE` if it is.
- **What an overrun ends with.** A push past the reserve answers
  `Signal::Terminate`, so the run ends at once and uncatchably, running the
  pending `ensure:` blocks on the way, with the error
  `Mutator::overrun_of_the_reserve` picks by walking down from the top of the
  stack and on into suspended frames, once per run. If the holder nearest the
  top is still in the signalling machinery's hands — its exception still
  searching for a handler, after `signal` or a `pass` (`Exception`'s
  `searching` slot), or running its default action (`reachedDefault`), or the
  kernel's `doesNotUnderstand:` still building its exception — the run ends
  with that error, as it would have had there been room to signal it; an error
  the search itself raised, a `handles:` nothing understands, belongs to the
  search it arose in, so the walk reports the outermost of a run of such
  holders. If a handler, or anything run once one was found, overran it, the
  run ends with L's own error, "stack overflow: recursion too deep". A
  `signal` push that the used-up reserve refuses ends the run with the error
  being signalled, the same way.

`goldens/runtime/depth-limit/the_depth_limit_is_caught`,
`an_error_just_below_the_depth_limit_is_caught`,
`a_message_not_understood_just_below_the_depth_limit_is_caught`,
`recursion_through_on_do_counts_alike` and
`a_handler_overrunning_the_reserve_ends_the_run` pin it from source, and
`crates/ns-runtime/tests/exceptions.rs` the uncaught cases at L − 3, L − 1,
L and L + 1 and the overruns of the machinery, a `pass`'s included.

- **L is checked on every push.** `Mutator::push_placed` is the one
  function every activation the runtime pushes goes through, and
  `Mutator::push` calls it. Its two placements, `Mutator::push_above` and
  `Mutator::push_on_slots`, each refuse a counted push when
  `Heap::counted_depth` has reached the limit. The heap keeps that count as a
  running total on push and pop, so an error path — which pops through
  `interpreter::unwind` like any other — leaves it exact; suspending and
  resuming leave it alone, since a suspended activation counts all along.
  Whether an activation counts is recorded in its frame's flags word. Only
  the block arm of `Mutator::push_step` ever records an exemption; a method, and every
  body an instantiation or a nested-class accessor runs — a factory, an
  initializer level, a superclass clause — are counted by construction.
- **The exemption is marked by the arguments.** A control-flow step's block —
  one a Boolean receiver's `^<parameter> value` quick method answers — and
  the failure
  block a sent mirror send activates when its leaf declines
  ([compiler.md](compiler.md), "Mirror sends"), carry
  `StepArguments::ControlFlow` — none has arguments, and only those build
  that variant —, and the block a `value` send activates carries
  `StepArguments::Send`, its send's own arguments; `push_step` exempts
  either when the send
  was a `SendInlinable` and the block's `captured_frame` is the reified frame
  of the activation that sent it ([compiler.md](compiler.md), "How the limit
  counts"). For a `SendInlinable`, only the reference lowering's loop answers
  a block with the send's own arguments: the `value` it sends each turn to
  its condition and body. `Mutator::push_continuation` still takes the
  flag of the activation a continuation replaces, which the loop forwards,
  but no continuation now replaces an exempt one
  ([OPEN-WORK.md](../OPEN-WORK.md)). The
  mark rides on the arguments rather than in a field of its own
  because the field measured 4–8% slower ("What the trampoline moved" above);
  so a future native that activates a block with no arguments for any other
  reason must not build `ControlFlow`. `do:`'s block is sent `value:` by
  `Array>>do:`'s loop, an ordinary send, so it is always counted. A loop
  receiver or body that is no block literal is sent `value` by
  `Closure>>whileTrue:`'s loop, a `SendPublic` under both lowerings, and
  counted, as is the activation of `Closure>>whileTrue:` itself: a
  recursion through such a loop takes three counted activations a level,
  and overflows at about a third of the limit. `&` and `|` share `and:`'s and `or:`'s
  native and so carry the mark too, which is harmless: no lowering emits a
  `SendInlinable` for them, since `control_flow::recognize` accepts only the
  eight inlined selectors, and a crafted artifact that marks one meets the
  captured-frame condition like any other.
- **`ns-next` runs every command on a thread it spawns** with an explicit
  stack (`std::thread::Builder`, `INTERPRETER_STACK_BYTES` in
  `crates/ns-cli/src/main.rs`; `ns-next-analyzer`, which runs the same front
  end and compiler, does the same with `ANALYZER_STACK_BYTES`): 8 MiB, four times the 2 MiB libtest threads
  the suite runs the deepest nesting the parser accepts on, since what still
  recurses on the Rust stack is the front end and the compiler, bounded by the
  parser's nesting ceiling ("What the trampoline moved" above) but for a
  chain of messages, which the compiler still walks by recursion. It is
  reserved, not committed, so only what a program uses is backed by memory.
  The whole command runs there, not only its sends, because the symbol
  interner is thread-local (`symbol.rs`) and every symbol a command interns
  must be interned on the thread that uses it, so the thread would be needed
  for that alone. The goldens that recurse through
  `do:`, instantiation and the nested-class accessor — `do-recursion`,
  `instantiation-recursion` and `accessor-recursion`, run in a debug build —
  end at L like any other recursion, and pin that the error, not an abort, is
  what a program sees.

Why the limit is a count and not a memory bound, as Cog and Primordial Soup
have, or a stack-pointer check, as V8 and HotSpot have: both trip at different
depths under the two lowerings, whose activations differ in number and size,
and the second also between a debug and a release build.

### A superclass chain nests one activation per level

A level's initializer invokes its superclass's from inside its own
activation (`InitializeSuperclass`), so the levels of a chain that run code
nest, one counted activation each, below the factory's, and a chain of more
such levels than the depth limit fails with "stack overflow: recursion too
deep", caught like any other
(`goldens/language/class/instantiation/a_chain_deeper_than_the_depth_limit_overflows.ns`,
1,000 levels instantiating and 101,001 overflowing). That is decision 6 of
`new` as compiled code ([object-model.md](object-model.md), "Decisions"),
taken knowingly: a flat chain would need a Rust driver between the levels,
the walk compiled `new` replaced, and the references nest the same way, a
superclass initializer being an ordinary call. A level whose initializer
would run nothing is passed by `Mutator::enter_or_pass` in a loop rather
than by recursion, so such levels neither nest nor escape the count: the
length of a chain of them is an image's to choose, the same exposure
`dispatch::walk` is iterative for, and a chain of any length of them costs
the factory's one activation
(`levels_without_code_are_passed_without_an_activation`,
`tests/instantiate.rs`).

### What the golden outcomes are trusted on

`Signal::Return` is an unwind of the activation stack to its target entry
("Unwinding" above), with the semantics it had when it propagated
through Rust `?`. This is the part of the trampoline the golden outcomes are
trusted hardest on: the reference lowering runs on the same VM, so it cannot
cross-check the unwind — both ways of running a program unwind through the
same code.

### How other systems bound recursion depth

The limit's design started from what the prior art does with a recursion that
outgrows its stack:

| System | Model | Depth limit |
|---|---|---|
| Primordial Soup | Fully trampolined: a closure's `value` primitive is `ActivateClosure`, which pushes a frame and returns to `Interpret`, so Newspeak code never recurses on the C stack. The frame stack is 1024 slots (`kStackSlots`, `vm/interpreter.h`); on overflow `StackOverflowOrInterrupt` flushes every frame but the top one into heap activations (`FlushAllFrames`, `vm/interpreter.cc`). | None. Depth is bounded by the heap; `vm/virtual_memory_*.cc` treats a failed allocation as `FATAL`. |
| Cog | Stack pages. `handleStackOverflow` (`src/spur64.stack/interp.c`) moves frames to the next page, first divorcing that page's frames into heap contexts if it is in use (`divorceFramesIn`). | None. Depth is bounded by memory. |
| Nova | Recursive on the Rust stack. `Agent::check_call_depth` (`nova_vm/src/ecmascript/execution/agent.rs`) counts execution contexts. | 3500 — "experimental number that caused stack overflow on local machine" — then a `RangeError`. |
| SOMns | A Truffle AST interpreter, recursive on the Java stack. | None. A `StackOverflowError` is a JVM failure; one of its test scripts raises the thread stack (`-Xss4096k`). |
| Lua 5.4 (not in `prior-art/`) | Lua-to-Lua calls run in the VM loop without recursing in C; C functions and metamethods re-enter it. | Two: the Lua stack at 1,000,000 slots (`LUAI_MAXSTACK`), and nested C calls at 200 (`LUAI_MAXCCALLS`) — "stack overflow" and "C stack overflow". |
| CPython ≥ 3.12 (not in `prior-art/`) | Since 3.11 a Python-to-Python call consumes no C stack. | Two: `sys.setrecursionlimit` (default 1000) counts Python frames only, and C recursion — builtins, extensions — is guarded separately; `RecursionError`. |
| V8, HotSpot | Frames on the native stack. | The stack pointer against a limit (V8's `--stack-size`, HotSpot's `-Xss`), so the reachable depth depends on frame sizes and the compilation tier. |

Newspeak's own tests assume the Smalltalk behaviour: `KernelWeakTests.ns`'s
`deepStack: 1000` exists to overflow Primordial Soup's frame stack and force a
flush.

The Smalltalk-lineage VMs make depth a question of memory, not a count, and a
trampolined VM could do the same, since its activation stack is memory it
reserves itself. ns-next counts instead, keeping a byte bound only as the
net beneath the count ("The depth limit" above), because a memory bound
would trip at different depths under its two lowerings — the reference lowering's activations are more
numerous and hold more registers — and the reference lowering is meant to check
an overflow too. What the count includes is decided in
[compiler.md](compiler.md), "How the limit counts". A fixed count like Nova's is
otherwise what a runtime falls back on when the stack its evaluator recurses on
cannot grow. The systems that took their calls off the native stack — Lua,
CPython from 3.11 — separate the two resources that can then run out, with a
generous bound on the language's own depth and a tight one on native
re-entries; Primordial Soup and Cog need no second bound because nothing in
them re-enters the interpreter from native code. ns-next is in their position:
nothing re-enters its run loop from Rust ("No re-entry from Rust" below), so
one bound, L ("The depth limit" above), is all it needs.

### No re-entry from Rust

Rust enters the run loop only as its outermost caller — a host's `send` or
`instantiate`, an application's assembly and its `main:args:` among them — and never from inside a
running computation. Of the three paths that used to run Newspeak code from
Rust, `do:` is a Newspeak loop -- `Array>>do:`, which sends `value:` for
each element as any send does ([core-library.md](core-library.md), "Real
bodies for control flow and `do:`") --, the nested-class accessor is a
trampoline step, and instantiation is compiled code that runs in the loop
like any method.
This was step 2a of Milestone 4, decided 2026-09-13, and it is what the root
discipline stands on ([memory.md](memory.md), "No Rust frame is live below a
safepoint"): between two instructions no Rust frame between the host and the
loop holds a value.

A trampoline step answers an activation together with a `Continuation`, the
continuation is a row of the side table of continuations the activation's
frame names, and `Mutator::resume` applies it when the activation returns.
The semantics and the order of every observable effect are what they were
when these paths ran in nested runs of the loop.

- **The nested-class accessor** (`interpreter/nested.rs`) answers the class as
  a value when the receiver's cache already holds it, and also when the
  declaration has no superclass clause, whose superclass is `Object` with no
  code to run. Otherwise it answers the clause's activation, a
  `Step::SuperclassClause`, pushed with the declaring class as its current
  class and continuing with `Continuation::NestedClass`. The boxed
  `NestedClassCreation` it carries holds the enclosing object, the index of
  its cache slot, the declaration and the declaring class; `resume` hands it to
  `nested::after_clause`, which does steps 3 to 5 — the application, the class
  object, the cache ([object-model.md](object-model.md)) — and answers the
  class. While the clause runs, the class's cache slot itself says that its
  creation is underway: `push_step` writes a marker into it once the clause's
  push has succeeded (`nested::begin_creation`), and `resume` or
  `Mutator::abandon` sets it back to nil when the activation is popped
  (`nested::end_creation`), before `after_clause` caches the class there. An
  access that finds the marker is the cyclic-definition error; the slot is
  already keyed by the enclosing object and the declaration, the pair a cycle
  is, so the check is the one read the accessor makes anyway. The marker is
  `false`, a primitive the image format already has: nothing but the accessor
  reads a hidden slot, so no Newspeak code sees it, and no image holds one,
  since only a host snapshots, between runs; an image that puts `false` in a
  hidden slot itself gets a cyclic-definition error, the same kind of
  wrong-but-safe answer as any other non-class value it puts there. The reset
  replaces only the marker: the decoder does not bound `InitializeSlot`'s field
  index, so an image's clause body can overwrite the slot, and a debug build
  counts the marks rather than asserting on the slot's contents, which such an
  image could turn into a panic. `abandon` is `resume`'s counterpart for a
  continuation whose activation the unwinder throws away rather than returns
  from: the loop forwards every popped entry's continuation to it and decides
  nothing, as `vm::finish` forwards to `resume`.
- **Instantiation** is compiled code ([object-model.md](object-model.md),
  "Instantiation"): a factory send activates the factory entry's body, a
  `Step::Factory`, whose instructions allocate the instance and answer the
  class's initializer as a `Step::Initializer`, and each initializer
  answers the next level's the same way from inside its own activation
  ("A superclass chain nests one activation per level" above). Every
  activation delivers its value with `Continuation::Deliver`, as a method's
  does, so no continuation and no state outside the stack carry an
  instantiation. The factory answers the instance it allocated, whatever
  its initializer answered: both lowerings end an instance initializer by
  answering `self` and discard a top-level `^` (`compiler::lower`'s
  `finish_body`, `compiler::reference`'s `end`), and
  `new_answers_the_instance_its_initializers_ran_on`
  (`crates/ns-runtime/tests/instantiate.rs`) pins it under both lowerings,
  from a host's `new` and from a program's; a decoded image's initializer,
  which may answer anything, cannot change it, since the factory body is
  never on the wire. `Interpreter::instantiate` is the host entry, shaped
  like `send`: it runs the same factory body (`decide_instantiation`).

**Every body is inline in its `Step`.** `Step::Factory`,
`Step::Initializer` and `Step::SuperclassClause` each carry only what their
push cannot find cheaply -- an initializer carries the body its decision
found, and a clause's code is its declaration's -- so none needs a boxed
description of its activation, and a factory carries its body's `Rc`,
which the lookup or the cache entry already holds. Each is no larger than `Step::Method`, on purpose: a
`Step` as large as `Step::Method` with an optional continuation left
`Result<Step, Signal>` at the 56 bytes the hot path was measured at and
still made every benchmark 4–6% slower, none of which instantiates in its
loop — the same effect `StepArguments::ControlFlow`'s comment describes,
presumably the discriminant's niche.
`a_decision_stays_the_size_the_hot_path_was_measured_at`
(`interpreter/mod.rs`) pins the size; it cannot see the niche. Every body
is counted.

**One host entry.** `Interpreter::send` and `Interpreter::instantiate` fetch
what the host's roots hold, decide, and hand what they decided to
`Interpreter::run_from_host`, which answers a value as it is and otherwise
pushes the activation and runs `vm::run` on it; what the run answers goes back
to the host rooted ([memory.md](memory.md), "Host roots").
It is the one place Rust enters the loop. A debug build checks there that the
loop is not already running — nothing it calls can reach `run_from_host` —
that the stack is left as deep as it was found, and that no nested class's
creation outlives the run. That the loop is never nested is also structure,
which the debug assertion only confirms: a decision, a native or a
continuation is handed only a `&mut Mutator`, which offers no way back to the
`Interpreter` and so no `run` at all ([memory.md](memory.md), "The brand").

A decision reads a send's arguments where they are, in the sender's
registers, while it holds `&mut Mutator`: it is handed an argument window,
`interpreter::Arguments`, which names them by the sender's
`RegisterBase` and reads each one through the heap it is handed, or names
values a decision or the host built (`Message>>sendTo:`'s, a host's send,
a `doesNotUnderstand:` handler's message). Each case is one word -- the
register base's index, the first register and the count packed into a `u64`
by arithmetic (`heap::ArgumentSlots`), or a reference to the values'
slice -- so the window is a tag and a word, built in registers and passed
in two: as three narrow fields beside a fat slice it was a sixteen-byte
aggregate, built by narrow stores and copied or read back wider, and every
such read waited for the stores to retire: +24% on Mandelbrot, measured
by bisecting the first stage of road step 7a′ ("What contiguous arguments
moved"). Nothing gathers the arguments anywhere first, so the step state
keeps no argument buffer: a send whose sources are consecutive registers
names them in place, and only one whose sources are not copies them into
its slots, out of line (`vm::place_arguments`). The registers
stay put from the send's start to its push: no collection runs inside a
decision, only at safepoints, and nothing pops the sender meanwhile.
`Interpreter::reenter`, `invoke_closure`,
`builtins::call_block`, the count of re-entries and its bound, and that
bound's error, `stack overflow: too many nested native calls`, are gone with
the re-entries.

**Three recorded outcomes changed.** The three goldens now under
`goldens/runtime/depth-limit/` that recurse through `do:`, through instantiation and through
the nested-class accessor ended in
`stack overflow: too many nested native calls`, the bound on re-entries, and
end in `stack overflow: recursion too deep`. Each of those recursions is made
only of counted activations now, so it ends at L like any other recursion;
the limit the old text reported no longer exists. That is why the change is
right — it was not recorded because the code produced it. The interpreter
thread's stack was re-derived from what still recurses on the Rust stack
("The depth limit" above).

The step was measured whole against the commit it started from: "What
removing the re-entries moved" above. Instantiation, `do:` and `bench_send.ns`
are level with it or faster; `bench_loop.ns` and `bench_block.ns`, which run
none of the changed paths in their loops, came out 1.4–1.9% slower, an open
finding that step 2b's measurement puts at +1.1%, overlapping, and −2.5%
("What 2b moved" above).

### How this meets the liveness requirements

Reification is not a compromise against
[liveness-and-reflection.md](liveness-and-reflection.md); it is that document's
own unifying principle applied one level down. The interpreter level is the
source of truth and a JIT is an accelerator that must be able to deoptimize to
it. A stack is an accelerator over the reified activation model, and
reification is its deoptimization.

That resolves the one requirement that looks like a conflict.
Edit-and-continue must be able to add a local to a method while an activation
of it is live, and only the top entry's register window can grow in place. The
answer is that editing a method takes its affected activations off the stack
first — reified and divorced while still running, the operation Cog's
`divorceAllFrames` performs on every stack page before a snapshot. A frame
object never grows; the activation will be divorced into a newly allocated,
larger frame object and the VM will rewrite every reference to the old one by
a walk of every root the collector traces (`trace.rs`) and every heap object,
the new frame allocated old and born remembered
([object-model.md](object-model.md), "The object layout"). Continuing
the activation from its new frame is a resume from a suspended frame
("Suspending and resuming" above), which exists; continuing a decoded frame
does not, since an image carries nothing of the side table
([image-format.md](image-format.md), "Planned: suspend and resume"). The
document already accepts that entering the debugger forces the
frames it touches out of their fast representation; this is the same step, one
level below the JIT.

## Closure conversion

**Built 2026-09-20**, designed with the user on 2026-09-19. M4's step 4
([PROGRESS.md](../PROGRESS.md)), placed after the inline caches and
before the collector, whose retention it affects most. The decisions are listed
under "Decisions" below; this section is what they add up to.

Under the optimized lowering a block does not reach its method's variables
through the method's frame. A captured variable that is never written after the
block reads it is **copied** into the closure when the closure is created; one
that is written lives in its scope's **indirection vector**, which the
declaring body and every block that needs it share. The reference lowering
keeps the by-reference model, as the oracle the conversion is checked against.

Before this, the activation stack paid for the by-reference model with an
unconditional divorce: a reified frame copied all its registers into the arena
when its activation returned, because a surviving block might still read them.
Whether a returning activation copies its registers out is now decided per
activation, from the block's own `outer_reach` ("Converted representation"
below). Conversion did not remove reification — a block with `^` still needs its home
activation, and so do the debugger and the activation mirrors — which is why it
came after the stack rather than before it: it changes where variables live,
not what an activation is.

**Every Smalltalk and Newspeak VM that moved to stack frames made this change to
do it.** Cog's closure compiler introduced copied values and remote temps
(`pushClosureNumArgs:copiedValues:`, `pushRemoteTemp`) precisely so frames could
live on a stack, and Primordial Soup does the same (`PushClosure`,
`PushIndirectLocal`; its compiler's `ClosureRewriter1`,
`prior-art/primordialsoup/newspeak/Newspeak2PrimordialSoupCompilation.ns`, and
the three cases `ActivationMirrorTesting.ns` names: "Not copied", "Copied",
"Indirect"). Reaching the frame by reference is what systems do whose frames
live in the heap anyway — Smalltalk-80's Blue Book, ns-next before the
activation stack, Nova, which follows ECMAScript's environment records to the
letter (`DeclarativeEnvironmentRecord`, a map of bindings per scope, chained) —
or whose JIT can remove the cost: SOMns materializes the frame for every block
(`prior-art/somns/src/som/interpreter/nodes/literals/BlockNode.java`,
`frame.materialize()`) and relies on Graal's partial escape analysis, which
removes the materialization only when the block is inlined.

### The closure keeps its defining activation

A closure keeps `captured_frame`, the reified activation that created it, as in
Cog (`BlockClosure>>outerContext`), Primordial Soup
(`Closure::defining_activation`) and SOMns. Only variable access moves to
copies and vectors. Newspeak's reflective API depends on the link:
`Activation>>enclosingActivation` (`KernelForPrimordialSoup.ns`) and
`ActivationMirror>>enclosingActivation` (`Mirrors.ns`) name a block
activation's creator, the debugger labels a stack entry "`[] in Foo bar`" by
walking that chain (`ActivationMirrorTesting.ns`, `describeActivation:to:`),
the creator is still answered after its method has returned
(`testClosureActivation`, on a block `seven` returned), and a block evaluated
directly in its method has `sender = enclosingActivation`
(`testEnclosingActivationEquality`). The collection iterators are
Newspeak loops rather than forwarders to a leaf ([core-library.md](core-library.md),
"Real bodies for control flow and `do:`"), so every block one runs is such
an entry.

The alternative — reify nothing for a block without `^`, carry only what it
captured, as the JVM does (a lambda holds its captured values and no frame;
JDWP shows only those) — was weighed and rejected: it saves one frame-arena
entry per block created, not per block run, and it would have weakened
`enclosingActivation` to `nil` or to a stand-in without identity. A variant that
kept the link only while a debugging flag was set was rejected too: a closure
created without it, including every closure in an image serialized from a run
without it, could never be given it back.

What a returned converted frame keeps is its receiver, its program counter, its
`home` and its `uncontinuable` flag, but none of its registers' values -- they
stay the nil the frame object was allocated with: a variable nothing
captured is unreachable once its activation returns, and a captured one lives
in a closure or a vector. So what a surviving block retains is what it captured
plus that frame object, not the values of the frame's registers. An activation that created a
fallback closure is the exception and keeps them, which is exactly what
`outer_reach` decides ("Converted representation" below).

The link costs a closure nothing it did not already hold, and the home is no
longer a field of its own: `Heap::closure_home` derives it from
`captured_frame` — that frame itself when it is a method's, an initializer's
or a superclass clause's, and that frame's own `home` when it is a block's.
The decoder refuses a closure frame without a `home`
(`DecodeError::FrameHomeMismatch`), and `closure_home` still falls back to the
captured frame rather than asserting: a decoded frame is never married or
suspended, so `raise_return` treats it as a dead home and raises
`cannotReturn` either way, where an assertion would have aborted the process
on an untrusted image ("An image is untrusted input", `CLAUDE.md`).

### Where the analysis lives

The capture analysis is a pre-pass of the optimized lowering,
`compiler/capture.rs`, not a resolver pass. The resolver counts every block
literal as a scope and cannot tell which blocks `compiler/control_flow.rs`
inlines; a variable read only in an inlined `ifTrue:` block is not captured,
and the loop rule below depends on knowing which blocks are inlined loops. The
pass calls `control_flow::recognize`, so it and the lowering agree on every
block. Neither the resolver nor the reference lowering knows about it.

`analyze(RootBody, &bindings) -> Captures` walks one code family — a root body
(method, initializer or superclass clause) and every block
literal under it — and answers, per body, keyed by `BodyKey::Root` or
`BodyKey::Block(Span)`:

- **each local's place** (`Place`): `Register`, whether it is captured by
  copy or not captured at all, or `Vector(slot)`, a slot of the body's own
  indirection vector;
- **each real block's copied values** (`Captured`), **in first-reference
  order**: each entry — a copied variable (`Captured::Variable`) or a scope's
  vector (`Captured::Vector`) — takes its place the first time the walk needs
  it, and the two kinds interleave rather than grouping (`Walker::needs`, and
  `finish`, which deduplicates while keeping that order). That order is what
  the block's register layout is built from, so it is part of the answer, not a
  presentation detail. What a block needs includes, **transitively**, whatever
  a block nested in it needs from further out — a middle block carries what an
  inner one needs, as in Cog — and a block that only *writes* an enclosing
  variable needs that scope's vector just as a reading one does, as does every
  real block between the two
  (`goldens/language/block/a_nested_block_that_only_writes.ns`);
- **which locals an inlinable branch of the body assigns**
  (`branch_assigned`), which is what "Fewer protective copies" below reads.

**The rule.** Parameters and immutable locals (`=`) are always copied: nothing
but their initialization writes them (§3.6). A mutable local (`| x |`,
`x ::= …`) goes into the vector when

1. a real block nested in its scope writes it, or
2. its own scope writes it after a real block has read it, in the lowering's
   textual order, or
3. its own scope writes it inside an inlined loop — the body or the condition
   of a `whileTrue:` or `whileFalse:` the compiler inlines — that also contains
   the reading block literal, even when the write is textually before it: the
   next iteration's write happens after this iteration's block was created.
4. its own scope writes it inside a block of an inlined `ifTrue:`, `ifFalse:`,
   `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:` or `or:` — outside any real
   block — and a real block reads it anywhere in the scope: such a write
   counts as after every capture, whatever its position ("Fallback blocks",
   below).

Rules 1 and 2 are Primordial Soup's (`ClosureRewriter1`'s comment and
`localWriteNode:`). Rule 3 is what ns-next's inlining adds: its inlined blocks
take no temporaries (`lower_inline_block_body`'s gate), so every variable a loop
writes is its method's, and textual order alone would let
`[i < 3] whileTrue: [x:: i. blocks add: [x]. i:: i + 1]` copy `x` and leave the
first block answering the first `x` instead of the last. Every other captured
mutable local is copied — the common "declare, set once, then capture" pattern
costs no vector.

### Fallback blocks

An inlined `ifTrue:`, `ifFalse:`, `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `and:`
or `or:` keeps a run-time guard whose failure path sends the real message with
real closures (`emit_guarded_branch`, whose `on_fallback` lowers the block
literals through `lower_block_literal`), so that inlining stays a pure
optimization (`goldens/language/control-flow/control_flow_fallback.ns`). The same block
literal is lowered twice: inlined on the common path, a real closure on the
fallback path. An inlined `whileTrue:` or `whileFalse:` is not affected: its
fallback is `SignalNonBoolean`, with no closure.

A fallback closure can outlive the statement that made it — a non-Boolean
receiver's `ifTrue:` may keep the block and run it later — so treating it as a
converted block would put every variable written in any `ifTrue:` branch into a
vector (rule 1), captured or not. Instead **a fallback closure, and every block
nested in it, keeps the by-reference model**: it reads and writes through
`LoadOuterLocal`/`StoreOuterLocal`, reaching the registers of the frame whose
common path it stands in for — that frame's own locals, its copied-value
registers, and, for a vector variable, its vector register followed by
`LoadVector`/`StoreVector`. Because such a closure can write a local of that
frame at any later time, rule 4 counts its writes as after every capture: a
variable written in an inlinable branch goes into the vector only when a real
block also captures it. The common path pays nothing; the fallback pays only
when it runs.

`lower_fallback_send` is the one place this is decided: it sets the `Lowerer`'s
`blocks_by_reference` around the fallback send, so every block literal lowered
underneath it — the fallback closure itself and anything nested in it — gets a
by-reference `Layout` (no copied values, no vector, every local in the register
its resolver index names) instead of a converted one. So the two lowerings of
one literal differ in more than where they sit:
`a_real_block_nested_in_an_inlined_block_is_converted_and_its_fallback_copy_is_not`
(`compiler/lower.rs`) pins both halves of one literal, the converted one
reading a copied register and the fallback one a `LoadOuterLocal` at depth 2.

Rejected: converting fallback closures like any other block (a vector for every
variable any branch writes), and sending `mustBeBoolean` on a conditional's
fallback path as Cog does, which would change recorded outcomes and make
inlining observable.

### Converted representation

**The vector** is an ordinary `Array`, of the indexable format, as Cog's
indirection vector is. No new object kind: the heap, `trace.rs` and the image
already handle it, and Newspeak code never receives it as a value, since only
the new instructions touch it. One vector per body that has vector variables,
not a box per variable — the Smalltalk and Newspeak lineage's choice (Cog,
Primordial Soup); Strongtalk, V8 and .NET instead keep one context per scope
holding every captured variable, read or written, and Lua, CPython, Kotlin
(`Ref.IntRef`/`ObjectRef`), Scala and Groovy box per variable. An `Array`'s
elements sit inline in its block ([object-model.md](object-model.md), "The
object layout"), so a box and a vector each cost one allocation for one
variable, and the vector saves `k − 1` for `k`. A small-vector inline representation was
considered and not built.

**Registers.** A method's or initializer's layout is parameters, temporaries,
then one hidden vector register when the body has vector variables, then
scratch. A block's is parameters, **copied values**, temporaries, the hidden
vector register, scratch. A vector variable keeps its resolver index, whose
register then simply stays `nil`, so every `Binding` stays valid. `Layout`
(`compiler/lower.rs`) is where that arithmetic lives — `local_register` pushes
every temporary up by the copied count, `register_holding` answers the register
a given `Captured` entry sits in, and `locate` turns a resolver index into a
`Location` in one frame, which `access_through` lifts into an `Access` by
adding the reading body's distance from that frame.

**Instructions**, emitted by the optimized lowering only:

- `MakeVector { destination, size }` at the start of the body, before any local
  initializer runs, since `| x ::= 3. |` on a vector variable writes the vector.
  A parameter is never a vector variable, so, unlike Cog, nothing is copied into
  the vector on entry. It is instruction 0 and sits outside every
  `statement_program_counter` range, as the terminating return already does:
  a range starts at the instruction count the lowering has already reached, so
  the body's first statement starts at 1, and `disasm` prints `-` where it
  prints a statement number for every instruction a statement owns.
- `LoadVector { destination, vector, index }` and
  `StoreVector { vector, index, source }`: a vector variable's read and write —
  the method through its own vector register, a block through its copy of it.
- `MakeClosure` carries `copied: Vec<u16>`, the registers whose values go into
  the closure. The reference lowering always emits an empty list, and so does
  the optimized one for a fallback closure.

The optimized lowering emits `LoadOuterLocal` and `StoreOuterLocal` only inside
fallback closures ("Fallback blocks"); everywhere else they remain the
reference lowering's.

**Activating a block** writes the closure's copied values into the registers
after its parameters (`Heap::fill_copied_values`, called by `Step::Block` right
after the push), as Cog and Primordial Soup do ("Copied values are included
at the beginning of temporaries"). A block then reads a copied variable as a
plain register, with no instruction at all. Those registers are read-only by
construction: the analysis puts every variable anything writes after capture
into a vector. `self`, `current_class` and `^` go through `captured_frame` as
before. The fill allocates nothing — the values go straight from the object
arena into the register window — and it stops at the end of the activation's
own window, so a crafted image whose `copied` list is longer than the register
file writes nothing above it.

**A closure object is three fixed fields and its copied values**: its code
id, its captured frame and its feedback base, then the copied values inline
([object-model.md](object-model.md), "The object layout"), 56 bytes with its
slot and nothing copied. It holds nothing that duplicates what is already
held: not the block's parameter names, whose only use was their count, which
the block code's `argument_count` already is and which
`Heap::closure_parameter_count` reads from there; and not its `home`, which
the captured frame determines (`Heap::closure_home`, above). `trace_block`
traces the captured frame and the copied values, never the two numbers.

**The divorce.** Whether a returning frame keeps its registers is decided per
activation, at run time. `CompiledCode` carries `outer_reach`, a **depth**, not
a flag: `bytecode::outer_reach_of` takes the maximum of the body's own
`LoadOuterLocal`/`StoreOuterLocal` depths and of each nested `MakeClosure`'s
block reach minus one. So it is 0 for converted code, 1 for a body that creates
a fallback closure reading its own registers, and one more per level above
that. It is computed when the code is built, by the compiler and by the decoder
alike, and is not on the wire.

`MakeClosure` calls `Heap::retain_registers` on the creating activation when
the block it creates has a non-zero reach, and `pop_activation` copies a
reified frame's registers out only for an entry so marked. A depth rather than
a flag is what keeps the chain unbroken through nesting: a closure that reaches
two frames out gives the body that creates it a reach of 1, so that body's own
creation marks *its* activation too, and so on down to the innermost. Under the
reference lowering every block reads something outer at depth ≥ 1, so every
activation that creates a closure is marked and every frame is divorced with
its registers, exactly as before; under the optimized lowering only an
activation that actually created a fallback closure is.

**An untrusted image** can put any value where code expects a vector.
`LoadVector` and `StoreVector` check the value's kind and the index and raise a
run-time error, not a panic, on a mismatch (`interpreter::locals::load_vector`
and `store_vector`, whose error names `indirectionVector`); code the compiler
produced never has one.

### The image format

Tags follow "assigned once, never renumbered or reused"
(`crates/ns-runtime/src/image/opcodes.rs`, pinned by
`crates/ns-runtime/tests/opcode_tags.rs`):

- `MakeClosure`'s payload changed, so tag 11 is **retired** and `MakeClosure`
  is 29; `MakeVector`, `LoadVector` and `StoreVector` are 30, 31 and 32.
  `IntegerBinary` ("What leaf inlining moved", above) took the next number,
  33. The next free instruction number is 41.
- A closure's object record carries its copied values and no longer its
  parameter names or home, so `OBJECT_CLOSURE` 4 is retired
  (`RETIRED_OBJECT_BODIES`, the first retired body tag) and the record is 6.

A code record carries `copied_count: u16`, so that a block's register layout is
known from its code alone: `Heap::fill_copied_values` sizes and fills the
window between a block's parameters and its temporaries from it, and the
decoder reconciles every `MakeClosure` and every closure record against it
without needing the creating instruction in hand. `outer_reach`
is *not* on the wire: the decoder derives it with the compiler's own
`outer_reach_of`, which works because a block's code precedes its parent's in
the table.

The decoder's verification, before any decoded code runs:
- every register operand of the three new instructions is within the body's
  `register_count` (`own_registers`, which also lists a `MakeClosure`'s copied
  registers beside its destination);
- a `MakeClosure`'s `copied.len()` equals its block's `copied_count`
  (`CopiedCountMismatch`, raised from `lexical_parents`);
- a closure record's copied values number its code's `copied_count` (the same
  error, raised from `decode_closure_record`, before any value is read);
- `argument_count + copied_count` does not exceed `register_count`
  (`CopiedCountOutOfRange`, in `u32` arithmetic in `decode_code`, asked only
  once `argument_count <= register_count` so that a body whose arity alone
  overruns its register file is still reported as `ArgumentsExceedRegisters`).

Each of those counts sizes its `Vec` through `reserve_for`, which caps the
capacity at the bytes left in the reader — the per-field pattern the format
already uses, so a claimed count allocates in proportion to the input rather
than to a `u16` ([image-format.md](image-format.md), "Verification";
`bounded_count` is what bounds the graph section's one record count, which
these fields are not).

A vector's size and kind are run-time values and are checked at run time
(above). `disasm` prints the three new instructions and the registers a
`MakeClosure` copies, through `bytecode::disassemble`, which `inspect` uses
too; `inspect`'s own structural walk reads a closure record's copied values
from the same tags as the encoder and decoder. A
returned converted frame is written with its whole register file, `nil`
where nothing was copied, and `verify_graph` holds a frame record's register
count to its body's ([image-format.md](image-format.md), "Verification").

### Fewer protective copies

`materialize` (`compiler/lower.rs`) copies a named mutable local into a scratch
register when something lowered before its consumer may write it. That question
used to be asked per expression class, and **every send** counted as such a
write, because the code it runs could write the local through a closure
(`CLAUDE.md`, "An expression's value register must still hold that value").
Under the optimized lowering a converted closure writes no register of another
activation: a captured written variable lives in a vector, and a vector read
lands in a scratch register as a snapshot; a copied register is read-only. The
one exception is a fallback closure ("Fallback blocks"), which writes by
reference exactly the locals its inlinable branch assigns. A local's register
can therefore be written only by an assignment in its own body, including inside
an inlined block, or — for a local some inlinable branch of the body assigns —
by a fallback closure that any send may run.

So the question is now asked **per register**, not per expression class.
`a_send_can_write(register)` is the whole of "a send runs user code that can
write this local": true only for a named local of this body that the analysis's
`branch_assigned` marks and whose `Place` is a `Register`. In a by-reference
body — a fallback closure, or a real block nested in one — it is true for every
writable register, the conservative branch, since every closure nested in such
a body reaches this frame by reference. `writes_register(expression, register,
inlined_depth)` walks the expression without descending into real block
literals: an assignment writes the register when its value expression does,
when its target resolves to that register in this frame, or when its target is
a setter send and `a_send_can_write`; a send or a cascade writes it when
`a_send_can_write` or when its receiver or any message's arguments do; a
recognized inlinable form walks its receiver and its blocks' statements
instead. An assignment through a setter copies its value only under the same
condition.

`inlined_depth` is the one subtle parameter. The resolver counts every block
literal as a scope, including one an inlinable form inlines, but the matching
`ScopeKind::Inlined` is pushed only while that block is lowered — after
`lower_operand` asks the question — so the walk increments `inlined_depth` as
it descends into an inlined block's statements and subtracts it from the
binding's depth before resolving. It cannot underflow: the inlining gate admits
no block with parameters or temporaries, so no binding inside an inlined block
names that block's own scope. Getting it wrong is a missed copy, which is
silent wrong output at exit status 0, so it is pinned two levels deep both as
lowering tests and as a golden (`goldens/language/block/writes_two_inlined_levels_deep.ns`,
whose two loop-only methods cannot be rescued by `branch_assigned`).

`lower_assign`'s `debug_assert!`s are unchanged, and `check_writable` carries
both of them — this frame's own `read_only_locals` table for a register write,
the target frame's for a `StoreOuterLocal` — while skipping a vector slot,
which is writable by construction because only a variable something writes is
ever put in a vector. The invariant in `CLAUDE.md` and
[compiler.md](compiler.md) keeps its statement and has this as its
justification. The reference lowering is unaffected. A mistake here is silent
wrong output with exit status 0; the reference lowering is the oracle for it,
and `goldens/language/block/a_fallback_closure_writes_a_local_a_send_reads.ns` pins the
one shape `a_send_can_write` answers `true` for at run time — a register-resident
local an inlined `ifTrue:` assigns, whose fallback closure a later send runs — in
each of the three positions that read such a local across a send: as an operand,
as an argument and as a setter assignment's value.

The goldens `goldens/language/evaluation-order/` holds for the same invariant —
`operands_that_mutate_each_other_keep_their_evaluated_values.ns` and
`assignment_through_a_setter_answers_the_value_before_the_setter_ran.ns` — do
*not* cover it, which is why that one was added: the first writes the local
from the same body (`writes_register`'s assignment case, never
`a_send_can_write`), and the second writes it from a real block, which after
conversion makes it a vector variable whose read is already a snapshot.

### Verification and the reference lowering

The reference lowering stays on the by-reference model, so that a defect in the
capture analysis — rule 3 above all — shows as the two lowerings disagreeing,
as a cache defect shows against the uncached reference since 3b. It is an
interim oracle: **the reference lowering is to be removed at some point, and
from then on the recorded golden outcomes are how this is tested.** The goldens
are written with that in mind: the capture goldens in `goldens/language/block/` were recorded on the
unconverted `master`, before any of this existed, and cover copied and vector
variables; a write before capture, after it, inside a `whileTrue:`, a
`whileFalse:` and a loop condition; a block that writes; nested blocks three
deep that carry a variable through a middle block; a variable read only in an
inlined `ifTrue:`; a vector variable with an initializer; an initializer body
and a slot initializer creating blocks; `^` from a block whose home has
captured variables; a closure surviving its method; a non-Boolean receiver
whose `ifTrue:` keeps its fallback closure and runs it later — reading and
writing a variable another block captured, from inside a method, a block and an
outer block that has returned; and the protective-copy cases above. Each
recorded output was checked against the semantics by hand before it was taken.

Two goldens were added while building, for defects the recorded set did not
cover: `a_nested_block_that_only_writes.ns`, for a block that only *assigns* an
enclosing scope's variable and the middle blocks that have to carry that
scope's vector to it — the capture analysis did not record such a need, and the
shapes failed to compile until it did — and `writes_two_inlined_levels_deep.ns`,
for the `inlined_depth` arithmetic above.

Four more were added once it was built, for what the recorded set had covered
only as lowering tests — which pin what the compiler emits, not what a program
answers, and so would not survive the reference lowering's removal:

- `a_fallback_closure_writes_a_local_a_send_reads.ns`, the run-time case
  `a_send_can_write` exists for ("Fewer protective copies" above);
- `a_block_in_a_fallback_closure_outlives_its_method.ns`, a real block *inside*
  a fallback closure reading a method local two frames out, evaluated after the
  method has returned: what `outer_reach_of`'s folding of a nested
  `MakeClosure` is for, and the only golden in which the method's frame is
  marked by nothing but that folding;
- `a_blocks_own_registers_and_vector.ns`, a block whose layout has all three
  summands of `vector_register = arguments + copied + temporaries` non-zero,
  and rule 3 applied to a block's own local rather than a method's;
- `and_or_fallback_closures_share_variables.ns`, the fallback closure of an
  `and:` and of an `or:`, which only `ifTrue:` had stood for.

### Cost

What it changed, for a method that creates a non-inlined block:

| | By reference (the reference lowering, and a fallback closure) | Converted (optimized lowering) |
|---|---|---|
| Per activation of a body that creates a block | a reified frame, plus a copy of every register at return | a reified frame; the closure's copied values; and, when the body has vector variables at all, one vector — `MakeVector` is the body's first instruction, so that allocation happens on every activation of it, block created or not |
| A block reading a captured variable | one hop per nesting level, then a live-or-dead branch | a register if copied, a register then the vector if written; independent of depth |
| The method reading its own captured variable | a register | a register if copied; through the vector if written |
| What a surviving block retains | the whole frame, including registers it never reads | what it captured, plus the frame record without registers |
| A later JIT | every local a block can reach stays in memory once a block exists | copied values are ordinary values; escape analysis can remove a vector |

**What that was worth, measured: nothing either way on speed, and about 2.6%
more peak working set on `bench_block.ns`.** The design expected the
interpreter to be no slower and the memory to fall; the first held and the
second did not. "What closure conversion moved", in the measurements section
above, is the measurement, with the reasoning for why the memory moved the way
it did and how far that reasoning is evidence.

## Running an instruction again after forcing a future

An instruction that meets an unresolved future where it needs the value -- `ResolveFuture`, `InitializeSlot` into a value's
slot, and a send whose leaf declined, raised or answered "no" because an
operand is a future -- cannot force it in Rust, since nothing re-enters the
loop. It answers a step instead: the future's computation -- its block
activated, or `value` sent to anything else -- under
`Continuation::Future`, whose `FutureForcing` carries
`AfterForcing::Reexecute`. For `ResolveFuture` and `InitializeSlot`,
`vm::force_and_reexecute` stores the instruction's `program_counter` -- `run`'s
doc comment lists the instructions that must -- and pushes that step; a send
has stored its `program_counter` already, and pushes a leaf's step as it
pushes any. When the computation returns, `vm::finish` hands its value
to `Mutator::future_computed`, which records it and answers a
`FutureOutcome`: `Reexecute`, and `vm::reexecute` switches back to the
caller at the stored `program_counter`, so the same instruction runs again
and finds the future resolved; `Deliver`, for a forcing a message to the
future started (`futureValue:ifFail:`, `AfterForcing::Deliver`), and the
value goes to that send as any return's does; or `Force`, when the block
answered another unresolved future, whose forcing is pushed in the popped
activation's place with this one chained behind it. Every other
continuation still goes through `Mutator::resume`.

Running an instruction twice is safe because the first run wrote nothing:
`ResolveFuture` and `InitializeSlot` force before they write, and **a leaf
that declines or raises has changed nothing** ([dispatch.md](dispatch.md),
"Primitives"), so running again the send that ran it is running it once.
A forcing whose activation an unwind pops, or which `push_step` refuses --
the depth limit, or a block's arity -- is thrown away through
`Mutator::abandon`, which leaves its future, and every future chained behind
it, unresolved again. A host's send has no instruction to run again: a
forcing that was to run one and finds the run's floor below it ends the run
with an error (`interpreter::future::host_passed_a_future`). The design is
[object-model.md](object-model.md), "Simultaneous slots".

## The loop over locals

`vm::run` keeps the running frame's state in local variables of one loop
that runs many instructions inside one entry, each instruction advancing the
program counter itself; nothing answers a per-instruction value for the loop
to test. Designed with the user on 2026-09-28, after "What simultaneous
slots cost" above traced a 4% loss on `bench_megamorphic.ns` to where LLVM
placed the block that tested a send's answer, and built the same day
([plan](../superpowers/plans/2026-09-28-1808-ns-next-loop-over-locals.md)).
It dispatches on the eight-byte slots of "The compact instruction
encoding" below.

**Why.** Two costs sat on every instruction when each was one entry. The
answer each instruction gave the loop, `Flow`, was too large for registers
-- its `Switch` carried an `ActivationReference`, an `Rc<CompiledCode>`, a
program counter and a `FeedbackBase`, its `Exit` a whole `HostResult` -- so
every send wrote it to the stack and the loop read it back and tested it,
and how fast that test was depended on how LLVM laid out `vm::run`: one more
instruction arm that called anything reordered it and cost
`bench_megamorphic.ns` 4%. And every register access found the
activation's window on the stack, bounds-checked, before it read the register
itself, bounds-checked again (`Heap::register_slot`,
`Heap::activation_local`, the path every access outside the loop still
takes).

**Prior art.** Primordial Soup's `Interpreter::Interpret` keeps `ip_`, `sp_`
and `fp_` as fields of the interpreter object; a send is a call that rewrites
them in place (`Activate` sets `ip_` to the method's first bytecode), with no
answer to test, and its safepoint is one compare of the stack limit that
interrupts lower. Cog's StackInterpreter keeps `localIP`, `localSP` and
`localFP` as locals of `interpret()` so the C compiler holds them in
registers, and writes them out (`externalizeIPandSP`) before anything that
can collect and reads them back (`internalizeIPandSP`) after. ns-next takes
Cog's choice: in Rust, locals of one function are what LLVM keeps in
registers, where fields of an object mostly are not. Threaded dispatch
(computed goto) does not exist in Rust -- the `match` already compiles to a
jump table -- and a tail-calling interpreter needs `become`, which is
unstable.

**Two levels.** `vm::run` holds `FrameRegisters` -- the running
`ActivationReference`, its code (`Rc<CompiledCode>`), its program counter,
its `FeedbackBase` and its `RegisterBase` (below) -- and the run's floor.
None of them is branded: each is an index or a number a move leaves valid.
Its loop enters once per stretch between safepoints:
`interpreter.enter(|mutator| run_stretch(mutator, &mut frame, floor))`
answers `Leave::Safepoint`, and `run` runs the safepoint and enters again,
or `Leave::Exit(result)`, and the run ends with it. The inner loop,
`run_stretch` (`#[inline(always)]`, as is the closure), copies the frame
into its own locals -- Cog's `localIP` and `localFP` -- and dispatches
`code.slot(program_counter)` until a safepoint is requested or the
run's base activation is done; only when it leaves for a safepoint does it
write the frame back, in one place every safepoint shares. A move or
collection happens only between two entries, so within a stretch every
handle stays valid until the entry ends; the brand's guarantee is the one it
was when each instruction was an entry ([memory.md](memory.md), "The
brand"). The safepoint is tested where it always was -- every switch to
another activation and every backward jump, the latter in the one
`jump_to!` macro every jumping arm goes through -- as a compare of the
heap's flag, and only when it is set does the stretch leave its entry. Under
the move, cache, collection, scavenge and suspend stress modes it is set
at every safepoint their schedule picks, so the
suite still meets every handle held across one.

**No per-instruction answer.** A straight-line arm writes its register,
advances the program counter and `continue`s. A jump sets the program
counter, and a backward one tests the flag. A send, a return, an unwind and
a future's forcing go through the helpers -- `send`, `deliver`, `finish`,
`resume_caller`, `unwind` and `raise`, `reexecute`, `force_and_reexecute`
-- which answer a one-byte `Transfer`: `Next` (the instruction's value, if
any, is written; go on after it), `Switched` (run the frame the `Handoff`
names) or `Exited` (the `Handoff` holds the run's result). The `Handoff` is
a local of `run_stretch` the helpers write the next frame or the result
into, and a raise the index of the instruction that raised, so the test
after a send is one compare of a byte in a register. `vm.rs` still
holds no dispatch logic: every decision stays in `interpreter/` -- the
caches, `act`, the continuations, the unwinder -- behind the interfaces it
had, which a JIT tier will reach the same way.

**The register base.** `heap.rs` hands out an opaque `RegisterBase`, which
only `Heap` can mint: `Heap::activation_register_base(activation)` computes an
activation's window start once per switch, and `Heap::register(base,
index)` and `Heap::set_register(base, index, value)` reach the frame
stack's words with one index and one bounds check. It is an index, not a
pointer: it survives the stack's reservation committing more and a move,
which touches no register window, and so it sits in `FrameRegisters` across a safepoint. It carries
the danger an `ActivationReference` does -- once its activation is popped,
the next one at that depth reuses the window -- so in a debug build it
carries its activation and every access checks the activation's generation,
as `check_activation` does; in a release build it is a `u32`, which a
release-build test pins. `LoadOuterLocal`, `StoreOuterLocal`, the
indirection vectors and married frames are unchanged: a married frame's
registers are its stack frame's words, so a write through the base is the
write it always was. `read_local` and `write_local` remain for every caller
outside the loop, the out-of-line arms among them. No `unsafe`.

**What pins it.** The semantics are the suite's: every golden under both
lowerings, the image round trips, the corpus, and the stress modes,
of which the move and the collection modes catch a handle held across a
safepoint because the stretch still leaves its entry there. `heap.rs`'s unit tests pin that a stale
`RegisterBase` panics in a debug build and that one survives a deeper push.
What it moved is measured in "What the loop over locals moved" above.

## A send's value delivered where it is decided: tried and halted

Not built. The design had a send's decision write its value straight into
the sending instruction's destination register and answer one byte, handing
over only an activation or a signal whole, instead of coming back to the
loop as a `Result<Step, Signal>`. It came from the profile in "What the loop
over locals moved", "Where the 3% goes": the joined `Result<Step, Signal>`
slot cost a store-forwarding stall on every send (28% of `vm::run`'s
samples on `bench_loop.ns`) and a call of `Step`'s drop glue (4.7%), and it
was where one more instruction arm moved the timing. What addressed that
stall is the one-word value ([object-model.md](object-model.md),
"Values and objects"); this section records the design and the
experiment that decided against it.

**Prior art.** Primordial Soup's primitives push their result onto the stack
themselves and answer only whether they succeeded (`Primitives::Invoke`);
Cog's primitives do the same through `pop:thenPush:`
(`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:72079`). Neither hands
a result object back to the loop.

**The sink, as designed.** `interpreter/` would have gained a trait,
`DecisionSink`, with three methods: `value(mutator, value)`, `step(step)` and
`signal(signal)`. `act_with_base`, `decide_send`, the three cache paths
(`send_public`, `send_lexical`, `send_super`, with `cached` and `miss`) and
`mirror::execute_method` would have become generic over it: every place that
answers `Ok(Step::Value(value))` would call `sink.value` instead, a `Step`
would go to `sink.step`, an error to `sink.signal`. The logic would have
stayed in one place -- the invariant that one place decides -- with an
inlined copy per sink, and `miss` would have stayed out of line and been
handed the sink, delivering its value itself instead of through an
out-pointer joined with the hit paths.

**Two sinks, as designed.** `vm.rs`'s `RegisterSink` would have written a
value into the destination register (`Heap::set_register`) and recorded in
one byte what happened -- delivered, activate, raised -- keeping a rare
`Step` or `Signal` in an out slot, as `Handoff` keeps the next frame.
`StepSink` would have collected the decision as the `Result<Step, Signal>`
it is, with the functions every other caller uses -- the host's send, the
unwinder, `Message>>sendTo:`, the block leaves -- as thin wrappers over it,
so none of those callers would change and `vm.rs` would still decide
nothing.

**Gated by an experiment, and halted.** The plan's first task
([its plan](../superpowers/plans/2026-09-28-2106-ns-next-decision-sink.md))
tried the sink on `send_lexical` alone in a throwaway copy on 2026-09-28,
profiled with `xperf` and measured three builds a side against `master`.
As written -- `act_into` and the cache paths generic, `decide_slot_accessor`
generic but left out of line -- the drop-glue call vanished (4.66% to 0%)
but the stall moved into the out-of-line `decide_slot_accessor_into`, one
store there holding 33.8% of all samples, and every fixture got slower
(`bench_loop.ns` +1.4%, `bench_send.ns` +10.4%, `bench_block.ns` +4.0%,
`bench_megamorphic.ns` +5.7%). With `decide_slot_accessor_into` forced
inline as well, the joined slot's stall and the drop glue were gone and
every fixture faster -- `bench_loop.ns` -17.1% (separated),
`bench_send.ns` -3.5%, `bench_block.ns` -0.9%, `bench_megamorphic.ns`
-1.5% -- at the price of `vm::run` growing from 7,143 to 13,371
instructions and its frame from 0x3E8 to 0x7D8 bytes, with
`Heap::instance_field` and `instance_set_field` left out of line (13.8% of
samples) and `LoadLiteral`'s own stall (through `make_value`, 12.3% of
`vm::run` on `master`, 18.6% after) untouched. What the experiment showed
beyond the send path: LLVM copied the 16-byte enum `Value` as its tag byte and two
overlapping quadwords, so **any** such `Value` that passed through a memory
temporary stalled, and a sink helped only where delivery is inlined all the
way to the register write. The user halted the plan there (2026-09-28): the
stall is a property of a 16-byte enum `Value`, and the one-word value
([object-model.md](object-model.md), "Values and objects") removes it
everywhere. A revisit, should one be wanted, starts from the forced-inline
variant's numbers above.

## The compact instruction encoding

**Status: built** (designed with the user on 2026-10-02 and built the same
day;
[plan](../superpowers/plans/2026-10-02-1910-ns-next-compact-instruction-encoding.md)).
`CompiledCode` stores its body as fixed eight-byte instructions
(`bytecode/compact.rs`), a `#[repr(u8)]` enum with no heap field, and keeps
everything that does not fit eight bytes in side tables beside them; the run
loop matches on that enum. `Instruction` stays the compiler's and every tool's
vocabulary, but no body is stored as a list of them: a body is encoded as it
is emitted and read back through a decoded view, and only an instruction with
no eight-byte form keeps its `Instruction`, in the body's `wide` table. The
encoding came after the eight-byte value ([object-model.md](object-model.md),
"Values and objects"), and before the object space ([memory.md](memory.md),
"The object space"), so that it was designed against the value the
loop actually moves -- one tagged word -- and the interpreter stays the first
tier in front of the JIT, as Cog's does. What it moved is "What the compact
instruction encoding moved" above.

### What it starts from

**The dispatch findings** of "Where the 3% goes" above: with `Instruction`
as the stored form, every dispatch decoded its discriminant through a niche
(`xor`, `cmovns`) before its jump, because the enum's payloads -- a
`Literal` of 32 bytes holding a `String` or a boxed digit array, a
`Vec<u16>` of argument registers, an `Rc<CompiledCode>` -- give it one; and
`vm::run` was large enough that `program_counter` and the code pointer were
spilled to its stack frame. A send's argument registers were a separate
heap allocation the loop chased on every send, and `LoadLiteral` matched on
the literal's kind every time it ran, a constant included.

**What the code holds**, counted on 2026-10-02 over the sixteen benchmark
programs and the harness under `benchmarks/are-we-fast-yet/`,
`PlatformFactory.ns` and `CollectionsFactory.ns` under the optimized
lowering (`Kernel.ns` left out, since only `resolve_kernel` compiles it):
1,642 bodies, 23,635 instructions; the highest register index is 45, so
every register operand fits a byte; every jump lies within 127 instructions
of its target; of 7,400 sends, 3,366 take no argument, 3,523 one, 401 two
and 110 three to seven; the highest call-site number is 596; of 2,214
`LoadLiteral`s, 1,884 load `nil`, a Boolean, a `SmallInteger` or a `Float`,
and 766 of the 782 integers fit 16 bits. `SendPublic` is 20.7% of the
instructions, `Jump` 11.6%, `SendLexical` 10.6%, `Move` 9.5%, `LoadLiteral`
9.4%, `IntegerBinary` 5.3%. A static count says what exists, not what runs
or what a later program will need; it decides sizes, never whether a form
exists.

**The prior art.** All three are stack machines with a variable-length byte
code:

- **Primordial Soup** folds a small index and the arity into the opcode
  (`push local 0-7`, sends of literal 0-7 with 0 or 1 argument), has
  dedicated wide opcodes rather than extension prefixes, pushes -1 to 2,
  `nil`, `true` and `false` with one-byte opcodes and any 16-bit integer as
  an immediate, and keeps selectors and every other literal in the method's
  literal array (`prior-art/primordialsoup/vm/interpreter.cc:1062-1577`;
  `vm/object.h:818-826`). Its loop is a plain `switch`, with `ip`, `sp` and
  `fp` as fields of the interpreter (`interpreter.cc:1055-1062`,
  `interpreter.h:206-208`).
- **Strongtalk** aligns 32-bit words inside the byte stream so that the
  generated assembly interpreter reads them in one load, and keeps each
  send's inline cache in those words, rewriting the opcode as the site goes
  monomorphic, polymorphic or megamorphic
  (`prior-art/strongtalk/vm/interpreter/bytecodes.hpp:60-81`,
  `interpretedIC.cpp:406-516`). Its interpreter is threaded through a
  dispatch table, the top of stack cached in a register
  (`interpreter.cpp:686-734`).
- **Cog** builds operands of any width from the Sista extension prefixes
  `extA` and `extB`, keeps the bytecode in the same heap object as the
  literal frame, dispatches by computed goto, and its baseline JIT walks the
  bytecode through a descriptor table and maps a machine-code pc back to a
  bytecode pc through a byte map
  (`prior-art/opensmalltalk-vm/src/spur64.sista/gcc3x-cointerp.c:10856-10878`,
  `cogitX64SysV.c:396-413`, `:8090-8160`).

What their density answers is not ns-next's question: their code lives in
the heap and the snapshot -- Primordial Soup even swaps every saved `ip` for
an index around a collection (`interpreter.cc:1875-1906`) -- while ns-next's
code lives outside the heap and the image carries it in a wire format of its
own ([image-format.md](image-format.md)). Their stack form is Smalltalk-80's
inheritance, and a JIT recovers registers from it with a simulated stack
(Cog's `ssPush`), which ns-next's compiler has already assigned. Strongtalk's
caches in the stream contradict "a cache records what the lookup decided; it
never decides" and the code one mixin shares with every application of it.
Computed goto and threading have no stable Rust form: `become`, which a
tail-call-threaded loop needs, is experimental in Rust 1.98.0 (checked on
2026-10-02; [OPEN-WORK.md](../OPEN-WORK.md) holds the experiment that would
size its gain). What carries over is what all three share: constants with
their own opcodes, an integer immediate, sends specialized by arity, and
everything rare or wide kept out of the common form.

**The forms compared** on the same 23,635 instructions: `Instruction`
itself, 48 bytes an instruction, 1.13 MB plus a heap allocation per
argument list; a fixed 16-byte slot with `u16` operands, 378 KB; a fixed
eight-byte slot with byte operands, 189 KB plus small tables; a stream of
32-bit units, as Lua's and Dalvik's register machines encode, about 133 KB;
a register byte stream, about 95 KB. The eight-byte slot was chosen
(2026-10-02): a sixth of the enum's size and about 1.4 times the 32-bit
stream's, and it keeps the program counter an instruction index, so that
jump targets, the statement map, a frame's `program_counter` in an image,
the wire format and a later JIT's pc map stay exactly what they were, and
the compiler, not a hand-written decoder, checks every form. A
variable-length stream would make the program counter an offset everywhere
it is stored, and an encoding error in a decoder both lowerings share would
not show as a difference between them: the reference lowering is no oracle
for the encoding. A stack form was not considered further: it would undo
the register assignment, the closure conversion and the value-register
invariant the compiler is built on.

### The encoding

**One instruction is one eight-byte slot**, `CompactInstruction`, a `Copy`
enum `#[repr(u8)]` whose every field is an integer: a register is a `u8`, a
jump target an absolute `u32` instruction index, and anything else a `u16`
or `u32` index into a side table of the body. A `const` assertion pins the
size at eight bytes. `#[repr(u8)]` lays every variant out as a `repr(C)`
struct after the tag, so fields are declared narrowest first. The loop copies
the slot out (`code.slot(program_counter)`), so the match runs on a value in
registers, with no niche to decode.

**The side tables**, each a boxed slice of the body's `EncodedBody`,
private to `bytecode::compact` with every field of the sites they hold: a
slot and the entry it names are written together by `CodeBuilder`, and
only `for_each_emitted_mut` rewrites an entry afterwards (a call site or a
block), so no other code can leave a slot naming an entry of the wrong
shape. The loop reads them through `#[inline(always)]` accessors
(`constant`, `literal`, `send_site`, `block`, `mirror_site`,
`superclass_site`, `register_list`, `wide`), which left `vm::run`'s size and frame
unchanged:

- `constants`, the `Immediate` words `LoadConstant` reads -- a
  `SmallInteger` beyond 32 bits or a `SmallFloat64`, which hold no handle,
  so the code stays heap-independent and needs no `Trace`;
- `literals`, the `Literal`s `LoadLiteral` allocates afresh each time it
  runs: a `String`, a `Symbol`, a `LargeInteger`, a `Float` outside the
  `SmallFloat64` range, and the deferred `UnresolvedIdentifier` and
  `BodyTooLarge`;
- `send_sites`, one `SendSite` per send: its selector, its `CallSite`, the
  lexical depth for a `SendLexical`, and, for a wide send, its destination,
  receiver, argument base and argument sources;
- `blocks`, one per `MakeClosure`: the block's `Rc<CompiledCode>`, its
  parameters and its copied registers;
- `mirror_sites`, one per `SendMirror`: its argument base and sources and
  its failure target;
- `superclass_sites`, one per `InitializeSuperclass`: its steps, its
  selector and its argument base and sources, since the selector does not fit
  the slot beside the steps;
- `register_lists`, the element registers of a `MakeArray`, and the
  argument sources of an `InitializeAppliedLevel`, whose eight bytes hold
  its destination, its argument base and its steps (an `InitializeInstance`
  passes its body's own arguments and names no list);
- `wide`, the full `Instruction` of every instruction other than a send
  that has no eight-byte form here.

**Constants** (2026-10-02): `LoadNil`, `LoadTrue` and `LoadFalse` take only
a destination; `LoadSmallInteger` carries an `i32` in the slot, which
covers every integer literal counted above but 16, without a table read;
`LoadConstant` reads the `constants` table; `LoadLiteral` reads `literals`
and allocates. The encoder chooses by the literal, for a lowering and for
the image decoder alike, so the wire format is unchanged.

**Sends** (2026-10-02): `SendPublic`, `SendLexical`, `SendSuper` and
`SendInlinable` each come in four forms, for no argument, one, two, and a `Wide`
form that reads everything from its `SendSite` -- three arguments or more, or a
register above 255. The two-argument `SendPublic` is `{ destination, receiver,
argument_base, sources: [u8; 2], site: u16 }`, eight bytes -- a send names
its arguments' contiguous slots by their base and each argument's source,
and reads its arguments in place when the sources are consecutive, else
copies a source that is not its slot into it before it decides
([compiler.md](compiler.md), "Sends") -- and an eight-byte form's arm
turns its sources into the window's first register in registers: none
reads the base, one its source, two their first source after one compare,
and only two that do not follow each other call the placement, out of
line, with no allocation to chase.
`SendMirror` has one eight-byte form, its arguments and failure target in its
`MirrorSite`; like every other instruction it takes `Wide` when its operands do
not fit (a register above 255, or the mirror-site table full). Every send arm
only gathers its receiver, its argument window and its site into a `PendingSend` and
leaves the match through its kind's labelled block; **one send path per kind**
follows that block, so each kind's cache hit path is inlined into `vm::run`
once, not once per form. The pending send carries no kind: a single join for all
four kinds needs one, and was measured to cost `bench_loop.ns` 3.5% -- LLVM
built a stack aggregate with narrow stores and dispatched a second jump table on
the kind ("What the compact instruction encoding moved" above).

**`IntegerBinary` is one opcode per operation** (2026-10-02) --
`IntegerAdd`, `IntegerSubtract`, `IntegerMultiply`, `IntegerFloorDivide`,
`IntegerLess`, `IntegerGreater`, `IntegerLessOrEqual`,
`IntegerGreaterOrEqual`, `IntegerEqual` -- each `{ destination, left,
right, target: u32 }`, as Cog's `bytecodePrimAdd` and Strongtalk's
`smi_add` are opcodes of their own: no second dispatch on the operation.

**`Wide { index: u32 }`** is every other instruction that does not fit: a
register above 255, or a table index beyond 16 bits (a depth is a `u16` in
both forms, so it never decides).
Its `Instruction` sits in `wide`, and the loop runs it in one out-of-line
function, `run_wide`, that answers how to go on -- the next instruction, a
jump, or a transfer to another frame or out of the run. Each instruction's
effect is one function that its narrow arm and `run_wide` both call, so the
two forms cannot drift apart, and the rare wide operands cost `vm::run` one
call rather than a second copy of every arm. A send is never a `Wide`: its
own wide forms join its kind's path. That a body with more than 255
registers runs slower is a cost, not a semantic difference; every form
runs.

**One stored form** (2026-10-02). Both lowerings and the image decoder emit
`Instruction` values into a `CodeBuilder`, which encodes each at once and
patches jump targets in place (`Instruction::set_target` for a target in
`wide`); `CompiledCode` stores only the `EncodedBody`, the slots and their
tables. `CompiledCode::instruction(index)` and `instructions()` decode the view
back -- for `disasm`, the image encoder, the decoder's verification (once a
body) and the tests -- and the round trip is exact; `family_size` and
`outer_reach` are read off the tables without a decode, answering what
`family_size_of` and `outer_reach_of` answer over the view;
`instruction_count()` answers the body's length, and only `vm.rs` reads
`slot(index)`. Two `CompiledCode`s are equal when they mean the same -- every
field but the encoding, and the decoded instructions, a `Float` literal by its
bits -- so a body built narrow equals the same body built wide. The decoder
numbers a family's call sites in place through
`EncodedBody::for_each_emitted_mut`, which visits every send's `CallSite`, in
`send_sites` or in `wide`, and every block, in emission order whatever form each
took -- the compiler's own order ([compiler.md](compiler.md), "Safepoints and
call-site identity").

**The wide forms run under a stress mode** (2026-10-02):
`NS_NEXT_STRESS_WIDE=1`, debug builds only like the move, cache,
collection and scavenge modes and read once per process
(`interpreter/safepoint.rs`), makes `CodeBuilder::new` encode every
instruction in a wide form, so that the whole golden suite runs through the
`Wide` arm and the four wide send forms. It is one of the end-of-batch
gates. A test of the narrow forms builds with
`CodeBuilder::never_wide_by_stress`, so its assertions hold under every
mode. Goldens and tests pin the forms no measured program reaches:
`goldens/runtime/encoding/` a method of three hundred registers and sends
of three to eight arguments, under both lowerings, and
`crates/ns-runtime/tests/compact_encoding.rs` bodies past 65,536 constants,
send sites and allocated literals, whose first entry it checks is narrow
and whose 65,537th wide, and which answer under the optimized lowering;
the reference lowering passes its register ceiling first, so there each
raises the too-large error ([compiler.md](compiler.md), "Compilation cannot
fail").

**Measured before it was built** (2026-10-02), like the decision sink ("A
send's value delivered where it is decided" above): a throwaway copy ran
`vm::run` on the eight-byte slots and was measured against `master`, and
the user decided on its numbers to build the slots, so the 32-bit stream
was never measured. What the gate showed, the join it led to, and what the
built encoding moved are "What the compact instruction encoding moved"
above.

## Keeping the hot path in registers

The send, push and return path is Rust that LLVM compiles, and twice in
road step 7a′ its machine code, not its algorithm, cost 10-28%: a
sixteen-byte `Arguments`, and later the 72-byte `ActivationHeader`, were
written onto the stack by narrow stores and read back by sixteen-byte
loads, which the store buffer cannot forward, so every send waited for the
stores to retire ("What contiguous arguments moved"; "Representation" above);
and `Mutator::push_sent` grew from 276 to 7,265 instructions when the push
chain was inlined into it. Both were found only after hours of measuring.
Four rules keep the path in registers, and a gate holds them.

- **What the path passes or answers by value is at most two scalars.**
  Rust's ABI passes at most two scalars in registers; a larger aggregate
  goes through memory, a copy written and read back. The argument window
  is a tag and one word (`Arguments`, its slots packed into
  `heap::ArgumentSlots`'s `u64`), an activation's handle one 32-bit
  offset, the optional sender two scalars, the feedback base one, a
  continuation a tag and a box, a value one word, and a `Transfer` one
  byte. Release-only size tests hold the window, the handle, the sender,
  the feedback base and the continuation there
  (`interpreter`'s `what_the_hot_path_passes_by_value_is_at_most_two_scalars`,
  `heap`'s `an_activation_handle_and_an_argument_window_are_one_scalar_each`,
  beside the older pins of the handles and of `RegisterBase`). Three values
  are larger and still cross by value, through memory: the decided `Step`
  (56 bytes) that `push_sent` and `push_step` take, the pushes'
  `Result<ActivationReference, Signal>` (40), and the `Popped` (24) that
  `Heap::pop_activation` answers; the decision's `Result<Step, Signal>`
  and the `Signal` are pinned besides. Each is pinned at the size it was
  measured at, so that it does not grow
  (`what_the_hot_path_passes_through_memory_stays_the_size_it_was_measured_at`,
  `what_a_pop_answers_stays_the_size_it_was_measured_at`), and whether to
  lend them is open ([OPEN-WORK.md](../OPEN-WORK.md), "Three values of
  the send, push and return path cross by value through memory").
- **Nothing larger is moved through a `Result`, an `enum` or a `match`
  there; it is lent.** The activation header is built once, in
  `Mutator::push_placed`'s callers, and lent `&mut` to `push_above` and
  `push_on_slots` and from them to `Heap::push_activation_from` and
  `Heap::push_overlapped_activation`, which write it into the frame in
  place. A refusal hands it back to nobody: the push answers a `Refusal`
  of at most the arguments, and only `Mutator::push_refused`, `#[cold]`
  and `#[inline(never)]`, takes the header by value, and boxes it for the
  reserve's paths. The by-value entries that remain, `Mutator::push` and
  `push_placed`, are `#[inline(always)]`, so the header is never copied
  into them, and the public `Heap::push_activation` serves hosts and
  tests. No test can keep the header out of by-value signatures; the
  codegen gate's count of wide loads is what notices a copy of it.
- **Inlining is decided, not left to LLVM's budget.** The small helpers
  of the path are `#[inline(always)]` (`Continuations::insert` and
  `row_of`, `Heap::fill_copied_values`, the push layers), since a larger
  caller once pushed them past the budget and made each a call; the
  refusals and rare paths are `#[cold]` `#[inline(never)]`
  (`push_refused`, `push_past_the_limit`, `push_into_the_stack_reserve`,
  `Heap::push_reserved_activation`, `Continuations::insert_into_free_row`,
  `vm::return_to_suspended`), so that they cost the hot code nothing; and
  the out-of-line pieces of the loop, `vm::place_arguments` and
  `vm::run_wide`, are `#[inline(never)]`, since inlined they cost
  `vm::run` its registers ("What contiguous arguments moved").
- **The codegen gate holds the result.** `cargo xtask gates codegen`
  (`xtask/src/codegen.rs`; [README.md](README.md), "Gates") reads the
  release listing of `ns-runtime` and holds each out-of-line function of
  the path -- `vm::run`, `vm::run_wide`, `push_sent`, `push_step`,
  `push_initializer_placed`, `Heap::pop_activation`, `vm::finish`,
  `vm::resume_caller`, `Mutator::resume`, `vm::place_arguments`,
  `vm::unwind`, `Step`'s drop glue and `builtins::execute` -- to a budget
  of instructions and one of sixteen- or 32-byte stack loads over earlier
  narrower stores, each the count at the time with a headroom of about
  10%. At `0037350d`, the commit with the header's stall, it fails:
  `push_sent` has 6,588 instructions and 79 such loads against budgets
  of 5,390 and 14, `push_step` and `push_initializer_placed` likewise.
  The count is static, over the listing's order rather than the control
  flow, so it counts candidate sites, cold ones among them; what it
  catches is a jump in their number.

## Planned: tail calls, skipped but kept

**Status: a direction the user set on 2026-09-25, not designed and not
built.** Today every send pushes an activation and nothing is eliminated.
§3.6 specifies tail-call elimination through the continuation object —
when the sender has nothing left to do, the invoked activation's
continuation is the sender's own — "to the extent that it is observable",
marks it "Unimplemented", and in the same section requires that
"programming environments must always allow developers to retain
activations during debugging", noting that elimination "runs counter to
Newspeak philosophy" when it throws history away.

**The direction is the middle way, not a debug switch.** Making elimination
depend on a debug mode was considered and rejected for two reasons:
elimination is observable through stack depth, so a program that runs with
it would hit the depth limit without it, and the recorded outcomes would
depend on the mode; and a live system has no mode decided in advance — the
debugger opens on an uncaught error, a `halt` or an interrupt, by which time
eliminated activations would be gone. Instead, when it is built, a tail send
**skips** the sender in the continuation — the return goes straight to the
sender's continuation, which is what makes the stack not grow — while the
skipped activations stay reachable for the debugger, as the specification's
split between continuation and activation allows; MIT Scheme's bounded
history of eliminated frames is the prior art for keeping the recent ones
visible without keeping all of them. How much is kept is for the design to
settle.

**One constraint is fixed already:** an activation the exception machinery
marks — `on:do:`, whose `^self value` is a tail send — is never skipped, or
`signal` would no longer find its handler. `ensure:` is never in tail
position, since `onUnwind value` follows `self value`.

## Overlapping register windows

**Status: road step 7a′, decided with the user on 2026-10-24, built and
measured in two stages.** The first makes a send's arguments contiguous
slots or consecutive sources, read in place through an argument window,
the argument buffer gone ("What contiguous arguments moved"). The second
overlaps the windows: the header 13 words in every build, its numbers
left out of the register numbering, a send's own callee laid on its
caller's slots -- the caller's outgoing argument registers *are* the
callee's first registers, so its arguments are copied into no window of
the callee's -- the pop restoring the top the callee was pushed at,
suspension keeping a sender's registers below its callee's header, frame
objects and image records without the header's numbers, and the
decoder's refusal of a register that names a header word, of argument
slots below the header's end or below the body's locals, of a send source
that is another of its slots, of a write to an argument register, and of
an outer load or store into another frame's scratch
([image-format.md](image-format.md), "Verification"). Neither a push nor
a pop writes a scratch register; the leftover analysis decides what the
collector nils instead ("What the leftover analysis moved"). The second
stage costs nothing on the geometric mean against the first ("What
overlapping windows moved").

**What a send copies.** Each argument at most once, most of them
never. A send names its arguments' contiguous slots, allocated after its
receiver, and each argument's source. A send's value, an inlined integer
operation's, an inlined conditional's or `and:`/`or:`'s, a literal or a
closure is produced in its slot, a setter's value as any argument; a
named local passed as an argument, and an argument whose value the
optimized lowering leaves in the register an inlined loop or mirror send,
an assignment or a cascade answered, is not ([compiler.md](compiler.md), "Sends"). The send
reads its arguments where they are whenever its sources are consecutive
registers -- none or one always are, and so are sources that all are their
slots -- and only otherwise copies each source that is not its slot into
it first, out of line (`vm::argument_window`, `vm::place_arguments`). The
decision and the native then read the arguments where they are, through an
argument window (`interpreter::Arguments`, a tag and one word, "No
re-entry from Rust"), and a decision that answers without a push -- a
primitive, a slot accessor, a cache hit's value -- writes no slot. The
push of the send's own callee lays its frame on the slots
(`Mutator::push_on_slots`, `Heap::push_overlapped_activation`): a window
that names consecutive sources rather than the slots is copied into the
slots first, on the push path, after the decision (`Heap::place_in_slots`;
one or two word by word, more by a loop kept inline), and a window that is
the slots is copied nowhere. Every other push copies the window from
wherever it is, above the sender's whole window. A copy into a slot
nothing reads would be pure cost, and nothing but the send reads a slot
but a setter assignment: its value is the register its argument came
from, a named local's below the slots, or else the slot -- where the value
was produced there, or copied there by the lowering when it was left in a
register above the slot, an inner assignment's (`a:: b:: 4`), which the
callee's header goes over
(`compiler::lower`'s `lower_setter_assign`). The first stage measured what
the copies cost: the window built in memory up to +24% (Mandelbrot), the
placement inlined into every send path and its eager copies +2% to +9% on
every send-heavy program, before both went.

**The prior art overlaps them, and puts the header between the arguments
and the temporaries.** Cog's stack frame is the caller's pushed receiver
and arguments, then the saved frame pointer, method, flags, context and a
copy of the receiver, then the temporaries
(`prior-art/opensmalltalk-vm/src/spur64.stack/gcc3x-interp.c:150-157`,
`internalActivateNewMethod` at 5601-5660; arguments read above the frame
pointer, `temporary:in:` at 65657-65667); Primordial Soup's is the same
(`prior-art/primordialsoup/vm/interpreter.cc:18-45`, `FrameTemp` at
94-101), and Strongtalk's native frame too
(`prior-art/strongtalk/vm/runtime/frame.hpp:29-51`). A primitive reads
its arguments in place in all three, and a failed one activates the method
over them. Lua keeps the header out of line (`CallInfo`) so that the
window is contiguous from `base`, the compiler placing the arguments at
the top of the free registers; Dalvik's `invoke/range` takes contiguous
registers and its non-range form copies once, into the callee's place.

**The layout is Cog's, with the header's registers left out of the
numbering.** A frame is its arguments, then the header, then the rest of
its registers:

```
caller   ... live registers | slot 0 .. slot argc-1 | dead above
callee                      | r0 .. r(argc-1)       | header (13) | r(argc+13) ..
```

- **The callee's register numbering skips the header**: registers `0 ..
  argc` are the arguments, `argc .. argc + COUNT` name the header's words
  and no instruction names them, and copied values, temporaries, the vector
  register and scratch start at `argc + COUNT` ([compiler.md](compiler.md),
  "Registers"). Every register is still `base + r`, one addition and no
  branch on an args/temps split; the header sits at `base + argc`, and an
  `ActivationReference` is its offset.
- **The header is 13 words in every build**, since its size is in the
  compiled code and a debug build's artifact or image must run in a
  release one: the debug build's generation rides in the flags word,
  whose argument count is 16 bits -- 8 + 16 + 2 + 31 bits, inside 61.
  The register ceiling bounds the count: a body with more parameters
  compiles to the raising body ("Compilation cannot fail"), whose declared
  arity still passes the arity check, so a longer list of values -- a
  host's send or `Message>>sendTo:` -- is refused at the push instead:
  `Interpreter::push` raises the error the raising body would, without
  pushing, so the sender is the activation that signals it.
- **A send's arguments are contiguous slots at the top of the caller's
  live registers**. Both lowerings allocate the `argc` slots right
  after the receiver and lower each argument into its slot: a literal or a
  send writes it directly, so nothing copies it. Every register numbered
  above the slots is dead when the send executes -- the callee's header and
  window overwrite them -- which the bump allocator makes true: nothing is
  allocated below a slot after it, and what an argument is computed with
  lands above. Both lowerings check it on every body in a debug build
  (`compiler::reference::check_leftovers`): no instruction after a send
  reads a register above its slots before something writes it again.
- **A named local passed as an argument is the send's to copy**: the
  send names its slot base and, for each argument, the register it comes
  from, reads consecutive sources in place and copies any other source
  that is not its slot into it ("What a send copies" above) -- at most
  one copy, inside the send's own handling, where a `Move` before it
  would cost an instruction. The reference lowering never uses it: it
  `Move`s every local into a fresh slot, as its rule 3 already asks, so
  that the optimized lowering's in-send copy is what the two are compared
  on
  ([compiler.md](compiler.md), "The reference lowering").
- **A push without the arguments in the caller's registers copies them**
  to the top of the stack, above the caller's whole window: a
  `doesNotUnderstand:` handler's message, `Message>>sendTo:`, `cull:`, a
  future's forcing, an unwind block, a host's entry, a resumed
  frame, and the initializer a factory enters, whose arguments are the
  factory's own and lie below its header. A future forced before a send
  is re-run lands above the caller's window too, so the slots it re-reads
  are untouched. Only an activation that takes the run loop's send's own
  arguments (`StepArguments::Send`) is laid on the slots; a mirror send's
  leaf and `InitializeInstance` hand theirs on from elsewhere, and land
  above too (pinned by
  `goldens/language/evaluation-order/a_push_that_is_not_the_senders_callee_leaves_its_window.ns`).
- **A pop restores the top it was pushed at**, not the callee's
  offset: the caller's window end, or past it where the caller lies on a
  longer frame's slots, since a push never lowers the top. The caller's
  registers above its slots are its own again, dead, and the pop writes
  none of them, nor does a push write a scratch register: every trace of
  the stack nils each register a frame does not read before it writes it
  (`Heap::trace_stack`; [memory.md](memory.md), "What the collector
  traces"; "What the leftover analysis moved"). A debug build writes a
  poison word no value can be into both, and refuses an instruction's
  read of it ("Representation" above). Suspending a sender copies its
  registers below its callee's header only and leaves the rest nil, so a
  frame object holds no stale header word, and nils every register the
  trace would. A frame object and an image record hold the registers
  without the header's numbers: register `r` is field `r` below the
  argument count and field `r - 13` above the header, and a number the
  header takes has no field (`Heap::frame_field_of_register`), so a frame
  of a body past the register ceiling whose arguments leave no number
  above the header holds its arguments alone, and an image carrying one
  decodes.
- **Parameter slots are immutable (§3.6, §6.3.2)**, so a callee never
  writes the caller's slots: a setter assignment's value, read from the
  slot after the send, is still there (pinned by
  `goldens/language/evaluation-order/a_setter_value_read_after_its_send_survives_the_callee.ns`).
  The decoder refuses an instruction that names one of the header's
  numbers, a send whose slots start below the header's end, whose
  callee's header would go over its sender's, an instruction that writes
  an argument register, and an outer load or store into another frame's
  scratch, which would reach a live frame above it
  ([image-format.md](image-format.md), "Verification").
- **The decision reads the arguments where they are**, through an
  argument window -- the frame, the slot base and the count, read through
  the `Heap` -- instead of a slice; there is no argument buffer.

**Two stages, each measured** as "What frames as words moved" was, with
three builds a side. The first makes the arguments contiguous and drops
the buffer: both lowerings lower a send's arguments into slots at the top
of the live registers, the send forms name the slot base and each
argument's source, and decisions, primitives and builtins read the
argument window. A window is a base and a count, so the slots come
first: over arbitrary register lists the window would have to lend the
instruction's list to every decision. The second overlaps the windows.
The first is kept: no program slower than `master` with its builds
separated, twelve of twenty-four faster ("What contiguous arguments
moved"). The second, measured against the first with the change that
forgets a value-answered send's step, which speeds both, is level on the
geometric mean, -0.09%, with two programs slower with their builds
separated, by 0.5% and 1.6%, and three faster; with what of that the
code could fix fixed, DeltaBlueTon80 is 0.9% slower and Mandelbrot
level. It is kept, decided with the user on 2026-10-26, against the rule
that a stage slows no program: the 0.9% is the flags load for the
register base on every switch that Cog's layout, the point of the stage,
forces, four ways around it measured no better, and the branch as a whole
is 1.34% faster than before the leftover analysis with no program slower
("What overlapping windows moved", "What the leftover analysis moved").

## Planned: JIT tiers

Would change: hot code runs as machine code instead of in `vm::run`. Nothing
is designed beyond the inline caches; the tiering model is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md), and the constraint
that binds every tier is that no native code may ever enter an image. The
tiers are the last three steps of the road to a high-performance VM (decided
with the user on 2026-09-29; the whole order is in
[OPEN-WORK.md](../OPEN-WORK.md), "The road to a high-performance VM"). They
build on the representation steps before them -- the memory layer, the
eight-byte value, the object space with its header word, class word and inline
fields, generations and inline allocation ([memory.md](memory.md),
[object-model.md](object-model.md)) -- which give generated code fixed field
offsets, a guard read in one load, a cheap write barrier and a bump
allocation it can emit itself, and on the real Newspeak bodies of
`whileTrue:`, `whileFalse:` and `Array>>do:` ([core-library.md](core-library.md),
"Real bodies for control flow and `do:`").
Each step gets its own design when it comes up.

- **Step 7, an execution model for compiled code.** Frames that interpreted and
  compiled code share, as Cog's stack pages are, in place of today's
  trampoline over the activation stack for compiled callers; a calling
  convention; GC maps, so the collector finds and rewrites the values in a
  compiled frame at a safepoint; and deoptimization metadata, so a compiled
  frame can be turned back into interpreted ones. Deoptimization serves the
  debugger as much as the optimizer: inspecting, stepping and editing a
  compiled activation goes through it, as it does in Strongtalk and
  HotSpot. Decided with the user on 2026-10-24, in four parts and this
  order. **7b**, resuming an activation from its suspended frame, is
  built ("Suspending and resuming" above): the landing point of every
  later deoptimization, lazy as Primordial Soup's `CreateBaseFrame` and
  Cog's `makeBaseFrameFor` are, with a sixth stress mode that runs every
  golden on from suspended frames; `retry` keeps popping down to its
  `on:do:` activation, which is never skipped ("Planned: tail calls,
  skipped but kept"), and the decoder refuses every frame not marked
  uncontinuable ([image-format.md](image-format.md), "Planned: suspend
  and resume").
  **7a**, frames as words with a fixed header, every word tagged, so no
  stack map is needed, as in Cog, Strongtalk and Primordial Soup, is
  built ("Representation" above): one contiguous reservation of
  `ns-memory` holds them, as HotSpot's, V8's and Strongtalk's native
  stacks are contiguous, with the depth limit kept as the bound the
  goldens pin and a byte bound added only as a net ("The depth limit").
  It is kept because the interpreter measures no slower in release, no
  program slower with its builds not overlapping `master`'s -- the
  condition it was built under, without which it would have moved to
  step 8: measured with three builds a side that vary the block layout
  inside `vm::run`, nineteen of twenty-four programs are faster and the
  rest level ("What frames as words moved"). **7a′** is
  built: the register windows overlap, the caller's outgoing arguments
  being the callee's first registers, in Cog's layout, in two measured
  stages ("Overlapping register windows"), the first -- contiguous
  arguments read in place -- faster than `master` on twelve of
  twenty-four programs ("What contiguous arguments moved"), the second
  level with the first on the geometric mean ("What overlapping windows
  moved"), neither a push nor a pop writing a scratch register since the
  leftover analysis decides what the collector nils ("What the leftover
  analysis moved"). **7c** is written: the deoptimization contract and
  the protocol between interpreted and compiled frames, "Planned: the
  deoptimization contract" below, which steps 8 and 9 build to.
- **Step 8, a baseline JIT**, Cog's class: every method compiled without
  speculation, the inline caches and the allocation fast path emitted into
  the code, values kept in registers, for x64 and arm64, since images are
  portable across architectures. Executable memory comes from `ns-memory`
  ([memory.md](memory.md), "The memory layer"). Designed on 2026-10-27:
  "Planned: the baseline JIT" below; its first two stages, the
  foundation, are built ("The baseline JIT's foundation").
- **Step 9, an optimizing JIT with type feedback, inlining and deoptimization**,
  Strongtalk's and HotSpot's class: recompiling hot code along the receiver
  types the caches recorded, inlining the sends that make Newspeak slow when
  they are not inlined -- slot accessors, `outer` sends, kernel methods that
  forward to the VM mirror -- and deoptimizing when a speculation fails. Its
  target is measured against Java with the Are We Fast Yet benchmarks, whose
  Newspeak versions SOMns carries (`prior-art/somns/core-lib/Benchmarks`)
  and whose Java counterparts are under `benchmarks/are-we-fast-yet/java/`
  ("The Are We Fast Yet benchmarks"), and with
  `prior-art/newspeak-benchmarks`, a separate suite -- DeltaBlue, Richards,
  Splay, parser combinators and micro-benchmarks -- with Java versions of five of its programs.

**Open: whether the interpreter tier is generated by the JIT's backend.**
`vm::run` is a Rust `match` loop, compiled by LLVM, which decides where the
program counter, the register base and the mutator live and has twice
cost 10-28% through codegen ("Keeping the hot path in registers");
`vm::run` holds about 60-70% of `bench_send.ns`'s samples. JSC's LLInt is
written in offlineasm, V8's Ignition handlers are built by its optimizing
compiler's backend, and HotSpot's template interpreter is generated at
start-up by its macro assembler; each keeps the program counter, the
frame and the dispatch base in fixed machine registers and leaves no
compiler between the design and the instructions, which typically gives
1.3-2x on interpreter-bound code. LuaJIT's interpreter is hand-written
assembly, which is not the option here: a hand-written interpreter beside
a generated JIT is two implementations of every instruction, which drift.
The interpreter would instead be emitted by the macro assembler road step
8 builds. It is decided only once the instruction set and the frame
layout settle and that macro assembler exists
([OPEN-WORK.md](../OPEN-WORK.md), "Open design questions").
Before that, a tail-call interpreter -- a handler per instruction, each
ending in a guaranteed tail call to the next, which Rust gives only with
`become`, unstable in October 2026 -- would measure the gain of fixed
registers without a generated tier (the same question in OPEN-WORK.md).

Steps 1 to 6 of the road give a very good interpreter on a modern object
representation; road step 8 reaches Cog's class, and road step 9 Strongtalk's and
HotSpot's.

## Planned: the deoptimization contract

Would change: nothing yet; this is the contract road steps 8 and 9 build
to (road step 7c, decided with the user on 2026-10-27). It says where a
compiled frame lives, how control passes between interpreted and
compiled code, when a compiled frame becomes interpreted ones and back,
and what each observer of an activation may assume. The interpreter
stays the source of truth ([liveness-and-reflection.md](liveness-and-reflection.md),
"The unifying principle"): every compiled frame can be turned into the
interpreted activations it stands for, and nothing a program, the
collector, the debugger or an image observes depends on which tier ran.

**The principle.** The hot path of compiled code writes nothing that only
an observer needs, and everything an observer needs is reconstructed at
the one place observation can begin: the exit of machine code to the run
loop. Every observer -- the collector, a suspension, the unwinder, the
activation leaves, a deoptimization, an interpreted callee, later the
debugger and an activation mirror -- runs only after such an exit. That
is HotSpot's arrangement, debug information at safepoints rather than in
the frame, carried over to a stack of tagged words with no stack map,
which is Cog's and Strongtalk's: neither has stack maps, because every
value live at a point the runtime can see is a tagged word in the frame.
Cog flushes its register-cached values before every send, backward
branch, conditional jump and trap (`ssFlushTo:`,
`cogitX64SysV.c:40797-40812`, `:38825-38840`, `:34200-34210`,
`:36290-36298`) and traces every word of both frame kinds
(`cointerp.c:20768-20860`); Strongtalk spills every live value before a
send (`compiler/codeGenerator.cpp:1396-1412`) and traces every word of a
compiled frame (`runtime/frame.cpp:342-349`). Cog's citations in this
section are to the generated C under
`prior-art/opensmalltalk-vm/src/spur64.sista/`, Strongtalk's to
`prior-art/strongtalk/vm/`.

### Where a compiled frame lives

- **On the activation stack, at the interpreter's positions.** A compiled
  frame occupies the layout "Representation" above describes -- its
  arguments, the thirteen header places, then its registers -- and
  baseline code numbers its registers as the interpreter does, so that a
  baseline frame and an interpreted one differ only in which code
  resumes them. The header word that tells them apart is a bit in the
  flags word, since the code word is a `CodeId` and not an address Cog
  could compare against its code zone (`cointerp.c:19944-19947`).
- **The hot path writes two header words: the receiver and the current
  class.** A compiled call places its callee on its argument slots as the
  interpreter's send does, and writes the callee's receiver and current
  class into their header places; every other header word is written when
  the frame is materialized ("The exit materializes" below). The current
  class cannot be derived later: a class object exists per enclosing
  instance, a cache cell holds no handle, a return address names only the
  call site, and a walk of the receiver's chain for the code's mixin --
  Primordial Soup's -- answers the wrong level where one mixin is applied
  twice in a chain; and a lexical or `super` miss inside the callee needs
  it before any exit. The call computes it as the interpreter's
  materialization does, from the chain's start and the entry's level
  (decided with the user on 2026-10-28). Every system read writes its
  frame's identifying words eagerly -- Cog's prologue four (the method,
  the context, the receiver, the saved frame pointer;
  `cogitX64SysV.c:31390-31443`), Sparkplug's six, the interpreter frame's
  (`prior-art/v8/src/builtins/x64/builtins-x64.cc:1906-1929`) -- so a
  call costs two stores more than HotSpot's and less than Cog's. A value
  that lives across a send the code did not inline is stored into its
  slot before the send, as HotSpot and V8 spill every live value across
  a call.
- **The return addresses are on the native stack** of the thread that
  runs the interpreter, on which compiled code calls and returns with the
  processor's `call` and `ret`, so a return is predicted by the return
  stack buffer. They are not on the activation stack because it grows
  upwards and a native call needs a stack that grows downwards; they are
  on the thread's own stack rather than a reservation of their own so
  that a call into Rust needs no switch of stacks, as HotSpot's and V8's
  need none ("Planned: the baseline JIT", "The native stack carries the
  return addresses"). It holds only the return addresses of the
  compiled frames above the run loop's latest entry into machine code,
  and the frame's own base register addresses the activation stack.
- **Machine code holds no handle**, as compiled code holds none today
  ([compiler.md](compiler.md), "Readers of compiled code take the
  mixin"): a literal object is loaded through the heap's tables, so the
  collector never patches code, where Cog's has to (`cogitX64SysV.c:9453-9792`).
- **Optimized code may declare a raw area** in its frame, a range of
  words fixed per code -- not per program counter -- that the walk of the
  stack, the leftover pre-pass and the debug poison all pass over. An
  unboxed float or integer then lives across a call without being boxed;
  with only tagged spills, a float outside the `SmallFloat64` range kept
  across a call in a hot loop would allocate a box on every iteration.
  HotSpot affords raw spills through its oop maps; one range per code is
  the whole map ns-next needs. Baseline code declares none.

### The exit materializes

Machine code leaves for the run loop -- it returns to the loop rather than
calling into it, so nothing re-enters the loop from Rust ("No re-entry
from Rust" above) -- when it takes a safepoint whose poll found a
request, sends to a body that has no machine code, meets a continuation
other than `Deliver`, unwinds what it cannot cut itself ("Non-local
return and unwinding" below), raises an error, or calls a leaf that
reads or writes another activation. Before it leaves, the exit walks the
return addresses on the native stack down to the run loop's entry and writes
each compiled frame's header but the two words its call wrote: its code, its program counter, its
caller word, its flags with the compiled bit, its continuation (`Deliver`),
its feedback base, its guard and entered level (none, so the first
own-slot access takes the cold check, as a resume does today: "Suspending
and resuming" above), and its reified word, nil unless the frame reified
itself, whose reification wrote it then ("Returns" below). Everything it writes is derived: a return
address names a call site, the call site its code and program counter,
and a callee's frame lies at a fixed offset of its caller's, the call
site's, so the caller word follows. Each compiled frame's registers hold
by then every value an observer can need, spilled at its send. The exit
then resets the native stack to where the run loop entered machine code.

From the exit on, **every frame on the activation stack reads as an
interpreted frame**: a baseline frame exactly, word for word, an
optimized frame through its metadata ("Optimized frames" below). The
collector walks it as today, `Heap::trace_stack` skipping only a raw
area; a suspension, the unwinder and the activation leaves read it as
today. So the stored program counter is written by no instruction of
compiled code -- Strongtalk maps a return address to its scope and
bytecode index the same way (`code/nmethod.cpp:508-531`,
`runtime/vframe.cpp:52-61`), Cog lazily
(`mustMapMachineCodePC:context:`, `cointerp.c:21633`) -- and "Where
`program_counter` is stored" above stays true of every frame an observer
sees.

### Returns

- **A frame that lived through an exit returns only through the run
  loop.** When the loop resumes a compiled frame, it enters the frame's
  code at the machine address its program counter maps to, through a
  table per code; that frame is the bottom of a new stretch of machine
  code, and its return goes to the loop again. Each frame pays one slow
  return per exit it lived through -- after a scavenge, the stack's depth
  times the loop's round trip, which is small beside the scavenge itself.
- **A frame pushed by compiled code since the latest exit returns by a
  bare `ret`.** It was pushed by a send, so its continuation is
  `Deliver`; it counts against the depth limit; and only its own code can
  have reified it, since every other reifier -- a handler search, a
  `currentActivation` leaf, the depth reserve, a suspension -- runs after
  an exit. Whether a return must divorce a frame object is therefore
  known from the code: only code with an instruction that reifies its own
  frame tests it, and every other return tests nothing. The continuation
  word and the reified word are read only on the loop's path, as
  `vm::finish` reads them today.
- **Compiled code creates no fallback closure.** An inlined conditional
  whose receiver is not a Boolean hands its frame to the interpreter at
  that instruction, to run it again there, as Cog converts its frame at
  `mustBeBoolean` (`cointerp.c:16346-16366`).

### Safepoints and the depth limit

- **A poll writes nothing back.** Only the cold path of a poll that found
  a request spills the registers its code caches and takes the exit;
  receiver and arguments may travel into a callee in machine registers,
  as Cog passes up to two (`numRegArgs`, `cogitX64SysV.c:2761`), the
  callee storing them only where it needs them.
- **The entry's poll is the depth check.** A safepoint request lowers the
  limit a compiled push compares the depth against, Cog's `stackLimit`
  arrangement ([memory.md](memory.md), "Where a collection can happen"
  already keeps it as the fallback), so a compiled call pays one
  compare for both, besides the comparison of its frame's end against
  the activation stack's committed end.
- **The count stays exact.** The goldens pin where a recursion overflows,
  so the running total of counted activations agrees with the
  interpreter's at every check. Compiled code may keep it in a machine
  register and writes it back at the exit; whether it does is road step
  8's measurement. An inlined scope is not counted at run time: at a
  call site d scopes deep the compiler adds d to the push's comparison
  and to its increment, a constant, so the count includes every
  activation a deoptimization could rebuild there.

### Between the tiers

- **A baseline frame becomes interpreted in place**: clearing its
  compiled bit, once materialized, is the whole conversion, since its
  words are the interpreter's.
- **An interpreted frame becomes a baseline one in place**, on-stack
  replacement at a backward jump whose counter trips: the frame's
  compiled bit is set and the loop enters the code at that jump's
  machine address, a back edge being a point where baseline code holds
  every value in its slot. Cog switches there and only there
  (`attemptToSwitchToMachineCode:`, `cointerp.c:14623-14735`, `:8379-8393`).
  Without it a hot loop in a method entered once -- a benchmark's main
  loop -- would never run compiled.
- **An optimized frame is deoptimized lazily**, when it would become the
  top of the stack: one below the top whose code was invalidated is only
  marked, and converted when control returns into it, as HotSpot defers
  it; the top one -- the frame whose leaf invalidated its code, or whose
  guard failed -- is converted at once. The conversion writes the k
  activations the frame stands for as suspended frames ("Suspending and
  resuming" above) and resumes the innermost, the others coming back as
  returns reach them. That is Strongtalk's eager packing into an array
  and lazy unpacking on return (`DeltaProcess::deoptimize_stretch`,
  `runtime/process.cpp:886-915`; `unpack_frame_array`, `:784-874`),
  with the suspended frame as the packed form, a form the suspend stress
  mode already runs every golden through. A frame below the top that
  must be converted at once -- only when the debugger, a stack trace or
  a mirror asks for one of its inlined activations as an object -- is
  converted by suspending everything from it up and resuming the top,
  the stress mode's own path, since the callee above lies on its slots.
- **A conversion resumes in one of two ways**: the value of the send at
  the stored program counter is delivered to that send's destination, as
  `vm::resume_caller` delivers a callee's value today -- every inlined
  caller of an optimized frame, and a frame returned into, resumes so --
  or the instruction at the stored program counter runs again, as
  `vm::force_and_reexecute` runs one today: after a failed guard and at
  a non-Boolean conditional. Strongtalk has the same two, a deoptimized
  return from a send and a redone bytecode after an uncommon trap
  (`runtime/process.cpp:716-718`, `:759-777`). Nothing resumes inside
  an instruction, and since the VM keeps every value in a register, no
  expression stack has to be mapped (`runtime/vframe.cpp:388-411` is
  what Strongtalk needs for one).

### Optimized frames

- **Metadata only at interrupt points**: every send the code did not
  inline, every poll, every guard and every instruction that can raise --
  Strongtalk records its scope descriptors at the same kind of points
  (`CodeGenerator::updateDebuggingInfo`, `compiler/codeGenerator.cpp:782-787`),
  Cog its pc map at every call's return address. At each, a chain of
  scope descriptions, innermost first, each giving its code id, its
  program counter, which of the two ways it resumes, its receiver,
  current class and operand, and for every register of its body where
  its value is: a frame word, a constant, an object to materialize -- an
  eliminated closure, indirection vector or box -- or dead, which
  becomes nil, the same deadness as the leftover analysis's
  ([compiler.md](compiler.md), "What a callee leaves"). Strongtalk's
  `NameDesc` kinds are the model (`code/nameDesc.hpp`); a value it
  cannot recover becomes nil too (`runtime/vframe.cpp:525-529`).
  Baseline code needs none of this: its registers are the frame's words.
- **Invalidation follows the dependency keys** of
  [liveness-and-reflection.md](liveness-and-reflection.md), "Caches and
  their dependency keys": optimized code records the applications and
  mixin ids its speculation rests on, and an install or a reclaim that
  changes one marks every code that depends on it and requests a
  safepoint, so no marked code runs past the next one; Strongtalk walks
  every stack and deoptimizes the frames of marked code in the same way
  (`runtime/process.cpp:917-926`). The bulk forward deoptimizes all
  optimized code, as Strongtalk's `become:` does (`prims/oop_prims.cpp:68`).
  Baseline code speculates only in its inline caches: invalidating them
  resets its call sites and touches no frame, as Cog's `unlinkAllSends`
  touches none, and code of a superseded mixin version keeps running the
  frames that run it, as interpreted code does
  ([liveness-and-reflection.md](liveness-and-reflection.md), "An edit
  replaces the mixin under its id").

### Observers

- **A walk over activations marries only what it answers.** The
  kernel's handler search steps with one leaf, `activationNextHandler:`,
  which walks the frames itself and reifies only the `on:do:` activation
  it answers, as Cog's handler search does (`primitiveFindHandlerContext`,
  primitive 197, `cointerp.c:84085-84112`), so an exception through
  optimized code converts nothing it does not stop at
  ([core-library.md](core-library.md), "Exceptions"). The search for
  unwind markers is the Rust unwinder's, which reads each frame's marker
  where it lies and reifies nothing ("Unwinding" above), so it needs no
  leaf, where Cog's `primitiveFindNextUnwindContext` serves an unwinder
  written in the image.
- **What is observed is never inlined.** `on:do:`, `ensure:`,
  `ifCurtailed:` and every kernel method whose activation a leaf reads or
  marks (`signal`, the handler's, the unwind marker's) stay frames of
  their own in optimized code, so every activation the exception
  machinery stops at is a whole frame.
- **Any other request for an inlined activation as an object deoptimizes
  its frame** -- the debugger, a stack trace, an activation mirror --
  and every write to an activation, a restart and a forced return
  deoptimizes it first. Reading one through its metadata, as
  Strongtalk's `vframe`s read (`prims/vframe_prims.cpp:67-214`), stays
  open to the debugger.

### Non-local return and unwinding

- **Machine code cuts a non-local return itself** when its target's frame
  lies in the current stretch of machine code and no activation with an
  unwind marker lies between: it pops to the target and returns there,
  as Primordial Soup cuts its stack when the home is on it and no
  unwind-protect lies between (`prior-art/primordialsoup/vm/interpreter.cc:908-1001`).
  Otherwise it takes the exit, and the one unwinder carries the unwind
  on as today ("Unwinding" above) -- so a `^` out of a `do:` block costs
  no exit. An unwind that must look into an optimized frame -- its
  target, or an unwind marker, inside it -- deoptimizes that frame
  first, which "What is observed is never inlined" makes the exception.

### Open

What the contract leaves to road steps 8 and 9 is in
[OPEN-WORK.md](../OPEN-WORK.md): where the depth count lives in machine
code, measured; and the home of a closure created in an inlined
scope, since a frame object is married only to a frame that stands for
one activation. A closure the optimizer does not inline still reifies the
frame it is created in, whatever it captured, since `enclosingActivation`
and the debugger read that link ("The closure keeps its defining
activation" above); one it inlines creates neither.

## The baseline JIT's foundation

Road step 8's first two stages are built ("Planned: the baseline JIT",
"Stages" below): the code zone and the x64 encoder, then the entry, the
exit and the instructions without a send. On x64 a body is compiled at
its second entry, and its machine code runs from its push until the first
instruction that has no template -- in most bodies their first send or
their return -- where it exits and the interpreter runs that instruction
and the rest of the body. Since no send, return or closure has a template
yet, the foundation is not expected to be faster than the interpreter,
and machine code is entered only where it can loop ("What the baseline
JIT's foundation moved" has the measurement). Only x64 has an encoder:
the JIT is compiled where `crates/ns-runtime/build.rs` sets `cfg(ns_jit)`,
on x64 alone, the one line the AArch64 port changes; elsewhere
`jit::available` answers `false` and every body stays interpreted. What the design names beyond this -- the sends and the cache
cells, closures and allocation, the exit that materializes several
compiled frames, on-stack replacement, the register cache, compacting the
zone, the AArch64 encoder -- is stages 3 to 7, still planned.

- **The code zone** (`crates/ns-memory/src/code_zone.rs`): one
  reservation of at most 128 MiB (`CodeZone::MAXIMUM_BYTES`, AArch64's
  `bl` reach), mapped twice on Windows and Linux -- an executable view
  code runs from and a writable view the zone's one `CodeWriter` stores
  through -- and one `MAP_JIT` mapping with the per-thread switch on Apple
  AArch64, so no page is ever writable and executable at once. Code is
  installed from the base up, the writer committing pages as it goes and
  flushing the instruction cache over what it wrote when it is dropped
  (nothing on x64); on Linux each zone keeps the memory object behind
  its views open for its life, one descriptor, so that a commit
  allocates its pages with `fallocate` and a zone the system cannot back
  fails as a value rather than by `SIGBUS`, on every kernel with `memfd`.
  `CodeZone::enter` calls the entry stub with the platform's C
  convention, handing it a
  block of thirty-two words -- 0 to 3 its own, 4 to 31 the caller's,
  15 to 29 of those free and kept together for the later stages' words --
  and the leaf shim calls a Rust function `fn(&mut C, [u64; 3]) -> u64`
  back.
  Every zone has an identity of its own, never 0, so that a body records
  which zone its code lies in. The native stack's bounds, which the entry
  is to check once compiled frames nest, are not read yet
  ([OPEN-WORK.md](../OPEN-WORK.md)).
- **`ns-assembler`** (`crates/ns-assembler`): the abstract operations and
  the x64 encoder (`X64`), branches sized in one pass, among them
  `jump_register`, `add_immediate`, `move_if` (a conditional move) and
  `divide_signed`; `Assembler::with_encoder` assembles with a configured
  encoder. The flag contract is stated per operation: each sets, keeps or
  clobbers the condition flags, and `Condition::Overflow` is defined only
  after an add, a subtract or `multiply_checked` -- which AArch64 is to
  honour with `smulh`, `mul`, a compare of the high word with the low
  word's sign and one `ccmp` that sets V alone. Every constant of an
  encoder is a named `const` citing its official source -- the Intel SDM's
  volume and section for x64, the instruction's developer.arm.com page
  for AArch64 -- and no integer literal but 0 and 1 stands outside one,
  which `tests/encoding_sources.rs` holds every encoder file to; every
  encoding is checked against `iced-x86`'s disassembly
  (`tests/x64_encodings.rs`).
- **The macro assembler** (`jit/macro_assembler.rs`) decides which machine
  register plays which role, the same under both x64 conventions, since
  the pinned ones are registers both have a callee preserve: `rbx` the
  address of the frame's register 0, `r14` the entry block, `r15` the
  object space's base, `r11` the idioms' own scratch, `rax` a leaf's
  answer, and nine scratch registers for the templates. `r13` plays no
  role: preserved by both conventions, it is the one free register a
  value survives a leaf call in, kept for the register cache, and a leaf
  call keeps the stack pointer it aligned away from in block word
  `LeafStackPointer` (14) instead. Its idioms are the only way a
  template touches a frame, the block, a leaf, a poll or an exit: load and
  store a frame register, load the receiver, load an immediate word, call
  a leaf (aligning the native stack at run time, as Cog aligns the C
  stack before each call into the runtime), exit, exit on a condition,
  poll, and count a back edge for the tests. In a debug build a frame
  register's load refuses a dead register's poison word as the
  interpreter's read does, by an exit of its own (`ExitKind::DeadRegister`,
  the register's index in block word `ExitRegister`, 30), after which Rust
  panics with `heap.rs`'s message for it.
- **The stubs** (`jit/stubs.rs`), at the zone's base: the entry stub saves
  every general-purpose register either convention has its callee
  preserve (no vector register: a template that uses `xmm6`-`xmm15` on
  Win64 must have the stub save them first), loads the pinned roles from
  the block, saves the stack pointer in block word `SavedStackPointer`
  (10) and calls the body. A body never returns to it: an exit **jumps**
  to the exit stub through block word `ExitStub` (12), so no `call` that
  never returns unbalances the processor's return-address predictor and
  no body holds an absolute address; the exit stub writes the program
  counter and the kind (block words 8 and 9), resets the stack pointer to
  the saved one and returns to `enter`'s caller. The running activation
  travels in block word `Activation` (13), for the leaves.
- **The baseline compiler** (`jit/baseline.rs`) lowers a body instruction
  by instruction through the one exhaustive `match` over `Instruction`
  (`lower`), recording where each instruction's code starts
  (`ProgramCounterMap`). Templates: `LoadLiteral` of a literal whose value
  is a word (`nil`, a Boolean, a `SmallInteger`, an immediate float),
  `LoadSelf`, `Move`, `StoreLocal`, `Jump`, `JumpIfNotBoolean`,
  `BranchIfFalse`, `BranchIfTrue`, and `IntegerBinary`'s `SmallInteger`
  fast path -- add, subtract, multiply, floor divide and the five
  compares, with their tag checks, declining to the send at the
  instruction's target, before anything is written, on an operand that
  is no `SmallInteger`, an overflow or a zero divisor. A backward jump
  polls the heap's safepoint request (in the machine words, block word
  `MachineWords`, 11; "The machine words" below) first and
  exits as a safepoint there, so a loop that makes no send still reaches
  one. The generic leaf, `Leaf::Step` (`jit/leaves.rs`), runs a
  straight-line instruction through the interpreter's own function for
  it -- a `LoadLiteral` that allocates, `MakeArray`, `MakeVector`,
  `LoadVector`, `StoreVector` -- and answers "re-execute" only before it
  has changed anything, so the interpreter raises the error itself. Every
  other instruction exits and the interpreter runs it.
- **The trigger and the entry** (`jit/mod.rs`): every `CompiledCode`
  carries a `JitSlot`, never encoded, which a clone answers fresh,
  counting its entries; at the second, `note_entry` compiles the body
  into the zone of the running interpreter, whose state -- the zone, 16
  MiB by default (`DEFAULT_ZONE_BYTES`, a first guess), the
  program-counter maps and the statistics (`Interpreter::jit_statistics`)
  -- is held on the `Mutator`. A push in `vm::run_stretch` pays one load
  and one compare (`JitSlot::is_armed`); everything else is
  `jit::entered`, out of line and `#[cold]`. A run's base activation,
  which the host pushed, is entered at `vm::run`'s start like any push,
  or a host's entry send would never run compiled. **A body whose machine
  code cannot take a backward jump before its first exit is neither
  installed nor entered** (`JitStatistics::bodies_leaving_before_a_loop`):
  such code runs a few straight-line instructions the interpreter runs as
  fast, for an entry and an exit on every push, which measured
  `bench_block` 31.5% and `bench_send` 21.7% slower ("What the baseline
  JIT's foundation moved"). A body the compiler refuses -- more instructions
  than a `u16` counts, code the assembler cannot lay out, no room left in
  the zone -- stays interpreted for good, and so does one entered while
  the JIT is off (`NS_NEXT_NO_JIT=1` for a process,
  `Interpreter::set_jit_enabled` for a test): enabling the JIT later does
  not revive it, so that a disabled JIT costs every later push only the
  compare. A body compiled by another interpreter, whose slot names
  another zone's identity, is compiled again into this one rather than
  entered at an offset that means nothing here. A panic inside a leaf
  never unwinds through machine code: it aborts the process at the shim,
  so a debug build's dead-register read inside a leaf ends the process
  rather than failing one test.
- **Two stress modes** run every golden through machine code: under
  `NS_NEXT_STRESS_JIT` every body is compiled at its first entry and
  entered even when its machine code leaves before a loop, and under
  `NS_NEXT_STRESS_EXIT` every poll takes its exit as if a safepoint were
  requested ([README.md](README.md), "Gates").
  `crates/ns-runtime/tests/jit.rs` pins the trigger, the zone's identity,
  a full zone and a body exiting mid-way, and `goldens/runtime/jit/` a
  counting loop and an overflow in machine code.

Since compiled code calls no compiled code yet, an entry runs exactly one
compiled frame, whose words the interpreter's push wrote: the exit writes
only where the interpreter resumes, and the native stack holds no
compiled frame's return address to walk.

### The machine words

Machine code runs while `CodeZone::enter` holds the mutator as its
context, a `&mut` protected for the whole entry, through which the shim
hands every leaf its `&mut Mutator`. A word machine code reads or writes
by itself between two leaf calls -- at an address it was entered with,
not through the shim -- therefore lies outside the mutator's bytes: an
address into them, taken before the entry, would make every such access
a foreign one to the protected reference, undefined behaviour under the
aliasing model once a leaf has written the word, though no processor
notices and Miri cannot run machine code to find it. Such words are the
heap's **machine words** (`heap/machine_words.rs`): an allocation of
their own, `Cell`s behind an `Rc` the heap holds, which Rust reads and
writes only through shared references and never borrows mutably, and
which machine code reaches through block word `MachineWords` (11). An
`Rc` rather than a `Box`, since a `Box` is retagged as unique wherever it
moves. Today they hold the safepoint request, which the interpreter's
poll and machine code's both read (`Heap::safepoint_requested`); the
depth count (`DepthCount`, which `Heap::counted_depth` reads and the
push and pop write, so that the count lives nowhere else), the depth
limit (`DepthLimit`: L, or L plus `DEPTH_RESERVE` while
`Mutator::set_depth_reserve` holds the reserve -- a reserve no longer held
but not yet forgotten leaves it at the higher value until
`depth_reserve_held` forgets it), the end of the activation stack's
ordinary room (`OrdinaryStackEnd`, written beside `FrameStack::ordinary_end`
in `grow_to`) and, for each of the five immediates, its class object as a
stored word (`ImmediateClass(index)`, rewritten by
`Mutator::rewrite_immediate_classes` after every scavenge, compaction,
move and bulk forward, since a stored word is a handle the collector does
not trace) and its dispatch id (`ImmediateGuard(index)`). Eden's top and
limit, which stage 4's bump allocation reads and writes in place, are to
join them in the room the area keeps (32 words).
The object space and the activation stack, whose words are the object
fields and frame registers machine code reads and writes, are `ns-memory`
reservations outside the mutator likewise, and their addresses come from
`Reservation::base_address`: the reservation's own pointer, obtained from
the mapping, never a slice of it. Every slice Rust borrows of the
reservation between two leaf calls is derived afresh from that same
pointer, so a write through one is a child's access, which leaves the
address machine code holds valid, and none of those slices is alive while
machine code runs. An address taken from a slice instead would be a child
of that `&mut` retag, invalidated by the next sibling's write. The
argument is written where `CodeZone::enter` is documented, at
`Reservation::base_address` and at the area's definition, since no tool
checks it.

Reading the flag through the area costs the interpreter's poll one load
more -- the area's address, then the word -- at every backward jump and
every transfer; `vm::run` measured 7,970 instructions against 7,969 in the
codegen gate, and no program slower ("What the baseline JIT's foundation
moved", "The machine words moved").

## Planned: the baseline JIT

Would change: a body that has run twice runs as machine code, for x64 and
arm64, to the contract above. Road step 8, decided with the user on
2026-10-27 after reading V8 (`prior-art/v8`), HotSpot (`prior-art/jdk`),
Cog, Strongtalk, Primordial Soup and SOMns. Its measure is not the
baseline's own speed alone: the peak of every one of those systems comes
from its optimizing tier -- Strongtalk's from inlining along the receiver
types its caches recorded, with three allocatable registers
(`prior-art/strongtalk/vm/asm/x86_mapping.hpp:28`) -- so the baseline is
built so that road step 9 finds the feedback, the frames, the assembler
and the code zone it needs, and replaces nothing of them. V8's citations
here are to `prior-art/v8/`, HotSpot's to `prior-art/jdk/src/hotspot/`,
Cog's to the generated C under
`prior-art/opensmalltalk-vm/src/spur64.sista/` (`cogitX64SysV.c` unless a
file is named).

### The foundation and the one extension point

The JIT is a foundation that instructions are added to without touching
it (the user's requirement). The foundation is every layer below the
lowering of one instruction:

- **`ns-memory`**: the code zone's reservation and its two mappings, the
  one writer through which every byte of code is written, the
  instruction-cache flush, the entry into machine code, the shim through
  which machine code calls a Rust function, and the bounds of the native
  stack. The only `unsafe` the JIT needs is here.
- **`ns-assembler`, a crate of its own**, which knows nothing of the VM: a
  set of abstract operations -- moves, loads and stores of the widths the
  heap uses, arithmetic with overflow, compares, conditional and
  unconditional jumps to labels, calls, returns -- and one encoder each for
  x64 and AArch64, with labels, branches sized in one pass (forward
  branches at their longest, backward ones exactly, as Cog's
  `generateInstructionsAt` does, `:38529-38605`), and one contract on the
  condition flags that every encoder honours: each operation sets, keeps
  or clobbers them, and says which conditions it defines. AArch64
  materializes a constant in place with a `movz`/`movk` sequence of at
  most four instructions and keeps no literal pools: the immediates the
  JIT embeds are small (tagged integers, 22-bit dispatch ids, offsets),
  so a pool would rarely save an instruction, and without one each
  operation's bytes are self-contained and the encoder trait admits
  AArch64 unchanged. It forbids `unsafe` and is tested on its own against
  a disassembler.
- **The macro assembler**, `crates/ns-runtime/src/jit/`, which writes the
  VM's idioms once in the abstract operations: load an object's guard,
  check a cache cell, bump-allocate in eden, the write barrier, the depth
  check and the safepoint poll, load and store a frame register, spill
  and fill the register cache. HotSpot's layering is the model -- encoder,
  then `MacroAssembler` with every VM idiom, then one subclass per client
  (`cpu/x86/macroAssembler_x86.hpp:44`; the interpreter, C1, C2 and the
  stubs share `profile_receiver_type`, the poll and the allocation) -- and
  V8 runs every tier on one `MacroAssembler` (`src/maglev/maglev-assembler.h:97`,
  `src/compiler/backend/code-generator.h:449`). An idiom is written once
  and every tier, the stubs and a generated interpreter if one comes
  ("Planned: JIT tiers", "Open") call it, so they cannot drift.
- **The runtime around the code**: the code zone and its compaction, the
  per-code table between machine addresses and program counters, the
  stubs (send, miss, polymorphic, megamorphic, leaf, exit), the exit that
  materializes frames, the machine-readable cache cells, the compile
  trigger and on-stack replacement.

The extension point is **one exhaustive `match` over `Instruction`** in
the baseline compiler, as `bytecode.rs`'s register-effect matches are
exhaustive, so a new instruction cannot compile until it chooses one of
three lowerings:

1. **A template**: machine code written against the macro assembler's
   idioms and the register cache's operand interface, never against an
   encoder.
2. **A leaf call**: the instruction's own interpreter step, a Rust
   function shared with `vm::run`, called through the leaf stub with the
   frame's registers flushed. Only an instruction that runs no Newspeak
   code, cannot collect and reads no other activation may take it --
   HotSpot's `JRT_LEAF` class (`share/runtime/interfaceSupport.inline.hpp:300-314`).
3. **An exit that re-executes it**: the frame is materialized and handed
   to the interpreter at that instruction, as a non-Boolean conditional
   already is ("Between the tiers" above), and comes back to machine code
   at its next back edge. Always correct, so every instruction has a
   lowering from the day it is added.

What the register cache flushes around an instruction it reads from the
instruction's declared register effects (`registers_read`,
`register_written`, `successors` in `bytecode.rs`), which every
instruction already declares; whether its lowering can reach an observer
it reads from the lowering chosen. So a new instruction adds one arm and,
for a template, the idioms it lacks to the macro assembler; nothing below
changes. A new architecture adds an encoder and the macro assembler's
per-architecture idioms; nothing above changes.

### Executable memory

- **Strict W^X on every platform.** V8, HotSpot and Cog run code pages
  read-write-execute on x64 Windows and Linux (`src/heap/memory-allocator.cc:682-697`;
  `os/windows/os_windows.cpp:3740`; Cog's
  `prior-art/opensmalltalk-vm/platforms/win32/vm/sqWin32SpurAlloc.c:190-205`), and switch per thread
  on Apple AArch64 (`pthread_jit_write_protect_np`;
  `src/common/code-memory-access-inl.h:323-332`;
  `os_cpu/bsd_aarch64/os_bsd_aarch64.cpp:515-527`). Since the caches are
  data ("Inline caches" below), code is written only when a body is
  installed and when the zone is compacted, so strict W^X costs nothing
  on the hot path. On Windows and Linux the zone is **mapped twice**, one
  view executable, one writable, as Cog's dual-mapped zone is
  (`prior-art/opensmalltalk-vm/platforms/unix/vm/sqUnixMemory.c:335-373`), which also lets road step
  9 compile on a thread of its own while machine code runs -- V8 turns
  concurrent Sparkplug off where it can only toggle
  (`src/flags/flag-definitions.h:1269-1274`); on Apple AArch64 it is
  `MAP_JIT` with the per-thread switch.
- **One reservation of at most 128 MiB**, AArch64's `bl` reach, so every
  call within the zone is direct on both architectures; the stubs sit at
  its base and never move, as Cog's trampolines do (`:10724-10737`) and
  as V8's short builtin calls rely on (`src/execution/isolate.cc:5872-5918`).
- **AArch64's instruction cache is flushed once per install**, over the
  installed range, as HotSpot batches it
  (`AArch64ICacheInvalidationContext`, `share/runtime/icache.hpp:132-157`).
  x64 needs no flush.

### The native stack carries the return addresses

The return addresses of compiled frames are on the **native stack of
the thread that runs the interpreter**, so that a call into Rust needs
no switch of stacks. HotSpot and V8 call their
runtime on the same stack their compiled code runs on, with no switch
(`cpu/x86/c1_Runtime1_x86.cpp:78`; `src/builtins/x64/builtins-x64.cc:4524`,
switching only for WebAssembly), while Cog switches to the C stack on
every call into the runtime and back (`genSmalltalkToCStackSwitch`,
`:8580-8592`). With the return addresses on the thread's own stack, a
leaf call is a plain call, and Windows' stack probes and unwinder find
the stack they expect -- Cog abandoned `longjmp` because MSVC's unwinder
"gets hopelessly confused" by a stack switch (`:8264-8275`).

- **The activation stack is unchanged**: compiled frames' words are
  there, at the interpreter's positions; the native stack holds only the
  return addresses of compiled frames above the latest entry, and leaf
  calls while they run.
- **The entry checks the native stack's room.** A counted activation
  costs eight bytes of it, so the depth limit with its reserve bounds the
  compiled frames at about 0.8 MB; the entry compares what is left of
  the thread's stack (`ns-memory` reads its bounds) against that bound
  plus the deepest leaf, and a thread with too little runs interpreted.
  `ns-next` runs programs on a thread of 8 MiB
  (`INTERPRETER_STACK_BYTES`, `crates/ns-cli/src/main.rs:160`).
- **The exit resets the stack pointer in the entry stub** to the value
  the entry saved, after the walk over the return addresses "The exit
  materializes" above describes, and returns to the run loop. No Rust
  frame lies between the entry and the compiled frames except a leaf
  call's, which has returned before any exit.
- **A panic never unwinds through machine code**: a leaf returns its
  failure as a value, and a panic in one aborts.

### Fixed registers and the calling convention

The internal convention is ns-next's own; only the entry and the leaf
shim cross the platform ABI. Pinned in callee-saved registers while
machine code runs: **the frame base** on the activation stack, **the
mutator's context** (HotSpot's `r15_thread`, Cog's `VarBaseReg`, `:351`),
and **the object space's base**, since a handle is a byte offset and
every field access is base plus offset (HotSpot's `r12_heapbase`, V8's
`r14` pointer-compression base, `src/codegen/x64/register-x64.h:360-391`).
A send passes the receiver and up to two arguments in machine registers
as well as in their slots' places, the callee storing them only where it
needs them (Cog's `numRegArgs`, `:2761`). Which machine register plays
which role is the macro assembler's, per architecture.

### Inline caches

The caches stay data, shared with the interpreter; machine code reads
and writes the same sites at the same `feedback_base`, so road step 9
reads one profile whichever tier ran, as Maglev reads the feedback vector
Ignition and Sparkplug share (`src/maglev/maglev-graph-builder.cc:6503-6564`;
`builtins-x64.cc:1886-1929`). HotSpot has moved its compiled inline
caches out of the code into data (`CompiledICData`,
`share/code/compiledIC.hpp:57-97`); Cog keeps them in code and so scans
the whole zone to unlink every send to a method on every install
(`unlinkSendsToandFreeIf`, `:12523-12610`), which live editing would
make frequent. "A cache records what the lookup decided; it never
decides" holds unchanged.

- **Every site gets a machine-readable cell** beside its `CacheEntry`: the
  guard and an entry address, written whenever the Rust side fills,
  widens or clears the entry. The entry address is the callee's machine
  entry for a method with code, else a stub that acts on the entry's
  `Target` ("Sends in machine code" below).
- **The cells lie in a reservation of their own** (decided with the user
  on 2026-10-28), an `ns-memory` reservation parallel to the site table
  and the polymorphic pool, committed as they grow, so a site's cell is
  at a fixed address -- the table's base plus the site's index -- while
  `sites`, a `Vec` inside the mutator, moves when it grows, and machine
  code may read no word inside the mutator ("The machine words" above).
  A heap object, V8's feedback vector, would cost tracing and object
  space for nothing a cell holds: a cell names no handle. Cog, Strongtalk
  and HotSpot keep the state in the call instruction's target, which
  "Executable memory" above rules out.
- **A send in machine code** loads the receiver's guard (the header
  word's low 22 bits, one load), compares it with the cell's, and calls
  through the cell's entry address; a mismatch calls the polymorphic
  stub, which scans the site's block of four (Strongtalk's compiled PIC
  holds four, `code/compiledPIC.hpp:36`; Cog's six, SOMns's chain six),
  and past that the megamorphic stub probes the global cache from
  machine code, as Cog's open PIC probes the interpreter's method cache
  (`:29031-29170`). A miss calls the lookup as a leaf, which fills the
  entry and the cell.
- **A lexical, `self`, `outer` or private send checks no receiver
  guard** where the resolution does not depend on the receiver's class,
  as SOMns's `LexicallyBoundDispatchNode` calls directly
  (`prior-art/somns/src/som/interpreter/nodes/dispatch/LexicallyBoundDispatchNode.java:17-35`).
- **Measured, not decided: an indirect call through the cell or a
  patched direct call.** V8's Sparkplug+ and HotSpot patch only a call's
  target, monotonically (`src/ic/ic.cc:851-869`, `:899-920`;
  `share/code/compiledIC.cpp:190-255`); patching writes code, so it costs
  a write through the second mapping and a flush on AArch64. The
  baseline starts with the indirect call and measures; Strongtalk's
  "10%" for a jump-table hop (`code/compiledIC.hpp:58-59`) is from 1995.
- **No counts in the cache entries** in road step 8. HotSpot's C2 inlines
  on receiver frequencies (`share/opto/doCall.cpp:258-350`) and pays about
  30% for full profiling (`share/compiler/compilationPolicy.hpp:129-131`);
  Strongtalk inlined on its caches' contents and callee invocation counts
  alone (`compiler/rscope.cpp:318-355`). Whether road step 9 needs counts
  is decided there.

### Sends in machine code

Stage 3, decided with the user on 2026-10-28 after reading Cog,
Strongtalk, V8 and HotSpot again for it.

- **`SendPublic`, `SendLexical` and `SendSuper` call through their
  cells.** A send whose call site is `CallSite::UNCACHED` -- every send
  of the reference lowering, `SendInlinable` among them, which only that
  lowering emits -- has no cell and takes an exit, so the oracle stays
  interpreted at its sends.
- **A callee with no machine code is compiled by the miss.** The leaf
  that fills an entry counts the callee's entry as a push would and
  compiles it at its second, as Cog's `ceSend` compiles a method its
  send finds in the method cache (`cointerp.c:16708-16807`); only a body
  the compiler refuses leaves the call to an exit, which the run loop
  turns into an interpreted push. So installing code no longer needs the
  zone unshared: it is installed while machine code runs, which never
  holds an address an install moves.
- **A target that is no method with code** is reached through a stub
  chosen from the entry's target alone, when it is filled: a slot getter
  loads the field; a constant, the receiver or a parameter answers it --
  Cog's quick methods (`genQuickReturnConst`, `genQuickReturnInstVar`,
  `genQuickReturnSelf`, `cogitX64SysV.c:30418-30463`); a slot setter and
  a primitive whose mirror row is leaf-safe run their interpreter step
  through the leaf shim, the setter until stage 4 emits the write
  barrier; every other target -- a block-activating fast path, a nested
  class, a factory, a `doesNotUnderstand:` handler, an unsafe row --
  takes an exit, and the interpreter runs the send. The stub is chosen
  from the target because the target is immutable: a `MixinMethod` is
  never changed in place, so the fast path it carries is the one a fresh
  read would find.
- **`SendMirror` runs a leaf-safe row through the leaf shim** and exits
  for every other row. A row is leaf-safe when its leaf answers a value
  or declines before it changed anything, runs no Newspeak code, pushes
  nothing and reads no other activation; each row says so, and the
  default is the exit.
- **The depth count and the activation stack's committed end are machine
  words** (decided with the user on 2026-10-28): a compiled push compares
  the count with the limit and the frame's end with the committed end,
  one compare each, and exits when either fails, so the interpreter
  raises the overflow or grows the stack. The innermost frame and the
  number of live frames are derived by the exit's walk. Whether the
  count lives in a machine register is stage 5's measurement.
- **The megamorphic stub probes a machine-readable mirror of the global
  cache** beside the cells, written wherever the global cache is, as
  Cog's open PIC probes the method cache (`cogitX64SysV.c:29031-29150`),
  Strongtalk's megamorphic stub its lookup cache
  (`prior-art/strongtalk/vm/code/stubRoutines.cpp:315-435`), V8's
  megamorphic load its stub cache and HotSpot's vtable stub the vtable;
  a probe that misses calls the lookup as the miss does.

### The register cache

Since a body's registers are frame slots, the baseline caches them in
machine registers within a basic block and writes them back before every
send, poll, merge point, exit and leaf call -- Cog's simulated stack and
`ssFlushTo:` (`:40797-40860`, `:40924-40990`, `:38825-38840`), which keeps
the contract's "every compiled frame's registers hold every value an
observer can need" true by construction. V8's Sparkplug keeps nothing but
its accumulator in a register (`docs/compiler/sparkplug/compiler-sparkplug.md:17`)
and HotSpot's C1 keeps nothing across a call
(`share/c1/c1_LinearScan.cpp:1254-1286`). Built in two measured stages:
templates that read and write the slots, then the cache.

### When a body is compiled

- **Synchronously, at its second entry**, as Cog compiles on an
  interpreter method-cache hit (`cointerp.c:5880-5905`); V8 compiles
  Sparkplug code at about eight invocations weighted by length, in
  batches (`src/execution/tiering-manager.cc:158-170`). The counter is
  data on the compiled code, not in machine code. A body that cannot be
  compiled -- past a size the zone's layout bounds -- is marked and never
  tried again.
- **On-stack replacement at a back edge** whose per-frame counter in the
  flags word trips, as Cog's 40 back jumps (`cointerp.c:2759`,
  `:8379-8409`); "Between the tiers" above says how.
- **Compilation on a thread of its own** is road step 9's; the second
  mapping keeps it open.

### The code zone

Machine code holds no handle, so nothing of it is patched or relocated by
a collection -- V8 relocates embedded objects and pins pages with running
code (`src/heap/mark-compact.cc:6339-6364`), HotSpot rewrites immediate
oops (`share/code/nmethod.cpp:2073-2084`), Cog keeps a list of methods
referring to young objects (`:9664`). Literals are loaded through the
heap's tables. The zone is compacted only at an exit, when no return
address into it exists, so moving a body updates only the cache cells
that name it. Cog's policy decides what is freed: code with a frame on
the activation stack stays, the rest by usage, until a quarter of the
zone is free (`freeOlderMethodsForCompaction`, `:13099-13132`). Code
dies with its mixin's row when a full collection reclaims it
(`compiler.md`, "Readers of compiled code take the mixin"). Cog's default
zone is 2.7 MiB (`:31554-31559`); the default here is chosen by
measurement within the 128 MiB bound.

### Polls and the depth check

The entry's depth check is the poll ("Safepoints and the depth limit"
above). A back edge compares against the same request word, one compare,
as Cog's backward jumps check `stackLimit` (`:38851-38858`); V8 polls at
a back edge only when its budget runs out
(`src/interpreter/interpreter-generator.cc:2520-2521`). Whether the depth
count lives in a machine register is measured.

### What the templates emit inline

Cog's machine code proves each: `SmallInteger` arithmetic with tag checks
and an overflow fallback (`genSpecialSelectorArithmetic`, `:39772`),
compares fused with the following branch (`:35512`), the Boolean test of
a conditional -- one compare each here, `true` and `false` being
immediates -- bump allocation in eden that requests a scavenge past the
threshold and carries on into the slack, the collection waiting for the
next safepoint (`genCreateFullClosure…`, `:19554-19620`), and the write
barrier with its early outs -- an immediate value, a young destination, an
old value, an already remembered object (`:21739-21780`).

### Tests

- **A seventh stress mode compiles every body at its first entry**, so
  every golden runs through the JIT; an eighth takes the exit at every
  poll, so every golden runs through materialization and resumption.
- **The encoders are tested against disassemblers**, dev-dependencies of
  `ns-assembler` only: `iced-x86` for x64 and one for AArch64.
- **AArch64 runs under QEMU**: the suite cross-compiled for
  `aarch64-unknown-linux-gnu`, run by `qemu-aarch64` in WSL2. QEMU does
  not model an incoherent instruction cache, so a missing flush passes
  there; Apple's `MAP_JIT` path is checked by clippy only.

### Stages

Each measured against the tree before it: the code zone and the x64
encoder; entry, exit and the instructions without a send, every body
compilable since an instruction without a template exits; sends and the
caches; closures, non-local return, allocation and the barrier;
on-stack replacement, the polls and the depth count's measurement; the
register cache; the AArch64 encoder under QEMU. The first two are built
("The baseline JIT's foundation" above); the third, sends and the caches
in machine code, is next ([OPEN-WORK.md](../OPEN-WORK.md), "Next").

## Decisions

- **One engine.** The tree-walker was retired because it could not represent a
  suspended computation; golden outcomes protect behaviour, and the reference
  lowering is the differential oracle.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, so the reference lowering and a JIT share it. A send is split into a
  decision and an activation, which preserves this: the decision stays in
  `interpreter/`, and only running what it decided is the loop's.
- **`program_counter` is stored only at observable points**, not at every instruction — a
  deliberate hot-path trade the debugger milestone may have to revisit.
- **Activations are stack frames, reified on demand**, because the
  always-reified model cost about 41% of a send, before the arena, for a structure
  most activations never expose. The arena took the `Rc` box and the `RefCell`
  borrow, about 15% of `bench_loop.ns`. The stack took the two vector allocations
  and the reification of every activation nothing captures. Measured against the
  arena in one sitting, a user-method send fell from ~146 ns to ~91 ns, and
  `bench_send.ns` from 23.5 ms to 20.5 ms. Peak working set
  over `bench`'s twenty-two entry sends of `bench_loop.ns` fell from 340.7 MB to
  4.6 MB.
- **A live activation and a reified frame are two handle types**, `ActivationReference`
  and `FrameReference`, so that storing a stack handle is a compile error and a
  register access never branches on which kind it holds.
- **A returned activation has no sender**, as in Smalltalk, Cog and Primordial
  Soup. A live one's sender is the frame below it, by construction, or the
  suspended frame it returns to. The `sender`
  parameters that once threaded through every send and block invocation were
  removed rather than retyped, because once the sender is structural nothing
  read them.
- **Only by-reference code keeps a frame's registers alive.** The reference
  lowering and a fallback closure reach their method's variables by reference,
  and a reified frame keeps its registers when its activation returns only when
  a block it created can do that (`outer_reach`, "Converted
  representation"); converted code copies or shares a vector instead. The
  reification mechanism itself is permanent: `^` and the debugger need it with
  or without conversion.
- **Sends run on the activation stack, not the Rust stack**: one loop pushes,
  pops and resumes, and it pops everything it runs on every path out.
- **Control flow answers trampoline steps and depth has one bound**,
  L = 100,000 activations, counted alike under both lowerings, because a
  callback native would make the reference lowering's native stack grow with
  source nesting; with no re-entry from Rust left, nothing else can run out
  first ("The depth limit"). It keeps a reserve of 1,000 above it for
  signalling the errors raised near it (decided with the user on
  2026-09-25), held by a frame, not a stack index.
- **One unwinder, aimed at a frame** (decided with the user on 2026-09-25,
  "Unwinding"): non-local return, the exceptions' `return:`, `retry` and
  `resume:`, and termination all go through `interpreter::unwind`, which
  runs `ensure:` and `ifCurtailed:` blocks as trampoline steps rather than
  calling back into Newspeak, and whose target is a `FrameReference`,
  because code runs while an unwind is in progress and a release build
  does not check an activation's generation.
- **Rust does not re-enter the run loop** (decided 2026-09-13, "No re-entry
  from Rust"): the nested-class accessor is a trampoline step, `do:` a
  Newspeak loop since the loops' real bodies (decided with the user on
  2026-10-27, [core-library.md](core-library.md), "Real bodies for control
  flow and `do:`"), and instantiation is compiled code since `new` is (decided with the user
  on 2026-10-22, [object-model.md](object-model.md), "Decisions"), so that
  no Rust frame is live below a safepoint and the root discipline has
  nothing to root but the host's values. The bound on re-entries went with
  them.
- **A superclass chain nests one activation per level with code**
  (decided with the user on 2026-10-22, "A superclass chain nests one
  activation per level"), and a chain deeper than the depth limit
  overflows, where a Rust driver flattening it would be the walk compiled
  `new` replaced.
- **The run loop's stretch is forced inline** (2026-09-16 for the
  per-instruction step it replaced, confirmed by the user 2026-09-17), since
  the inline caches' hit path makes it too large for the inliner to take on
  its own, and a step out of line cost every instruction a call; forcing it
  alone measured no faster ("What the inline caches moved").
- **The run loop runs over the frame's state in locals, one entry per
  stretch between two safepoints** (designed with the user 2026-09-28, built
  the same day; "The loop over locals"): no per-instruction boundary and no
  per-instruction answer, Cog's `localIP`/`localFP` rather than Primordial
  Soup's interpreter fields, since locals of one function are what LLVM keeps
  in registers ([dispatch.md](dispatch.md), "Decisions").
- **A body is stored and run as eight-byte instructions** (decided with the
  user on 2026-10-02 on a measured gate, built the same day; "The compact
  instruction encoding"): one `#[repr(u8)]` slot per instruction and side
  tables for what does not fit, so the program counter stays an instruction
  index and the wire format, the statement map and a frame's
  `program_counter` are unchanged; chosen over a 32-bit or byte stream,
  which would make the program counter an offset, and over a stack form,
  which would undo the register assignment. Each send kind joins its own
  send path, since one join shared by the four cost `bench_loop.ns` 3.5%.
- **The run loop polls a safepoint at every switch and every backward jump**
  ("The VM"): one compare of a flag the heap owns, and when it is set the
  stretch leaves its entry and `run` makes a cold call, at the points where
  what the loop carries holds no handle a move would change
  ([memory.md](memory.md), "Where a collection can happen"). The slow path
  serves the request by clearing the flag, which is also what keeps the
  optimizer from deleting the poll.
- **A nested class's creation is marked in its own cache slot** ("No re-entry
  from Rust"), not listed beside the stack: the slot is keyed by exactly the
  pair a cyclic definition is, so the check is the read the accessor makes
  anyway, where the list was scanned in full at every level of a deep chain of
  creations.
- **Closure conversion keeps the defining activation** (decided 2026-09-19,
  "Closure conversion"): a closure keeps `captured_frame` and only
  variable access moves to copies and vectors, because Newspeak's
  `enclosingActivation` and the debugger's "`[] in Foo bar`" need the link;
  dropping it for blocks without `^`, as the JVM does, saves one reification
  per block created and was rejected, as was keeping it only under a
  debugging flag, which a closure created without it could never recover.
- **A captured mutable variable is copied unless it is written after
  capture** (decided 2026-09-19): Primordial Soup's rule, plus a rule for
  writes inside an inlined loop that contains the capturing block — the
  performance-oriented choice over boxing every captured variable that is
  written anywhere, and made more important by step 6, which runs the core
  library's blocks per element.
- **One indirection vector per scope, as an ordinary `Array`** (decided
  2026-09-19), as in Cog and Primordial Soup, rather than a box per variable;
  no inline small-vector representation for now.
- **The capture analysis is the optimized lowering's**, not the resolver's
  (2026-09-19): only the compiler knows which blocks are inlined.
- **The reference lowering keeps the by-reference model as an interim
  oracle** (decided 2026-09-19): it is to be removed at some point, and from
  then on the recorded golden outcomes are how closure conversion, and
  everything else, is tested.
- **A fallback closure keeps the by-reference model** (decided 2026-09-19,
  "Fallback blocks"): the real closure an inlined `ifTrue:`, `and:` or `or:`
  sends when its receiver is not a Boolean reads and writes its frame's
  registers by reference, its writes count as after every capture, and the
  divorce copy is decided per activation by whether it created such a
  closure — so the common path pays nothing and inlining stays unobservable.
- **A send's own callee lies on its sender's argument slots** (decided
  with the user on 2026-10-24, "Overlapping register windows"): Cog's
  layout, the arguments, then the header, then the other registers, the
  header's numbers left out of the register numbering and 13 words in
  every build, so that a caller's outgoing arguments are its callee's
  first registers and a debug build's artifact runs in a release one.
- **Neither a push nor a pop writes a scratch register** (decided with
  the user on 2026-10-25, "What the leftover analysis moved"): the
  leftover analysis, derived from the code, says what a frame may read
  before writing it, the decoder refuses image code that reads it, and
  the collector nils it in its trace of the stack, rather than the pop
  filling what its callee gave back, which cost 1-2% of a run, or
  liveness maps declared in the image, which a flat send of inlined
  conditionals makes quadratic.
