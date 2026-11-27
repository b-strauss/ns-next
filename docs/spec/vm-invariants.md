# VM invariants

The internal rules of ns-next's virtual machine that no program observes
as such, but whose breach would make a rule of the other parts false --
a slot that answers another object, a weak entry that outlives its key,
a send that runs the wrong method -- most often without an error and at
an exit status of 0. Each rule says which rules of the language, the
run-time semantics, the bytecode or the image format it protects, and
what finds its breach: the tests that name it, the detectors of the
last section, or, where nothing can, a line `Undetected:` saying why.
The rules are stated over the abstract machine the next section
defines and name no part of the code; the design documents say where
each rule is enforced and point at it by its identifier, and keep its
reasoning, its mechanism and the defects it has produced.

## The model

The **interpreter** runs bytecode: its **run loop** fetches each
instruction of the running activation's body and executes it. A **host**
is the program that embeds the virtual machine; it enters the run loop
only through the one **host entry**, and each entry is a **run**. A
send's **decision** -- the lookup and what it answers -- is made before
the callee is pushed. Where a routine of the virtual machine would have
to run a block or a method, it answers instead a **step** the run loop
schedules. The **bootstrap** builds the kernel's classes before anything
runs; the **shipped modules** are the core library the binary embeds,
the **kernel** first among them. The terms of the language and the
run-time semantics -- a factory, a lazy slot, an ephemeron and its
finalizer, the handler search, a module's version, a breakpoint, an
evaluation at a stop -- keep the meaning those parts give them.

The **object space** is the one region of memory every heap object lives
in. An **object** is a sequence of **words**: a header word, which holds
its format, its size in words, its dispatch id and the collector's bits;
its class word, which names its class object; and its fields. The
**format** decides which words are pointers the collector reads, which
are weak, and which are numbers or bytes it never reads as pointers. A
value is one tagged **word**: an immediate small integer, an immediate
float, an immediate character, or a pointer to an object. Every region
of the virtual machine -- the object space, the activation stack, the
cells, the tables of machine code, the code zone -- lies in a
**reservation** of address space, of which only a prefix is
**committed**, backed by memory; a region grows by committing more and
gives memory back by decommitting it. A **boxed float** is a float
outside the immediate range, held in an object of its own.

A **handle** names an object, or a frame on the activation stack, by its
offset from the start of the space it lies in. It means something only
against the heap that minted it, and only until the next move of what it
names. A **root** is a place outside the object space that the collector
enumerates and every move rewrites; a host keeps a value across a run
only in one. Every move advances the heap's **move epoch**. Inside an
entry a handle carries the **brand** of that entry, which keeps it from
leaving it; at rest, between entries, a value is held only in a root.

The **activation stack** is a region of its own holding one **frame**
per running **activation**: a header -- its body, its receiver, its
current class, its sender, its program counter -- and its registers. An
activation is named by its offset on that stack, which a later push
reuses once it is popped. A **frame object** is an object of the object
space that stands for an activation: made on demand (*reified*) when a
closure, a frame reference or the debugger needs one, **married** to its
activation while that runs, **divorced** -- given its registers -- when
the activation is popped, and **suspended** when the activation is
copied into it and taken off the stack to be resumed later. A
**continuation** is what a step the run loop scheduled carries on with
when the activation below it is next resumed. The **depth reserve** is
the room past the depth limit kept for the handler that runs when the
limit is reached.

An activation's registers begin at its **register base**: its arguments
and the locals its body declares lie below the body's local count, the
**scratch registers** above it, and its **program counter** names the
instruction it stands at. A **side table** lies beside the activation
stack and holds rows its frames name: the continuations' rows, the
suspension table of suspended activations, and the rows that hold what a
frame's header does not. The **leftover analysis** computes, for each
program counter of a body, the registers the frame has not written since
its push, which may hold a word a callee or a popped frame left: those
registers are **tainted**, and a tainted register is dead. A block's
**home** is the activation of the method it was made in, which its
non-local return returns from; an **unwind marker** marks an activation
an unwind stops at -- an `on:do:`, `ensure:` or `ifCurtailed:`. The
**unwinder** is the one routine that carries out every unwind -- a
non-local return, an exception's return or retry, a debugger's restart
-- and the **depth count** is the number of activations the depth limit
is held to, from which a few block activations are **exempt**. The
**native stack** is the processor's stack, on which the host, the run
loop and compiled frames' **return addresses** lie.

The object space holds a **nursery** -- **eden**, where new objects are
allocated, and two **survivor** spaces -- below the **old space**. A
**scavenge** copies the nursery's live objects into a survivor space or,
by age, into the old space (*tenures* them), leaving a **forwarder** at
each place it vacated. A **full collection** marks every live object of
both generations, settles the weak objects and slides the survivors down
into the old space. A **move** is any rewrite of an object's offset: a
scavenge, a full collection, or the move stress mode's shift. The
**remembered set** lists the old objects that may name a young one, so
that a scavenge, which reads only the nursery and the remembered
objects, finds every young object an old one reaches. The **write
barrier** is the decision every store into an existing object makes to
keep that set exact. The **bulk forward** is the one operation that
rewrites every reference to a group of objects to name others, behind
`become:` and every install of an edit. New objects are allocated by the
**bump cursor**, which moves eden's top up towards its **limit**;
machine code allocates inline only below the **fast limit**, which lies
below the **scavenge threshold**, the top past which an allocation
requests a scavenge. Each object's header carries a **mark bit**, set
while a full collection marks it, and a **remembered bit**, set while it
is in the remembered set. The **forwarding table** maps, chunk by chunk,
each survivor of a full collection to its new offset. An ephemeron whose
key nothing else reaches is **mourned**: queued for its finalizer. The
**canonical table** holds the one symbol object of each interned text,
and a **weak table** holds the symbols a program makes until the
canonical table adopts them; a **reified mixin** is the object that
stands for a mixin in reflection.

A **safepoint** is the point between two instructions of the run loop --
at a switch to another activation and at a backward jump -- where the
loop serves what was requested of it: a collection, a debugger's stop, a
compilation, a stress mode's action. Nothing else collects, moves
objects or stops the run.

A **mixin** is a class declaration's compiled behaviour; a **mixin
application** is a mixin applied to a superclass, shared by every class
object of that shape, and a **level** is one application in a class's
chain. A **guard**, or **dispatch id**, is the number of an application
that every instance's header carries and every cache compares. A
**body** is a compiled method, block or initializer; it belongs to a
**family**, the body and the blocks in it, whose sends are numbered and
whose caches lie at its **feedback base** in the heap's cache space. A
**code id** names a body in the per-heap table of compiled code. A
nested class's class object is cached in a **hidden slot** of its
enclosing object, which marks it while the class is being created. A
family's **family size** is the number of its send sites. An install of
an edit replaces a mixin by a new version under its id, the old one
**superseded**. The **code table** holds the heap's compiled bodies by
code id, and the **application id table** holds each application's row
for machine code.

The **compiler** lowers a resolved module's bodies into instructions, by
one of two **lowerings**: the **optimized lowering**, which inlines
control flow and caches its sends, and the **reference lowering**, which
emits every send uncached and is the oracle the optimized lowering is
held to. A body is stored and run in the instructions' **eight-byte
form**, with a **wide form** for an operand past its range. A body's
**debug tables** -- its statement table and its variables' metadata --
say where its statements begin and which register holds which variable.
An application's **slot base** is the index of its level's first own
slot in an instance; a mixin's instructions name its own slots relative
to it. Before it, the **resolver** binds each name of the module -- to a
slot, a lexical send, a send to the VM mirror -- and the compiler lowers
the bindings it recorded. An **own-slot instruction** reads or writes a
slot of the receiver by its index from the slot base. A block that
captures a local its home may write after the capture reaches it **by
reference**: in the reference lowering and in a **fallback closure** --
the closure an inlined conditional makes when its receiver is no boolean
-- through the home's registers, whose depth is the instruction's
**outer reach**; in **converted** code, a captured local is copied into
the closure or shared through a **vector**, an object holding it.

A **cache entry** records, for one send site and one key -- the
receiver's guard, and whatever else the lookup's answer depends on --
the answer the lookup gave; the **global cache** does the same for sends
that have no site of their own. The **mutator** is the state the run
loop steps -- the heap, the activation stack, the caches -- which one
party at a time holds exclusively: the run loop while it runs machine
code, a leaf while machine code calls it. A **cell** is the
machine-readable form of a site's entries that machine code calls
through. A **leaf** is a routine of the virtual machine that machine
code or the interpreter calls to do one thing a send or an instruction
asks -- an arithmetic operation, an allocation, a mirror operation --
and that runs no Newspeak code. A **stub** is a piece of shared machine
code that a cell or an instruction jumps to: a polymorphic or
megamorphic dispatch, an accessor, a closure's call, the entry and the
exit.

A **primitive method** begins with a leaf and runs its body only when
the leaf declines; a **quick method** answers a constant, a slot or the
receiver. Both carry a **fast path** a send takes without pushing an
activation. A **verified slot-level pair** records that a receiver's
application was verified to have a level, so its slot accesses there
need no further check.

Machine code lives in the **code zone**, one region mapped twice, an
executable view and a writable one. A body compiled by the baseline
compiler has a **lean** copy and, where it makes a closure, a **reifying
twin**, whose frames may be reified; a body optimized has **optimized
copies**, one per receiver application and level it was entered at, each
with its **interrupt points**, where the frame it leaves matches the
interpreter's word for word. The **machine entry table** names, per code
id, the machine code a closure's call jumps to; a method body's
**dispatching rows** name, per receiver key, the optimized copy a
dispatching stub jumps to. The **entry block** holds the words that
every entry into machine code reads and that do not change between
entries; the **machine words** are the heap's further words machine code
reads and writes -- the safepoint request word, the depth limit -- some
of them set from another thread. An **exit** hands a frame running
machine code to the interpreter at the instruction it reached; an exit
at any depth **materializes** every compiled frame above it as the
interpreter's frames. The **cut** carries out a non-local return in
machine code without an exit.

The **baseline compiler** compiles a body's instructions one by one into
**baseline code**, which keeps every value in the frame. A **poll** is
the check of the safepoint request word machine code makes at a backward
jump, taking its exit when a safepoint is requested. A body's **data
words** lie in the code zone beside its code; among them its lean copy's
**transfer words**, which name its twin, and its **budget word**, which
names the body's row of **optimization budget**, the count its baseline
code spends until it requests an optimized copy. The optimizing compile
reads a **snapshot** of the body and of its callees, builds an **SSA
graph** -- whose **frame nodes** store values into the frame -- inlines
callees as **inlined scopes** behind a **type-case** on the receiver's
application, and records at each interrupt point the **write-backs**
that make the frame the interpreter's; a guard that fails **traps**,
leaving the copy for the baseline code. A **stretch** is the run of
compiled frames machine code pushed since it was entered, at its **entry
frame**. A body that keeps exiting is **abandoned** -- its code freed --
and **revived** later. A method body's **dispatching word**, among its
data words, names its block of dispatching rows. An optimized copy
records its **dependencies** -- what it inlined and what it assumed --
and is **invalidated** when one changes, **retired** when it keeps
trapping or exiting; a cache's **executed** and **trapped** bits say
whether its site ran and whether a guard there failed. The code zone is
**compacted** -- its live bodies slid together -- when a compile finds
no room.

The **VM mirror** is the one object through which the shipped core
library reaches the virtual machine's leaves. The **image loader**
decodes an image, which is untrusted input, into a heap; each record's
object is first allocated as a **placeholder** and filled in place. A
**stress mode** runs the suite with one action of the virtual machine --
a move, a collection, an exit -- forced at the safepoints it picks; a
**debug build** adds checks a release build leaves out. A value the
debugger **withholds** is never handed out for a client to act on. A
**trace event** or a **listing** of a body's code is queued where it is
recorded and delivered to the host's sink at a flush. A **mirror send**
is a send to the VM mirror, which only the shipped modules can make;
each selector the mirror understands is a **mirror row**, carried out by
its leaf. The image **encoder** writes an image and the **decoder**
reads one; a **module instance** is the object a module's root class
makes for a program. The **embedded code** is the shipped modules'
compiled code the binary carries, with a **fingerprint** of the sources
it was compiled from.

## The heap and the collector

r[vm.heap.collection-only-at-safepoint]
A scavenge, a full collection and every move of objects run only at the
run loop's safepoint -- polled at a switch to another activation and at
a backward jump -- or between entries; an allocation, a table's growth,
a leaf, a send's decision and a stretch of instructions at most request
one. A send's decision therefore reads its arguments in place in the
sender's registers, which nothing moves or pops before the push.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash),
[`runtime.send`](runtime.md#r-runtime.send),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order)

r[vm.heap.no-native-frame-below-safepoint]
The run loop is entered only from outside it, by the one host entry: no
decision, leaf, step or continuation re-enters it, the safepoint or a
host's send, so between two instructions the native stack holds the host
and the run loop and nothing between them, no native frame holds a value
across a safepoint, and the roots beyond the activation stack are the
host's values alone. A block a leaf would run is answered as a step the
run loop schedules.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search)

r[vm.heap.roots-complete]
Every place outside the object space that holds a handle or a stored
word is enumerated to the collector and rewritten by every move: the
bootstrap roots, the depth reserve's frame, every word of the activation
stack below its top, the side tables of continuations and of suspended
activations, the host's root table, the frame class, the remembered set
(for a scavenge) and the weak tables (forwarded, not marked); marking,
forwarding and the scavenge reach the mutator's roots through one
enumeration. A continuation's row is freed only by the pop of its
activation or moved into a suspension's row and back, and a suspended
chain nothing else names stays alive while its rows remain.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`semantics.exception.resumable`](language/semantics.md#r-semantics.exception.resumable),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.heap.format-decides-pointer-words]
An object's pointer words are named by one enumeration decided by the
format in its header: forwarding, every move and the remembered set's
verification read all of them, a weak word included, and marking and the
scavenge read them less only the weak words; no other enumeration of an
object's pointer words exists, so a word one collector reaches the other
reaches, and a weak word of a live object is rewritten wherever its
referent moves.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.non-pointer-words-untraced]
The collector never reads as a handle a word that is not one: an
arbitrary-size integer's digits, a boxed float's bits, a string's code
points, a byte array's bytes, the header word, and the numbers an object
carries (a closure's code id and feedback base, a class object's builtin
tag, applied clause and application, a frame's numbers).

Protects:
[`semantics.number.exact`](language/semantics.md#r-semantics.number.exact),
[`semantics.literal.float`](language/semantics.md#r-semantics.literal.float),
[`semantics.literal.string`](language/semantics.md#r-semantics.literal.string),
[`semantics.literal.integer`](language/semantics.md#r-semantics.literal.integer)

r[vm.heap.mark-bit-clear-outside-collection]
An object's mark bit is set only while a full collection runs and is
cleared, with its age, as the compaction moves each survivor; outside a
collection no object is marked.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.forwarding-table-exact]
At the start of every full collection every chunk of the forwarding
table outside the occupied ranges is clear, so a survivor's new offset
counts only this collection's live words; the slide moves the old
space's survivors down and the nursery's above them in address order, so
no copy overwrites a survivor not yet moved, and every forwarded handle
names a survivor.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash)

r[vm.heap.resolved-future-replaced-in-fields]
Once any future exists in a heap, a full collection, before it marks,
rewrites every object field holding a resolved future -- an instance's
or array's field, a weak word, an ephemeron's field, a closure's copied
value -- to the future's value, never a register, a frame object's
register or a host root; a resolved future's value is never itself a
future, and a future whose state words are no well-formed future's is
never seen through. A scavenge and a move replace nothing.

Protects:
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous),
[`semantics.value`](language/semantics.md#r-semantics.value),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.heap.resolved-future-judged-by-value]
In both collectors a resolved future held in a weak word or as an
ephemeron's key stands for its value: the entry lives exactly as long as
the value does, never as long as the proxy, an unresolved future is
judged as itself, and both collectors apply one test of what a resolved
future is.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron),
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)

r[vm.heap.id-reclaimed-only-when-unnamed]
A full collection reclaims a per-heap id -- a guard, an application, a
module's tree of mixin ids, a superseded mixin version, a code id, a
cache range -- only between marking and compaction and only once nothing
names it: no marked object, header, cache entry or recorded guard, no
running or suspended activation (its entered level counted), no reified
mixin or host holder, no optimized copy that inlined the body and no
breakpoint that patched it; a superseded version stays while a closure,
a frame or an activation runs its code, and a class's change of
application restamps every instance before its old guard can be freed.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.install`](runtime.md#r-runtime.install),
[`image.write.edited`](image-format.md#r-image.write.edited)

r[vm.heap.barrier]
Every store of a word into an existing object, by the interpreter or by
machine code, a store into a weak slot included, makes the barrier's
decision: an old holder that comes to name a young object is remembered.
Only an initializing store into an object just allocated skips it;
besides those only the collector's own rewrites, the bulk forward (which
remembers every old holder that names a young object after its rewrite)
and the replacement of resolved futures (right before a full collection)
write words without it.

Protects: [`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.frame-copies-remembered]
A divorce or a suspension that copies registers into an old frame
object, and a write into a divorced or suspended frame's register,
remember the frame when a copied or written word names a young object; a
married frame's registers lie on the activation stack, a root, and take
no barrier.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.heap.old-allocations-born-remembered]
Every object allocated in the old space outside the image decoder is in
the remembered set from its allocation, since its initializing stores
take no barrier and may store young values; the decoder's objects are
allocated only after everything they can name was tenured, and machine
code places an object old only through the interpreter's own cold
allocation.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.heap.remembered-set-exact]
The remembered set holds each old object that may name a young one
exactly once, an object carries the remembered bit exactly when it is an
entry and no young object carries it; a scavenge rebuilds the set,
keeping an old object only if a word of it, a weak word included, still
names a young object, a full collection empties it and clears every bit,
and a move relocates each entry through the shift it applies to the
objects.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.no-forwarder-reached]
A forwarder exists only while the scavenge or the bulk forward that made
it runs; once it ends nothing names one, so no send, accessor, leaf,
identity comparison, encoder or describer meets one and its reserved
guard is never compared, a scavenge copies a young object at most once
so every forwarder chain it follows is one link, and a forwarder keeps
its word count so the space stays walkable until it is reclaimed.

Protects:
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`semantics.send.ordinary`](language/semantics.md#r-semantics.send.ordinary),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.bulk-forward-atomic]
The bulk forward validates every pair before it writes anything and
changes nothing when it refuses; it then gives each replacement the old
object's identity hash, turns each old object into a forwarder, rewrites
every handle the heap and the run's roots hold in one walk, keeps the
remembered set for every old holder that now names a young object and
invalidates every cache, all inside one leaf with no safepoint and no
instruction between; no live object moves.

Protects:
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`runtime.install`](runtime.md#r-runtime.install),
[`semantics.reflection.atomic-install`](language/semantics.md#r-semantics.reflection.atomic-install),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash)

r[vm.heap.install-commit-runs-nothing]
The commit of an install replaces mixin versions, re-applies class
objects, migrates instances and invalidates caches without running
Newspeak code, sending to a user object or polling a safepoint between
the end of its preparation and its own end; every failure arises before
anything is replaced, and a failed install changes no version,
application or guard.

Protects:
[`semantics.reflection.atomic-install`](language/semantics.md#r-semantics.reflection.atomic-install),
[`runtime.install`](runtime.md#r-runtime.install),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity)

r[vm.heap.migration-by-declared-name]
An install migrates each instance whose layout changed by copying every
field by the id of the level that declares it (that level's occurrence
counted from the chain's root) and its name, a hidden nested-class slot
by the nested declaration's id; a slot the edit adds or moves starts nil
with no initializer run, shadowing slots of one name stay apart, fields
are written through the store accessors, and only instances are
forwarded.

Protects: [`runtime.install`](runtime.md#r-runtime.install),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`semantics.slot.accessors`](language/semantics.md#r-semantics.slot.accessors),
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application)

r[vm.heap.weak-settled-before-compaction]
In a full collection, marking is followed, in this order, by the
ephemeron fixpoint (a value and a finalizer reached only through a live
key, a chain of ephemerons collected whole), the mourning of every
unresolved ephemeron and the clearing of every weak word naming an
unmarked object, and only then may compaction run, since its forwarding
answers only marked objects.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)

r[vm.heap.weak-settled-before-nursery-vacated]
A scavenge forwards every young weak word to its object's copy or sets
it to nil, and mourns each ephemeron whose key did not survive, before
the nursery is vacated; it remembers, once, an old weak object or
ephemeron that still names a young object afterwards.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)

r[vm.heap.ephemeron-key-first]
An ephemeron's first field is its key and the collectors read it as
such: the kernel declares the ephemeron's slots in that order with no
initializer, so instantiation writes none of them, and the fields are
reached only through accessors of the ephemeron's own format, never the
instance-slot accessors.

Protects:
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)

r[vm.heap.symbol-canonical]
A symbol object is made only through the two canonicalizing operations
-- the canonical table, or the weak table of program-made symbols whose
object the canonical table adopts once the text is interned, the prune
adopting every surviving one -- so a heap holds at most one symbol
object per text; a decoded symbol enters the canonical table at once and
a second one of a living text is refused, instantiating the symbol class
is refused, and both tables are weak, forwarded by every move and pruned
by every collection, their inverse maps rebuilt after every move.

Protects:
[`semantics.literal.symbol`](language/semantics.md#r-semantics.literal.symbol),
[`image.verify.symbols`](image-format.md#r-image.verify.symbols),
[`image.load.symbols`](image-format.md#r-image.load.symbols)

r[vm.heap.symbol-hash-from-text]
A symbol's hash is a function of its text alone, never of the canonical
table's number or of its address, so it does not change when the
canonical table adopts a program-made symbol and is the same in every
process and across an image.

Protects:
[`semantics.literal.symbol`](language/semantics.md#r-semantics.literal.symbol)

r[vm.heap.reified-mixin-canonical]
A mixin is reified as at most one object per mixin per heap, held in a
table indexed by the mixin's id that is weak in a full collection,
strong in a scavenge and forwarded by every move; an object stands for a
mixin only if the table holds it for the id its field names, a decoded
reified mixin enters the table at once and a second is refused, and a
marked entry keeps its mixin's id alive.

Protects:
[`semantics.reflection.mirrors`](language/semantics.md#r-semantics.reflection.mirrors),
[`image.verify.mixin-records`](image-format.md#r-image.verify.mixin-records)

r[vm.heap.every-mover-realigns]
A byte array's data begin at the first 32-byte address of its block, an
offset computed from the block's address and never stored, so the
object's size in words never changes; every operation that copies such
an object to another address -- the scavenger's copy, the compaction's
slide, the move stress mode's shift -- moves the data to the new block's
alignment, and the decoder places them at its placeholder's.

Protects:
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity)

r[vm.heap.byte-array-own-format]
A byte array has a format of its own, never a string's, so that every
decision made by format alone -- is it a string, its shape, its
description, its encoding -- is right for it; no object of a string
format is a byte array and no byte-array body is accepted under another
class.

Protects:
[`semantics.literal.string`](language/semantics.md#r-semantics.literal.string),
[`image.verify.byte-arrays`](image-format.md#r-image.verify.byte-arrays)

r[vm.heap.objects-never-grow]
Every object keeps its size from its allocation to its reclamation; a
change of an object's size takes a new object and a bulk forward of the
old one's references.

Protects:
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`runtime.install`](runtime.md#r-runtime.install),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.access-within-object]
Every field, element and register access through an object indexes that
object's own block, which ends where its header says, so no access
through one object -- a crafted image's by-reference register write
included -- reaches another object's words; every view of a reservation
covers only committed accessible memory and none outlives a change of
what is accessible.

Protects: [`image.untrusted`](image-format.md#r-image.untrusted),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.field-access-total]
The heap's field read and write are total: an index past an object's
fields writes nothing and reads as an empty slot, so an own-slot
instruction's index, which the image decoder does not bound, never reads
or writes outside the object.

Protects: [`image.untrusted`](image-format.md#r-image.untrusted),
[`image.verify.not-checked`](image-format.md#r-image.verify.not-checked)

Undetected: no test yet: decode an image whose body's own-slot
instruction names an index past its receiver's fields, run it, and check
that the run neither aborts nor changes another object.

r[vm.heap.bump-limit-within-committed]
The bump cursor's limit never lies past committed read-write memory --
every change of the committed pages clamps it -- and machine code moves
the top only by a bump ending within the limit; the fast limit lies
below the scavenge threshold and short of eden's end by the move stress
mode's slack, is placed again by every change of eden's start, top or
threshold, and refuses every fast bump while the decoder allocates old.

Protects: [`image.bound.linear`](image-format.md#r-image.bound.linear),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.heap.allocation-initialized-before-read]
An allocation writes its header and class word and fills or stores every
field before any safepoint and before any read of the object, in the
interpreter and in machine code alike; the header is written only by the
heap's allocation paths.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate)

r[vm.heap.precomputed-header-matches]
A header precomputed for an allocation -- an application's instance
header, the closure, frame and boxed float guards -- carries the guard a
lookup of the object's class would answer; an application's dispatch id
and slot count never change after it is registered, and the class word
is read from the class object at every allocation.

Protects: [`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`runtime.send`](runtime.md#r-runtime.send),
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object)

r[vm.heap.value-tags]
A value is one word whose low three bits say what it is -- a handle, a
small integer, a constant (nil, false, true) or an immediate float --
the other patterns never produced, so that one bit tells a small integer
and the collector reads a word as a handle only under the handle's tag.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`semantics.literal.integer`](language/semantics.md#r-semantics.literal.integer),
[`semantics.literal.float`](language/semantics.md#r-semantics.literal.float),
[`semantics.literal.boolean-and-nil`](language/semantics.md#r-semantics.literal.boolean-and-nil)

r[vm.heap.number-one-representation]
Each integer and float has exactly one representation: an integer in the
small-integer range is always immediate and never an arbitrary-size
integer object, which is canonical (no zero top digit, zero never
negative), and a finite float in the immediate range is never boxed; one
function makes each kind of number from a computed one.

Protects:
[`semantics.number.exact`](language/semantics.md#r-semantics.number.exact),
[`semantics.literal.integer`](language/semantics.md#r-semantics.literal.integer),
[`semantics.literal.float`](language/semantics.md#r-semantics.literal.float),
[`image.verify.numbers`](image-format.md#r-image.verify.numbers)

r[vm.heap.fraction-normal-form]
A fraction object has exactly two integer fields, reduced, its
denominator above 1 and its sign on its numerator, so equal fractions
have equal parts and no fraction equals an integer; only the runtime
makes one, through one function, and an object is a fraction only by its
class's builtin together with exactly two fields.

Protects:
[`semantics.number.exact`](language/semantics.md#r-semantics.number.exact),
[`image.verify.numbers`](image-format.md#r-image.verify.numbers)

r[vm.heap.string-canonical-width]
A string or symbol is stored in the one-byte format exactly when every
code point fits one byte, units least significant first and the last
word's unused units zero, and in the 32-bit format otherwise, every
element a Unicode scalar value; so equal texts have equal words,
equality compares the words directly and the hash agrees with it.

Protects:
[`semantics.literal.string`](language/semantics.md#r-semantics.literal.string),
[`semantics.literal.symbol`](language/semantics.md#r-semantics.literal.symbol)

r[vm.heap.identity-hash-moves-with-object]
An object's identity hash is drawn from a counter the first time it is
asked for, never from its address or handle, is 0 only while unassigned,
and is carried unchanged in its header by every mover and, onto the
replacement, by the bulk forward; so allocation order and placement are
unobservable and the two lowerings, which allocate different objects,
answer the same hashes.

Protects:
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`image.load.hashes`](image-format.md#r-image.load.hashes)

r[vm.heap.shipped-world-draws-no-hash]
Building any part of the shipped world -- at boot, on a module's first
demand or by the loader -- runs no code that draws an identity hash, and
modules are compiled and rebuilt in one fixed order, so the hashes a
program draws are the same under a run and under a load.

Protects:
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash),
[`image.load.hashes`](image-format.md#r-image.load.hashes),
[`image.hashes`](image-format.md#r-image.hashes)

r[vm.heap.class-by-format]
Whether an object is a class object is read from its header's format
alone, never by following its class word, so the question can be asked
of an object a crafted image made; a format code no format uses reads as
a free gap.

Protects: [`image.untrusted`](image-format.md#r-image.untrusted),
[`image.verify.class-records`](image-format.md#r-image.verify.class-records)

r[vm.heap.class-word-never-nil]
An object's class word always names a class object: from the first
moment of the bootstrap a provisional class word names the builtin
`Object`, and every placeholder the decoder allocates names it until its
record is filled in place.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`image.untrusted`](image-format.md#r-image.untrusted)

Undetected: no test yet: in a unit test of the heap, allocate a
decoder's placeholder and read its class word before the record fills
it, checking it names the builtin `Object`.

r[vm.heap.class-word-is-metaclass]
After the bootstrap every class object's class word names its own
metaclass and every metaclass's names `Metaclass`; a metaclass is built
together with its class object, never later, and has no metaclass of its
own.

Protects:
[`semantics.class-side`](language/semantics.md#r-semantics.class-side)

r[vm.heap.instance-has-application-layout]
Every instance of a program class has exactly its class's application's
slot count of fields, as allocation makes it, as an install migrates it
and as the decoder holds every record to it; optimized code's unchecked
own-slot reads at fixed offsets rest on it.

Protects: [`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`image.verify.program-instances`](image-format.md#r-image.verify.program-instances),
[`image.verify.shipped-instances`](image-format.md#r-image.verify.shipped-instances)

r[vm.heap.no-recursion-over-input-depth]
No walk or teardown whose depth a program or an image chooses recurses
on the native stack: marking uses an explicit work list, the encoder an
explicit stack, an application chain tears down iteratively, and a
recursive walk of the mixin tree is bounded by a ceiling the front end's
limits imply.

Protects: [`image.untrusted`](image-format.md#r-image.untrusted),
[`syntax.nesting-limit`](language/grammar.md#r-syntax.nesting-limit)

Undetected: no test yet: build an object graph and an application chain
a million deep on a small thread, collect and drop them, and check the
process survives.

r[vm.heap.stop-runs-no-code]
While a program stands at a debugger's stop and the stop serves a
command other than an evaluation, no Newspeak code runs, nothing is
pushed or popped, nothing collects and no handle is live across the
wait; values, fields, the stack and the object space are read through
the heap's accessors alone, nothing is allocated in the object space,
and a lazy slot not yet filled is shown as not computed, never computed
by the stop.

Protects:
[`semantics.slot.lazy`](language/semantics.md#r-semantics.slot.lazy),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)

## Handles and the brand

r[vm.handle.space-relative]
A handle is a byte offset into the object space of the heap that minted
it and means nothing against another heap; an activation's handle is an
offset on its heap's activation stack, reused after a pop. A handle used
as a map key is keyed through its own heap, so two heaps' handles never
collide, and nothing does arithmetic on the key.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash)

r[vm.handle.activation-never-stored]
A running activation's handle, and its register base, are valid only
until its frame is popped, after which the next push at that depth
reuses the offset; everything that outlives the call -- a closure's
captured frame, a frame object's home and sender, an unwind's target, a
suspended sender, a side-table row, an image record -- holds the handle
of a frame object instead, a type of its own.

Protects:
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout)

r[vm.handle.nothing-carried-across-safepoint]
Every handle a collector can move, and every value or record containing
one, carries the brand of the entry it was minted or read in and cannot
leave it; at a safepoint every reference the run holds lies in a stack
frame's word, a side-table row, an object or a root, and the loop
carries across it only stack offsets, compiled code, a program counter
and a feedback base, read afresh from the frame after it. Only the heap
converts between a branded value and its at-rest form, no operation
rebrands, and the bare stored word is read and written only inside the
implementation.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.handle.host-holds-only-roots]
A host holds a value across a run only as a slot of the heap's root
table, which every collection, scavenge and move traces and rewrites;
outside every entry a host can build only an immediate, never hold an
object's handle, and a value read from a root is valid only within the
entry that read it.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.handle.stored-only-when-current]
Every move advances the heap's move epoch and rewrites every traced
handle into it; a value becomes a stored word only through the heap's
one store conversion, and only when its heap and epoch are current, and
a word read out is given the current ones, so a stale handle is never
laundered into a field.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write)

r[vm.handle.only-where-rewritable]
A handle is held only where the tracer can rewrite it in place -- an
object's pointer words, a stack frame's word, a continuation one
activation's side-table row owns, or the mutator's own roots -- never
behind shared ownership; a closure keeps its captured frame and copied
values in its object's words and names its body, which holds no handle,
by a code id.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.handle.never-zero]
No object starts at offset 0 of the space, so a handle is never 0 and an
all-zero word -- a missed initialization -- is never a live reference.

Protects: [`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.handle.debugger-holds-no-bare-handle]
Nothing that crosses from the VM's thread to the debugger agent's holds
a value, a handle or a root: a value is named by an id that only the
stop which handed it out resolves, ids running on across the stops of
one attachment, so an earlier stop's id names nothing rather than
another object; every object the debugger keeps beyond one heap access
-- the values a stop hands out, a suspended frame it reads, marked
objects, the last call's value, an exception's report, a restart's or
forced return's frame and value -- is a root, and a stop walks its
frames again after an evaluation.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.handle.trace-holds-no-handle]
A pending trace event or listing holds its bodies as owned references,
its selectors as symbols, a copy's key as a dispatch id named before a
full collection can reclaim the id and its receiver class as a computed
name; it holds no handle and keeps no program object alive, so a
collection or a move between recording and delivery changes nothing it
names.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)

r[vm.handle.mixin-registered-in-one-heap]
A mixin is registered in at most one heap, and registering a mixin
registers its whole declared tree, enclosing mixins first, so a mixin's
id always names a row of the one heap that minted it.

Protects:
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application),
[`runtime.install`](runtime.md#r-runtime.install)

Undetected: no test yet: register a mixin tree in one interpreter, ask a
second interpreter for an inner mixin's id, and check it registers the
tree afresh rather than answering the first heap's id.

## Activations and frames

r[vm.activation.frame-words-tagged]
Every word of every frame on the activation stack -- header and
registers alike -- is a valid tagged value, a number as a small integer
and an absent reference as nil, so the collector traces every word below
the stack's top, and every row of the side tables beside it, with no
stack map.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.activation.dead-registers-nilled]
Before any trace of the activation stack reads a frame's words, every
register the frame does not read before writing it, as of the program
counter its header stores, is overwritten with nil; the analysis is
answered only for a body that passed the body check, since an answer for
a malformed one would nil live registers.

Protects:
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`bytecode.verify.leftovers`](bytecode.md#r-bytecode.verify.leftovers),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.activation.top-at-window-end]
Neither a push nor a pop writes a scratch register; a push laid on its
sender's argument slots never lowers the stack's top, a frame's caller
word records the top it was pushed at and its pop restores exactly that
top without writing the caller's registers above its slots, so the top
is always some live frame's window end and no word at or above it is
traced.

Protects:
[`bytecode.register.argument-window`](bytecode.md#r-bytecode.register.argument-window),
[`bytecode.register.arguments`](bytecode.md#r-bytecode.register.arguments),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers)

r[vm.activation.only-senders-callee-on-slots]
Only an activation that takes the sending instruction's own arguments is
laid on the sender's argument slots; every other push -- a
does-not-understand handler, a reified message's send, a forcing, an
unwind block, a host's entry, a resumed frame, an initializer a factory
enters, a leaf's push -- copies its arguments above the sender's whole
window, so no live register of the sender is overwritten.

Protects:
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order),
[`bytecode.register.argument-window`](bytecode.md#r-bytecode.register.argument-window),
[`bytecode.register.arguments`](bytecode.md#r-bytecode.register.arguments)

r[vm.activation.refused-push-changes-nothing]
Every word of a new frame is computed before the frame is taken, so a
push the reservation or the depth limit refuses leaves the stack, its
top and every frame exactly as they were.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.activation.every-push-checks-depth]
Every activation the runtime pushes goes through one placement that
refuses a counted push at the depth limit; methods and every body an
instantiation or a nested-class accessor runs are counted by
construction, so no push site can skip the limit.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.activation.depth-count-exact]
The count of activations against the depth limit is one running total
kept on every push and pop; error paths pop through the one unwinder,
suspending and resuming leave it alone since a suspended activation
stays counted, a resume never checks the limit, and whether an
activation counts is recorded in its frame when it is pushed.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.activation.depth-exemption]
The depth count exempts exactly the block activations one lowering
inlines and the other activates -- a block activated for a control-flow
send the reference lowering marks, a declining mirror send's failure
block, a marked loop's `value`, a block a quick method's step answers --
and only when the block's captured frame is the sending activation's own
reified frame; the mark is built only by those steps, so both lowerings
reach the limit at the same point and crafted code cannot recurse
uncounted.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.activation.depth-reserve-held-by-frame]
The reserve of activations above the depth limit for signalling a VM
error is held by the frame object of the activation the error arose in,
never a stack offset, and only while that frame runs with the error's
signal or the kernel's does-not-understand directly above it; a later
signal at a reused offset never inherits it, and a host's run's end
releases it.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search)

r[vm.activation.stack-reservation-reserve]
The last sixteenth of the activation stack's reservation is refused to
an ordinary push and left to pushes made while the depth reserve is
held, so a refused push's signal, its handler search and a short handler
always find room.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)

r[vm.activation.register-in-window]
Every register access an instruction makes through an activation lies
inside that activation's own window and names no header word; the stack
is bounds-checked only against its committed end, so an index past the
window would read or write another frame's word.

Protects:
[`bytecode.verify.registers`](bytecode.md#r-bytecode.verify.registers),
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout)

r[vm.activation.register-base-from-argument-count]
A frame's first register lies its argument count of words below its
header, so the run loop's register base after a push or a return is read
from the frame's recorded argument count; every push takes exactly as
many arguments as its body declares, and a frame resumed from its object
or decoded from an image takes its body's count.

Protects:
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout),
[`image.verify.frames`](image-format.md#r-image.verify.frames)

Undetected: no test yet: suspend and resume a frame of a body with
arguments, and decode a frame record of one, then read its first
argument through the resumed register base.

r[vm.activation.offset-fixed]
A frame's offset on the activation stack is fixed for its whole life and
everything addresses the stack by index, so committing more of the
reservation moves nothing; a frame's start grows with its depth, so
positions compare as an order and a run's base has returned once the top
is back at its position.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.activation.run-balanced]
A run pops every activation it pushes, its base included, on every path
out -- a return, an error, a non-local return aimed below the base, a
termination -- and only the top frame is ever popped, so the stack is
exactly as deep after a run as before it.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.method.return`](language/semantics.md#r-semantics.method.return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return)

r[vm.activation.no-suspension-outlives-run]
Every path out of a run pops through the frames that return to suspended
senders, resuming each sender in turn, so the side table of suspensions
is empty when a run ends.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`image.write`](image-format.md#r-image.write)

r[vm.activation.reify-once]
Reifying an activation is idempotent: the first reification allocates
its frame object, marries it to the stack frame and records it in the
frame's header, and every later one answers that same object, so one
activation is one frame object for its whole life.

Protects:
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.activation.marriage]
A frame object is in exactly one of three states, told by one word:
married (it names its stack frame's offset), suspended (it names its row
of the suspension table and holds every register) or returned. While
married its own registers stay nil and are skipped by the trace, every
read or write of a register through it reaches the stack slot, the stack
frame's word naming it is traced and forwarded, and only the heap reads
or writes the marriage.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`bytecode.instruction.store-outer-local`](bytecode.md#r-bytecode.instruction.store-outer-local)

r[vm.activation.divorce-on-pop]
Popping a reified activation divorces its frame object: the object takes
the activation's last stored program counter, becomes uncontinuable,
loses its sender, and keeps the registers below its body's named locals
only when the frame asked to retain them, every other register nil; a
divorced frame is married to nothing afterwards.

Protects:
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.activation.outer-reach]
Every instruction that reads or writes another frame's registers by
reference declares its depth, in the instruction and in its eight-byte
form alike, and a frame keeps its registers past its return exactly when
a block it created may still reach them by reference -- a block of the
reference lowering or a fallback closure -- its reach folded one level
less through each nesting so the whole chain is marked; converted code
copies values or shares a vector and keeps none.

Protects:
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`bytecode.instruction.store-outer-local`](bytecode.md#r-bytecode.instruction.store-outer-local),
[`bytecode.instruction.make-closure`](bytecode.md#r-bytecode.instruction.make-closure),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.activation.frame-object-full-register-file]
Every activation is pushed with its body's whole register file, and a
frame object is allocated at its reification with a field for every
register its body numbers but the header's; neither ever grows or
shrinks, so a closure that reads its frame by reference never reads past
it.

Protects:
[`image.verify.frames`](image-format.md#r-image.verify.frames),
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout),
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`bytecode.instruction.store-outer-local`](bytecode.md#r-bytecode.instruction.store-outer-local)

r[vm.activation.suspension-copies-registers]
Suspending reifies every frame at or above the floor and, from the top
down, copies every register into its frame -- a sender's only below the
header of the callee on its slots, nil for every register the frame does
not read before writing it -- writes its program counter and sender and
moves the rest of its header into a side-table row; a frame object holds
a frame's registers without the header's numbers, so no frame object or
image record holds a stale header word; resuming copies them back and
frees the row.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`image.verify.frames`](image-format.md#r-image.verify.frames)

r[vm.activation.resume-derives-state]
A resumed activation's feedback base is decided again by the push's rule
for its kind and its recorded slot guard starts at none, so its first
own-slot access takes the full check, while its entered level is kept in
the suspension's row and restored, never derived from the receiver's
header; the side table holds no number a reclaim or a slide could
orphan.

Protects: [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.activation.sender-walks-reach-suspended]
Everything that walks from an activation to its sender -- the sender
answer, the handler search, the handler leaves, the unwinder, the depth
reserve's walks -- goes on into suspended frames, and every pop an
unwind makes first resumes the suspended sender of the frame it pops.

Protects:
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.exception.handler`](language/semantics.md#r-semantics.exception.handler),
[`semantics.exception.ensure`](language/semantics.md#r-semantics.exception.ensure)

r[vm.activation.program-counter-before-observation]
An activation's program counter is stored in its stack frame by every
instruction that can hand the activation to an observer before it does
so -- every send, a closure creation, a non-local return, a forcing, an
instruction that raises -- and when the loop leaves for a safepoint,
machine code's frames by the exit's walk; so a collection judges dead
registers, a divorce or suspension records, and a return resumes as of
the right instruction.

Protects:
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`image.verify.frames`](image-format.md#r-image.verify.frames)

r[vm.activation.unwind-target-is-frame]
Every unwind -- a non-local return, a handler's `return:`, `retry` and
`resume:`, a restart, a termination, the depth reserve -- is carried by
the one unwinder and names its target by the target's frame object,
asking it for its stack frame afresh before every pop, since `ensure:`
and `ifCurtailed:` blocks run meanwhile can reuse a popped target's
offset; a target found divorced is dead, never mistaken for its
successor.

Protects:
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return),
[`semantics.exception.handler`](language/semantics.md#r-semantics.exception.handler),
[`semantics.exception.ensure`](language/semantics.md#r-semantics.exception.ensure),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.activation.unwind-marker-from-code]
Whether an activation is an `on:do:`, `ensure:` or `ifCurtailed:` is a
property of its compiled code, read off each activation's code afresh by
the unwinder and the leaves, never from a cache; only kernel methods
carry a marker, it is never written to an image, and a decoded body has
none.

Protects:
[`semantics.exception.handler`](language/semantics.md#r-semantics.exception.handler),
[`semantics.exception.ensure`](language/semantics.md#r-semantics.exception.ensure),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.activation.factory-prefix-uninterruptible]
A factory body's allocation and instance-initializer instructions may be
carried out at the factory's push because nothing can run between the
push and them -- no program code, no collection; the frame is still
pushed, and an error leaves the factory unexecuted at its first
instruction, so the run loop raises it in the body's order.

Protects: [`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`semantics.instantiation.initializer-order`](language/semantics.md#r-semantics.instantiation.initializer-order),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)

r[vm.activation.passed-levels-iterative]
Superclass levels whose initializer would run nothing are passed in a
loop, never by recursion or an activation each, so a chain of any length
an image chooses costs one activation and no native stack.

Protects: [`image.untrusted`](image-format.md#r-image.untrusted),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)

r[vm.activation.abandoned-continuation-undone]
The unwinder hands every popped activation's continuation to the
abandoning counterpart of resumption, which undoes what the continuation
began -- above all a nested class's creation marker in its hidden cache
slot, reset only if it is still that creation's marker -- as the run
loop hands a returning one to resumption.

Protects: [`runtime.outer-send`](runtime.md#r-runtime.outer-send),
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object)

r[vm.activation.closure-home-derived]
A closure holds the reified frame of the activation that created it, and
its home is derived from that frame -- the frame itself for a method's,
initializer's or clause's, its home for a block's -- never stored apart;
a decoded frame without a home is a dead home, never asserted on.

Protects:
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return),
[`image.untrusted`](image-format.md#r-image.untrusted)

r[vm.activation.push-checks-arity]
A push refuses a method, an initializer or an applied level's own part
whose argument count is not what its send or factory passes, checked at
every level including decoded ones, because an activation addresses its
arguments and temporaries as one run of registers and a mismatch would
shift every temporary.

Protects:
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout),
[`bytecode.register.argument-window`](bytecode.md#r-bytecode.register.argument-window),
[`runtime.invoke`](runtime.md#r-runtime.invoke),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate)

r[vm.activation.reexecuted-instruction-wrote-nothing]
An instruction that runs again -- after a future it met was forced,
after a trap or a refused call, or after machine code handed it to the
interpreter -- wrote nothing in its first run: it forces before it
writes, and a leaf that declined or raised changed nothing but, for a
send, its own argument slots, which nothing reads after the send but the
send run again.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order),
[`semantics.slot.immutable`](language/semantics.md#r-semantics.slot.immutable),
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)

r[vm.activation.header-one-size]
A frame's header is the same thirteen words in every build -- a debug
build's generation rides in the flags word -- because the header's size
is part of compiled code and of image records, so code and images one
build wrote run in another.

Protects:
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout),
[`image.verify.frames`](image-format.md#r-image.verify.frames),
[`image.load`](image-format.md#r-image.load)

Undetected: no test yet: write an image with a frame record from a debug
build and load it in a build without debug assertions, as part of the
release-checks group.

r[vm.activation.debugger-reads-no-dead-register]
Every debugger read of a frame's register -- a variable at a stop, the
last call's value, a referrer from the stack -- reads only a register
the leftover analysis calls live at the frame's stored program counter,
and another frame's register only below its body's local count; a
tainted register is not available, never read.

Protects:
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.activation.evaluation-above-every-window]
The activation of an expression the debugger evaluates is pushed above
every live frame's whole register window, whichever frame it evaluates
in, so it overwrites no register a frame below the top -- its callees'
windows, an enclosing expression's scratch -- still needs.

Protects:
[`bytecode.register.argument-window`](bytecode.md#r-bytecode.register.argument-window),
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order)

r[vm.activation.evaluation-returns-to-its-stop]
An evaluation's base activation returns through a continuation of its
own, never through the caller-resumption path, switching to the stopped
frame at the program counter it stopped at, delivering nothing and
taking the safepoint before that frame runs another instruction; an
evaluation's backward jumps are never counted against the stopped
frame's body, so its end never switches that frame into machine code
mid-body.

Protects:
[`bytecode.instruction.return`](bytecode.md#r-bytecode.instruction.return),
[`bytecode.instruction.implicit-return`](bytecode.md#r-bytecode.instruction.implicit-return),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.activation.evaluation-retains-reached-frames]
The debugger makes an expression's closure over the stopped frame as the
frame's own closure creation would: when the expression creates a block
that reaches the stopped frame or beyond, that frame and every frame
along its captured chain the block reaches are marked to keep their
registers on return; an expression that only reads or writes the frame's
variables marks nothing.

Protects:
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.activation.evaluation-base-is-a-barrier]
An evaluation's base activation ends every handler search that reaches
it and every unwind from within it -- a non-local return to a home below
answers cannot-return from the base, `retry` and `pass` do not cross it,
an overrun's or an abort's termination ends there -- so no evaluation
unwinds, handles in or resumes the program's frames below; only the
debugger's own termination of the run goes past it.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`semantics.exception.handler`](language/semantics.md#r-semantics.exception.handler),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return)

r[vm.activation.evaluation-depth-bounded]
Evaluations never nest, since an evaluation goes past every breakpoint,
so the activation stack holds at most the depth limit, its reserve, one
evaluation's reserve and the reserve of an error signalled within it;
machine code within an evaluation nests at most that reserve above its
base, and whatever an evaluation overruns ends at its base, never the
run.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.activation.debugger-unwinds-through-the-unwinder]
A restart and a forced return name their target by its frame object and
are carried by the one unwinder, every `ensure:` and `ifCurtailed:`
block above it run and a pending unwind raised again as it was; the
unwinder tests whether an unwind is the debugger's only after the
program's own catches failed, so a program's non-local return and
`retry` arrive where they did, and an evaluation pushed inside an unwind
re-raises the unwind it interrupted unchanged.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.exception.ensure`](language/semantics.md#r-semantics.exception.ensure),
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return)

r[vm.activation.rewound-push-ran-nothing]
The debugger takes back an activation's push -- the initializer a
factory pushed, met by a step's held request -- only before that
activation ran any instruction, and the instruction that pushed it
decides and pushes it again once the program goes on, so each
initializer runs exactly once.

Protects:
[`semantics.instantiation.initializer-order`](language/semantics.md#r-semantics.instantiation.initializer-order),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate)

## The compiler

r[vm.compiler.total]
Compiling a resolved module is total and deterministic: it never raises
a program error, a compiler-invariant violation aborts the compile, and
every program error is deferred into a literal that raises when, and
only if, it runs -- an unbound name, a body past a limit, an unbuilt
construct -- so loading a module evaluates nothing and a method never
run never fails.

Protects:
[`bytecode.limit.too-large`](bytecode.md#r-bytecode.limit.too-large),
[`semantics.module.root-rule`](language/semantics.md#r-semantics.module.root-rule),
[`semantics.gap.actors`](language/semantics.md#r-semantics.gap.actors),
[`semantics.gap.pattern`](language/semantics.md#r-semantics.gap.pattern)

r[vm.compiler.only-error-free-source]
A source with any error-severity diagnostic is never resolved or
compiled: the parser entry everything that compiles uses answers an
error for it, and the recovered module the editor's entry answers, which
declares nothing the source does not contain, reaches the editor only.

Protects: [`syntax.peg`](language/grammar.md#r-syntax.peg),
[`syntax.compilation-unit`](language/grammar.md#r-syntax.compilation-unit)

r[vm.compiler.register-count-checked]
Every register count and register number a lowering computes is checked
arithmetic in every build, and a body past a limit compiles to the one
body that raises the limit, nothing else it emitted kept; a call-site
number never wraps either.

Protects:
[`bytecode.limit.registers`](bytecode.md#r-bytecode.limit.registers),
[`bytecode.limit.too-large`](bytecode.md#r-bytecode.limit.too-large),
[`bytecode.call-site`](bytecode.md#r-bytecode.call-site)

r[vm.compiler.value-register]
An expression's value register still holds that value when its consumer
executes: a named local's register is live storage, so a lowering copies
it whenever something lowered after it can write that register -- only
an assignment in the same body, inlined blocks included, or, for a local
an inlinable branch assigns, a fallback closure any send may run -- and
in a by-reference body every writable register counts as writable.

Protects:
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order),
[`semantics.send.local-setter`](language/semantics.md#r-semantics.send.local-setter)

r[vm.compiler.read-only-locals]
A parameter's, an immutable local's, a copied value's and the
indirection vector's registers are written only by their initialization;
no assignment binds to them in the frame or through a closure, so they
are never protectively copied.

Protects:
[`semantics.slot.immutable`](language/semantics.md#r-semantics.slot.immutable),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`bytecode.register.arguments`](bytecode.md#r-bytecode.register.arguments)

r[vm.compiler.simultaneous-temporary-writable]
A simultaneous temporary is written twice whatever its declaration says,
so it is never read-only: a sibling's block shares it through the
indirection vector rather than copying its first value.

Protects:
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)

r[vm.compiler.capture-agrees-with-lowering]
The capture analysis and the lowering decide alike which block literals
are closures and which inlined scopes -- control-flow, arithmetic and
mirror-send recognition asked at the same places -- key a block the same
way and enter blocks in the resolver's order; a mutable local goes into
its scope's indirection vector whenever anything could write it after a
real block captured it, an inlined loop's or an inlinable branch's write
counting as after every capture, and every other captured variable is
copied and never written in the block.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`semantics.send.local-setter`](language/semantics.md#r-semantics.send.local-setter),
[`bytecode.instruction.make-closure`](bytecode.md#r-bytecode.instruction.make-closure)

r[vm.compiler.fallback-closure-by-reference]
A fallback closure of an inlined conditional, and every block nested in
it, reads and writes the frame it stands in for by reference, so the
lowering never treats it as converted; its writes count as after every
capture.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`semantics.value.conditions`](language/semantics.md#r-semantics.value.conditions)

r[vm.compiler.copied-values-order]
A block's copied values are in first-reference order, transitively
including what nested blocks need, and that order is the block's
register layout: the creating instruction's copied list and the block's
copied count agree, and an activation fills exactly that many registers
after the parameters, never past its own window.

Protects:
[`bytecode.instruction.make-closure`](bytecode.md#r-bytecode.instruction.make-closure),
[`bytecode.verify.blocks`](bytecode.md#r-bytecode.verify.blocks),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.compiler.vector-never-a-value]
An indirection vector is touched only by the vector instructions and
never handed to program code as a value; a vector variable's own
register stays nil.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`semantics.object`](language/semantics.md#r-semantics.object)

Undetected: no test yet: compile a body whose captured mutable local
lives in a vector and check, over the body's instructions, that the
vector's register is read only by the vector instructions and the
closure creation.

r[vm.compiler.hop-count-counts-frames]
An outer-local reference's run-time depth is the number of frame scopes
among the innermost scopes the resolver counted, since an inlined block
has a scope and no frame; a real block made inside an inlined region
captures the enclosing frame.

Protects:
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`bytecode.instruction.store-outer-local`](bytecode.md#r-bytecode.instruction.store-outer-local),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.compiler.argument-slots-top-live-registers]
A send's argument slots are allocated after its receiver at the top of
the sender's live registers, scratch registers never stay live across a
statement, and every register numbered above a send's slots is dead when
the send executes, since the callee's header and window go over them;
the one value kept across a send is a setter assignment's, in its slot
or a named local below it.

Protects:
[`bytecode.register.argument-window`](bytecode.md#r-bytecode.register.argument-window),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers)

r[vm.compiler.register-effects-declared]
Every instruction's register effects -- what it reads, what it writes,
where it jumps, whether it falls through, its successors, a send's
argument slots and the message it delivers -- are declared in exhaustive
matches that the leftover analysis, both lowerings' checks and the
decoder share; at entry every scratch register is tainted, a send other
than a mirror send taints its slot end and above and writes none of its
slots for certain, and a join is tainted where either path is.

Protects:
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`bytecode.verify.leftovers`](bytecode.md#r-bytecode.verify.leftovers),
[`bytecode.register-effects`](bytecode.md#r-bytecode.register-effects),
[`bytecode.verify.send-arity`](bytecode.md#r-bytecode.verify.send-arity)

r[vm.compiler.reach-derived-never-read]
A body's outer reach and family size are derived from its instructions,
by the compiler, the image decoder and the embedded code's decoder alike
through one constructor, never read from an image or the artifact; the
embedded code's encoder destructures every type and matches every
instruction without a wildcard.

Protects:
[`bytecode.instruction.make-closure`](bytecode.md#r-bytecode.instruction.make-closure),
[`bytecode.compact.round-trip`](bytecode.md#r-bytecode.compact.round-trip),
[`bytecode.wire.call-sites`](bytecode.md#r-bytecode.wire.call-sites),
[`image.untrusted`](image-format.md#r-image.untrusted)

r[vm.compiler.mirror-literal-block-last]
Shipped code passes a literal block to a mirror send only as its final
failure argument, and a block a leaf itself activates as a parameter,
because the optimized lowering exempts the failure block's activation
from the depth count and the reference lowering, which sends, does not;
a cascade to the mirror is no mirror send.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.compiler.factory-body-synthesized]
A factory's body is synthesized from its arity alone by one function,
attached to every factory entry whoever builds it -- the compiler, the
image decoder, the embedded code's decoder -- and never carried by an
image; each level's initializer is a body on its mixin outside every
method table, reached only by the initializing instructions.

Protects: [`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`semantics.instantiation.factory`](language/semantics.md#r-semantics.instantiation.factory),
[`image.verify.initializers`](image-format.md#r-image.verify.initializers)

Undetected: no test yet: build a class's factory through each path that
attaches one (compiled, decoded from an image, decoded from the embedded
code) and check that the three bodies compile alike.

r[vm.compiler.mixin-relative-slot-index]
Compiled code lives on its mixin and is shared by every application of
it, so an own-slot or lazy-slot instruction's index is relative to the
mixin's own slots and is resolved at run time against the slot base of
the application the activation was entered at, never the class object's
current one; no shared body holds an absolute field index.

Protects: [`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`bytecode.instruction.initialize-slot`](bytecode.md#r-bytecode.instruction.initialize-slot),
[`bytecode.instruction.return-slot-if-filled`](bytecode.md#r-bytecode.instruction.return-slot-if-filled),
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application)

r[vm.compiler.code-holds-no-handle]
A body's instructions and side tables hold no handle: a constant is an
immediate word and every other literal is allocated afresh each time it
is loaded, so compiled code is independent of every heap and needs no
tracing.

Protects: [`image.load.caches`](image-format.md#r-image.load.caches),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.compiler.code-id-cache-current]
A body's one-entry cache of its code id answers only an id the same heap
answered under its current code-table epoch; every heap's epoch is
unique in the process and renewed whenever a full collection frees a
code row, and a copy of a body starts with an empty cache, so an id from
another heap or a freed one is never answered.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.invoke`](runtime.md#r-runtime.invoke)

r[vm.compiler.symbol-numbers-permanent]
Every symbol number the implementation holds -- in compiled code, a
cache key, a return site, a frame header, an image record, a leaf's or
stub's selector -- is permanent and never recycled; a program-made
symbol has no number until its text is interned, and nothing that stores
a number holds one.

Protects:
[`semantics.literal.symbol`](language/semantics.md#r-semantics.literal.symbol),
[`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup)

r[vm.compiler.embedded-code-current]
The embedded compiled code of the shipped modules is decoded only when
its header carries the magic, the format version and the fingerprint of
the sources the binary was built with; a stale or missing artifact makes
the boot compile the source instead, and code compiled without primitive
methods is never embedded.

Protects:
[`image.header.fingerprint`](image-format.md#r-image.header.fingerprint),
[`image.load.shipped-world`](image-format.md#r-image.load.shipped-world),
[`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror)

r[vm.compiler.edit-recompiles-from-current-text]
An edit that adds or removes a method or slot compiles the edited
declaration and everything nested in it again from the module's current
text, parsed and resolved whole by the program resolver, never reusing a
body compiled against the old member sets, because implicit-receiver
bindings take their depth from the member names of every class on the
lexical chain; a member text must be exactly one member of its kind
before it is spliced.

Protects:
[`runtime.implicit-receiver`](runtime.md#r-runtime.implicit-receiver),
[`semantics.send.implicit-receiver`](language/semantics.md#r-semantics.send.implicit-receiver),
[`runtime.install`](runtime.md#r-runtime.install)

r[vm.compiler.patched-slot-runs-original-once]
The run loop's arm for a slot a breakpoint patched runs no Newspeak
code, collects nothing and decides only; going on, it executes the
slot's original instruction through that instruction's ordinary arm once
per reach, a stop or a condition leaving for the safepoint rather than
running in the arm, and an instruction run again after a future resolves
goes past its patch as an unpatched one would; instruction indices never
move, so no statement table, leftover analysis or stored program counter
changes.

Protects:
[`bytecode.compact.breakpoint`](bytecode.md#r-bytecode.compact.breakpoint),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order),
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)

r[vm.compiler.patch-kept-with-its-body]
A patch is held with the body itself, keyed by the body and the index,
never by a reclaimable code id; a full collection treats every patched
body's code id as named, and restoring a slot writes the original back
only into the body it was taken from.

Protects:
[`bytecode.compact.breakpoint`](bytecode.md#r-bytecode.compact.breakpoint),
[`bytecode.compact.round-trip`](bytecode.md#r-bytecode.compact.round-trip)

r[vm.compiler.slot-readers-see-original]
Every reader of a body's slots but the run loop's fetch reads the slot a
patch replaced; a reader that matches a body's slots without the decoded
view runs only before a body can be patched or at an index machine code
reaches by a call, which a patched index never is.

Protects:
[`bytecode.compact.breakpoint`](bytecode.md#r-bytecode.compact.breakpoint)

r[vm.compiler.debug-tables-describe-their-body]
A body's debugger tables -- statement starts, sends, variable places --
are used only when the body compiled again from its module's current
text compiles alike to it, a patched slot compared by its original; a
block family gets tables only when its root does, the cache is keyed by
a body kept allocated so no other body takes its address, and every
install clears the cache.

Protects:
[`bytecode.register.layout`](bytecode.md#r-bytecode.register.layout),
[`bytecode.register.arguments`](bytecode.md#r-bytecode.register.arguments),
[`bytecode.body.statement-map`](bytecode.md#r-bytecode.body.statement-map)

r[vm.compiler.compiling-again-changes-nothing]
Compiling a declaration's level again for the debugger registers nothing
-- no mixin, code id, module or class -- runs nothing and installs
nothing, its result only compared; a program's text is resolved by the
ordinary resolver, never the kernel's, a lowering given a debugger
collector emits exactly the code it emits without one, and a text that
does not parse, resolve or hold the declaration answers no tables and
never aborts.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`image.untrusted`](image-format.md#r-image.untrusted),
[`bytecode.body.statement-map`](bytecode.md#r-bytecode.body.statement-map)

r[vm.compiler.debugger-assignment-never-splits-copy]
An assignment the debugger makes to a frame's variable is refused
wherever a closure creation that copies the variable may have run before
the frame's stored program counter, a loop's back edge included, always
in a frame holding such a copy, for a vector variable before its vector
is made, and for a variable that lives nowhere; so no assignment writes
one copy and leaves others stale.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

## Dispatch and the caches

r[vm.dispatch.cache-records-lookup]
A cache entry records exactly the resolution the lookup answered for its
key, with every handle taken out, and a hit runs the one action a miss
runs on that resolution rebuilt from the entry; nothing about a send's
target is decided at a hit that the lookup did not decide, a resolution
that answers nothing is never recorded, and neither is a key whose guard
is none.

Protects: [`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.send`](runtime.md#r-runtime.send),
[`runtime.implicit-receiver`](runtime.md#r-runtime.implicit-receiver),
[`runtime.outer-send`](runtime.md#r-runtime.outer-send),
[`runtime.does-not-understand`](runtime.md#r-runtime.does-not-understand),
[`semantics.send.super`](language/semantics.md#r-semantics.send.super)

r[vm.dispatch.key-covers-every-dependency]
Everything a send's target depends on is covered by its entry's key --
guard, selector, rule and, for a lexical send, the application of the
class the send is written in -- and its site; every read the lookup
makes that the key does not cover is made afresh on every send, hit or
miss, by the interpreter and machine code alike: the class object's
format, builtin field and value refusal at an instantiation, a
primitive's payload check, and a method's fast path read off the method.

Protects: [`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.access.private`](language/semantics.md#r-semantics.access.private),
[`semantics.instantiation.factory`](language/semantics.md#r-semantics.instantiation.factory),
[`semantics.value.conditions`](language/semantics.md#r-semantics.value.conditions)

r[vm.dispatch.guard-names-dependency]
A site's guard is the dispatch id of the application its target depends
on -- the receiver's application for a public, protected or non-private
lexical site (a class object's metaclass's), N's for a private lexical
site, S's for a super site -- per mixin application, never per class
object, and a hit at a private or super site uses a nested-class or slot
index only for a receiver it compared against the guard's application.

Protects:
[`semantics.access.private`](language/semantics.md#r-semantics.access.private),
[`semantics.send.super`](language/semantics.md#r-semantics.send.super),
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.outer-send`](runtime.md#r-runtime.outer-send)

r[vm.dispatch.privacy-decided-once-per-site]
Whether the private rule applies at a lexical site is fixed by its first
resolution and kept by every transition; a resolution whose privacy is
not the site's is never recorded there, and a non-private lexical entry
of the global cache is keyed by N's application as well as the
receiver's id, so it is reused only at a site with the same N.

Protects:
[`semantics.access.private`](language/semantics.md#r-semantics.access.private),
[`runtime.outer-send`](runtime.md#r-runtime.outer-send)

r[vm.dispatch.caches-hold-no-handle]
No cache entry, polymorphic block, global cache row, cell or family map
holds a handle: an entry holds a dispatch id and a target, the defining
class a hit needs is recomputed from the chain's start and the recorded
level, and the caches live in their own heap's table reached through an
activation's feedback base, never in shared code; no cache state,
feedback base or dispatch id is ever written into an image.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`image.load.caches`](image-format.md#r-image.load.caches),
[`image.write.not-written`](image-format.md#r-image.write.not-written)

r[vm.dispatch.header-guard-stamped]
Every object's header holds the dispatch id of its class's application
(a class object's, its metaclass's), stamped at allocation; wherever a
class's application changes, every live instance and the class object
are restamped, by a walk of the space or, for the instances an install
keeps, one pass, before the old id can become reclaimable, a forwarded
object keeping its new object's id; the decoder stamps every decoded
object in one pass before anything dispatches on it, and an id never
comes from an image's bytes.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`image.load.applications`](image-format.md#r-image.load.applications)

r[vm.dispatch.reserved-guards]
Three guard values are never minted for an application: none, which a
value whose class has no application answers and which is never cached;
vacant, which guards an empty site state and which no value answers; and
forwarded, stamped only on a bulk forward's forwarders. A header left
zero matches no entry, and a cell that guards nothing holds a guard word
no compared word can equal.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity)

r[vm.dispatch.invalidation-after-change]
Whatever changes a class object's application or chain, a method table
or a layout while guards stay unchanged -- the bulk forward, and so
every install, even one that migrates nothing -- invalidates every
cache, cell and verified slot-level pair before any code runs again;
invalidation vacates every site and the global cache but keeps every
family's range, since running activations keep their bases, and every
cache carries the mixin application, never a class object or a method
body, as its dependency key.

Protects: [`runtime.install`](runtime.md#r-runtime.install),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`semantics.reflection.atomic-install`](language/semantics.md#r-semantics.reflection.atomic-install),
[`runtime.send`](runtime.md#r-runtime.send),
[`runtime.slot.guard`](runtime.md#r-runtime.slot.guard)

r[vm.dispatch.freed-ids-scrubbed]
A full collection that frees a guard id, an application index or a code
id vacates exactly the cache entries, cells, application-cache entries,
machine entry rows and verified pairs that name it before it can be
minted again, so no entry filled for one application or body is met by
another that took its id.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object)

r[vm.dispatch.application-cache-key]
The application cache is keyed by the mixin's id and the superclass
application's full-width index, never by a class object, an address or
the 22-bit guard, so every class object of one declaration over one
superclass application, and every metaclass of one class side, shares
one application; every identity comparison of applications uses the
full-width index, so an application without a guard stays distinct and
runs uncached.

Protects:
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application),
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object),
[`runtime.apply-mixin`](runtime.md#r-runtime.apply-mixin),
[`semantics.value.identity`](language/semantics.md#r-semantics.value.identity)

r[vm.dispatch.mixin-id-one-per-mixin]
Every version of a mixin answers the one id it was first registered
under: an edit installs a fresh immutable mixin as the id's current
version, never mutating one, the application cache applies the id's
current version on a miss, applications and class objects already built
keep the version they were built from until the install re-applies them,
and a replacement that already has an id, replaces a shipped version or
swaps an instance side with a class side is refused.

Protects:
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application),
[`runtime.install`](runtime.md#r-runtime.install),
[`runtime.apply-mixin`](runtime.md#r-runtime.apply-mixin)

r[vm.dispatch.family-range-own]
A code family's range of the site table belongs to it alone for as long
as its root body, a live closure or an activation names its base; a
range is freed only when no marked closure and no activation names the
base, its sites and polymorphic blocks vacated when it is, a decoded
closure's range following the same rule, and two sends of one family
never share an entry.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`bytecode.call-site`](bytecode.md#r-bytecode.call-site)

r[vm.dispatch.site-within-family]
A send runs uncached when its number is at or past its running body's
family size or its activation's base is unassigned, and every read of
the site table is a checked one, so no site addresses an entry outside
its family's range; the reference lowering's sends are all uncached.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`image.untrusted`](image-format.md#r-image.untrusted),
[`bytecode.call-site`](bytecode.md#r-bytecode.call-site)

r[vm.dispatch.slide-rewrites-every-base]
When a full collection slides the families' ranges down, every holder of
a base -- the family map, decoded closures' ranges, every method target
in a site, a polymorphic block or the global cache, every closure's
feedback-base field, every frame header on the activation stack, a
suspended frame's row, and the running frame's base carried across the
safepoint -- is rewritten within that same collection.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup)

r[vm.dispatch.slot-guard-checked]
Every slot access whose index was decided without looking at the
receiver -- the own-slot instructions, a private or super slot accessor,
a super send's slot handler, the store into an enclosing object's hidden
class slot -- first checks a guard recorded only for a receiver verified
to have the level, against the level the activation was entered at by
full-width index or that level's recorded successor, then the verified
pair, then a walk of the receiver's chain; the slot base comes from that
same entered level, and a failed check raises an ordinary error rather
than writing another layout.

Protects: [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity)

r[vm.dispatch.entered-level-successor]
An activation entered before an install keeps the code it ran and
finishes with it, and its own-slot checks accept the entered level's
successor only when the successor's level keeps the slot base and its
own slots by name and in order, recorded by following successors to the
newest; when the slots changed the slot-guard error is raised.

Protects: [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.install`](runtime.md#r-runtime.install)

r[vm.dispatch.nested-cache-slot-relooked]
After a nested class's superclass clause ran, which may have forwarded
the enclosing object, the hidden cache slot is looked up again in the
enclosing object's current layout before the class is stored, and the
in-progress marker is reset only while that slot is still the current
layout's and holds its own creation's serial.

Protects:
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity)

r[vm.dispatch.hidden-slot-by-declaration]
A nested declaration's hidden class slot is keyed by the declaration's
mixin, never by its name, and is part of the absolute layout of every
application over the declaring level, at the same index in each.

Protects:
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object)

r[vm.dispatch.lexical-origin-from-code]
Whether a body is declaration code -- a body-less declaration's full
initializer and every block created in it -- is derived where the
mixin's bodies are attached, by the compiler and both decoders, never
read from the wire; where a frame's lexical walk starts is read off that
code, never off the class object per send: declaration code starts at
the declaration's enclosing object and declaring class and runs its
lexical sends uncached, every other code at its activation's current
class, and the walk goes on through each class object's declaring class,
never the receiver's dynamic class.

Protects:
[`semantics.outer.lexical`](language/semantics.md#r-semantics.outer.lexical),
[`semantics.send.implicit-receiver`](language/semantics.md#r-semantics.send.implicit-receiver),
[`runtime.outer`](runtime.md#r-runtime.outer),
[`runtime.outer-send`](runtime.md#r-runtime.outer-send),
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application)

r[vm.dispatch.level-facts-agree]
The facts an application copies from its mixin for passing levels
without code, and every other per-application fact, agree with its
mixin, and are computed again for every application an install rebuilds.

Protects:
[`semantics.instantiation.initializer-order`](language/semantics.md#r-semantics.instantiation.initializer-order),
[`semantics.mixin.clause-order`](language/semantics.md#r-semantics.mixin.clause-order)

r[vm.dispatch.declining-leaf-changes-nothing]
A leaf that declines or raises has written nothing -- no register, slot,
element or output -- because it checks every operand before its first
write, so running it again, falling to its failure path or re-running
the instruction that sent it is running it once; a primitive method's
fast path relies on it, since a declining leaf runs again in the
activated body before its failure block.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`runtime.send`](runtime.md#r-runtime.send),
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous)

r[vm.dispatch.future-seen-through]
Once any future exists, every leaf that would answer no, or keep or
render an operand, declines for a future operand, and every failure path
replaces a resolved future operand with its value and runs again, or
forces an unresolved one and runs again, so no program tells a future
from its value; the reflection leaves alone see the future itself.

Protects:
[`semantics.slot.simultaneous`](language/semantics.md#r-semantics.slot.simultaneous),
[`semantics.value.identity`](language/semantics.md#r-semantics.value.identity)

r[vm.dispatch.method-never-changed-in-place]
A method object is never changed in place, so the stub chosen from a
cache entry's target when it is filled is the one a fresh read would
choose; the one exception, a method's fast path the debugger switches
off while a breakpoint stands, invalidates every cache and optimized
copy when it switches.

Protects: [`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.install`](runtime.md#r-runtime.install),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.dispatch.unguarded-sends-receiver-independent]
A lexical, self, outer or private send in machine code skips the
receiver guard only where its resolution does not depend on the
receiver's class.

Protects: [`runtime.lookup`](runtime.md#r-runtime.lookup),
[`semantics.send.implicit-receiver`](language/semantics.md#r-semantics.send.implicit-receiver),
[`semantics.access.private`](language/semantics.md#r-semantics.access.private)

r[vm.dispatch.exception-reports-change-nothing]
The two rows the handler search reports through decide nothing the
search decides: without a debugger the unhandled row answers what its
failure would and the caught row answers the handler and records
nothing, with one the caught row answers the same handler; neither runs
Newspeak code nor collects, and the caught row, which reads another
activation, is an exit, never a leaf.

Protects:
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`semantics.exception.handler`](language/semantics.md#r-semantics.exception.handler),
[`semantics.exception.unhandled`](language/semantics.md#r-semantics.exception.unhandled)

## Machine code

r[vm.machine.observers-after-exit]
Every observer of an activation -- the collector, a suspension, the
unwinder, the leaves that read activations, a deoptimization, an
interpreted callee, the debugger, an image writer -- runs only after
machine code has exited to the run loop, and from that exit on every
frame of the activation stack reads as an interpreted frame word for
word, its program counter written by the exit; a frame that left
optimized code at a poll is sent on in baseline code, its eliminated
blocks made, before anything reads or rebuilds the stack, and an image
carries no machine state.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`image.load.caches`](image-format.md#r-image.load.caches)

r[vm.machine.written-frame-goes-on-interpreted]
A frame the debugger may write -- one an expression is evaluated in, a
variable set in, restarted or forced to return, or the caller a forced
return resumes -- has its compiled mark cleared and goes on interpreted,
never back into optimized code at its poll, whose entry restores values
from the code's constants rather than from the frame.

Protects:
[`bytecode.instruction.store-outer-local`](bytecode.md#r-bytecode.instruction.store-outer-local),
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.machine.baseline-frame-current]
Baseline machine code keeps no value in a machine register across
instructions: every template loads the frame registers it reads and
stores the one it writes, never keeping a frame register only in a
machine register past the point the frame word would hold it, so a
baseline frame holds every value an observer can need at every
instruction and an exit flushes nothing.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.machine.optimized-frame-is-interpreters]
An inlined scope lies where the interpreter would push its activation,
and at every interrupt point of optimized code every scope's words are
the interpreter's: before it every value not yet stored into its frame
word is stored, its receiver and current class written back like
registers, where control meets a binding that does not carry on is
stored on its way in unless the register is dead there, and the point's
metadata names, for every register the leftover analysis does not taint,
its frame word or an eliminated block, a tainted register becoming nil.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers)

r[vm.machine.registers-held-between-frame-nodes]
In optimized code a value is held in a machine register only between two
frame nodes, since a frame node may run code that writes the frame's
registers by reference, moves every object or reifies the frame; after
one every register is read again from its frame word, a phi is made only
for a register some path still reads, and between two frame nodes
nothing writes a register a value is held in but that value's own.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.machine.call-writes-receiver-and-class]
A compiled call writes its callee's receiver and current class into the
callee's header, the current class computed as the interpreter's
materialization computes it, from the chain's start and the entry's
level; every other header word may be derived later from return
addresses, but the current class cannot, since a mixin may be applied
twice in one chain.

Protects:
[`semantics.outer.lexical`](language/semantics.md#r-semantics.outer.lexical),
[`semantics.send.super`](language/semantics.md#r-semantics.send.super),
[`runtime.outer-send`](runtime.md#r-runtime.outer-send),
[`runtime.slot.guard`](runtime.md#r-runtime.slot.guard)

r[vm.machine.inlined-scope-current-class]
An inlined scope's current class is computed before each frame node of
the scope and, by an exit's walk, before the scope's header is written,
whose write records that class's guard and entered level; a scope whose
code addresses its own slots has its own-slot guard reset at its entry,
as a push records none.

Protects: [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`semantics.outer.lexical`](language/semantics.md#r-semantics.outer.lexical),
[`semantics.send.super`](language/semantics.md#r-semantics.send.super)

r[vm.machine.inlined-callee-ends-sender-bindings]
Where an inlined callee is laid over its sender's registers, the
sender's bindings of those registers end there unwritten, so no value of
the sender's is written into the callee's frame nor read after its
return; the frame register moves only around a scope's frame nodes.

Protects:
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`bytecode.register.arguments`](bytecode.md#r-bytecode.register.arguments)

r[vm.machine.exit-writes-every-scope]
The exit's walk writes every inlined scope's header in place, outermost
first -- its code, program counter, caller and top, flags, feedback
base, continuation, guard and entered level, register count and an empty
reified word -- from metadata kept by offset within the body so a
compaction moves it with the code, and the depth count then holds every
scope.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`runtime.handler-search`](runtime.md#r-runtime.handler-search)

r[vm.machine.eliminated-block-made-once]
An optimized body may eliminate a block whose only escape is a trap or
an exit: the block is held, on every way, by registers that hold nothing
else, is read by no frame node, return, operand or type-case, and is
made by the exit that finds it live -- one closure per block per exit,
its home reified and married in place, every register that holds it in
every scope given the same object; a closure creation while an
eliminated block is live exits.

Protects:
[`semantics.literal.block`](language/semantics.md#r-semantics.literal.block),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash)

r[vm.machine.exited-frame-returns-through-loop]
A compiled frame that lived through an exit returns only through the run
loop; a frame compiled code pushed since the latest exit returns by a
bare return, is counted against the depth limit and is reified only by
its own code, so only code with an instruction that reifies its own
frame tests for a divorce.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.method.return`](language/semantics.md#r-semantics.method.return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return)

r[vm.machine.no-fallback-closure]
Machine code creates no fallback closure: an inlined conditional whose
receiver is not a Boolean hands its frame to the interpreter at that
instruction, which runs it again there.

Protects:
[`semantics.value.conditions`](language/semantics.md#r-semantics.value.conditions),
[`semantics.send.ordinary`](language/semantics.md#r-semantics.send.ordinary),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.machine.depth-count-agrees]
The running count of counted activations agrees with the interpreter's
at every check: a compiled push counts and its return uncounts, an
inlined scope is counted by its call's constant adjustment and by the
exit's walk, a copy's prologue compares against the limit less its
deepest inlined chain and leaves for baseline code where that refuses,
an activation an exempting send pushes records its exemption, and the
depth limit word is a bound at or past which the interpreter decides.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.machine.native-stack-room]
Every entry into machine code compares the room left on the thread's
native stack with what the depth limit and its reserve need -- a
return-address slot per counted activation and a margin for the deepest
leaf -- and a thread with less runs the body interpreted, so machine
code reaches the depth limit before the native stack ends.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit)

r[vm.machine.entered-only-at-entries]
Optimized code is entered only at its prologue, just past it at a push,
or at the entry of the poll a frame left it at; a frame that goes on
mid-body anywhere else goes on in its baseline code, which has an entry
at every instruction, and installing optimized code writes afresh every
cell that called the baseline code and clears the body's machine entry
row.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.machine.back-into-optimized-code]
A frame goes back into its optimized code at a poll only if its inlined
scopes are still the stack's innermost chain and none is reified, its
code is the same copy by install serial, its feedback base unchanged,
and the native stack, the depth limit and the stack's end leave room;
the poll's entry restores every value the code past it reads from a
frame word the frame kept or a constant, and a poll that cannot restore
one has no entry.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)

r[vm.machine.on-stack-replacement-at-loops]
An interpreted frame switches into its body's baseline code only at a
backward jump, entering at the loop's head where baseline code holds
every value in its slot, only when the frame's feedback base is the
body's, into the twin for a reified frame, and with room on the native
stack; nothing of the frame is rewritten.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.machine.scope-resumes-at-boundary]
A scope leaving machine code resumes in one of two ways only: the value
of the send at its stored program counter is delivered to that send's
destination, or the instruction at it runs again, after a failed guard,
a trap or a non-Boolean conditional; nothing resumes inside an
instruction, and a trap is an exit whose write-back leaves the frame the
interpreter's word for word.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order)

r[vm.machine.feedback-decides-layout]
The feedback an optimizing compile reads -- cached receivers, executed
and trapped bits, the receivers a fallback saw -- and the loop-head rule
decide only what optimized code loads, stores and how it lays out its
ways, never what it computes, so every tier answers what the interpreter
answers.

Protects:
[`semantics.number.exact`](language/semantics.md#r-semantics.number.exact),
[`semantics.send.ordinary`](language/semantics.md#r-semantics.send.ordinary),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.snapshot-holds-no-handle]
The optimizing compile reads only a snapshot that holds no handle, taken
and compiled at the same safepoint, and its install only appends to the
code zone, so neither collects nor moves code.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.machine.reify-only-in-twin]
A lean body's frame is never reified while it runs: its closure creation
transfers the frame into the body's reifying twin -- optimized code's
after its write-back, a block escaping an inlined scope having its
callee sent instead -- no leaf reifies a frame, and a frame the
interpreter reified runs only twin code; no cell and no machine entry
row names a twin, a twin is freed whenever its lean body is and never
alone, and the lean body's transfer words are written afresh wherever
the twin moves.

Protects:
[`semantics.literal.block`](language/semantics.md#r-semantics.literal.block),
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return)

r[vm.machine.copy-per-receiver-and-level]
A method body's optimized copy is compiled for the pair of the
receiver's application and the application the body was entered at, and
entered only for a receiver of that pair -- a cell's guard, the
dispatching entry's rows, the full lookup past them -- since it reads
own slots at fixed offsets; a copy's request is made for the spending
frame's own key, and a block's, an initializer's, a factory's and a
clause's body have one copy, whose slot accesses keep their guards.

Protects: [`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`semantics.mixin.application`](language/semantics.md#r-semantics.mixin.application),
[`semantics.class.per-enclosing-object`](language/semantics.md#r-semantics.class.per-enclosing-object)

r[vm.machine.unguarded-slot-reads-verified]
A customized copy's own-slot reads, and an inlined callee's field
accesses at the application its arm or known receiver vouched for, are
made without a guard only because the receiver's application was
verified at entry, any forward of the receiver runs in a mirror leaf
machine code exits for, after which the frame goes on in baseline code,
and every instance of a program class holds its application's slot
count.

Protects: [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`image.verify.program-instances`](image-format.md#r-image.verify.program-instances)

r[vm.machine.dispatching-rows]
A body's block of dispatching rows is written only from the copies of
the body whose dispatching word names it, afresh at every install,
retirement, invalidation and move of a copy, and made vacant once the
body is freed, its code dies or its code id is freed; a vacant row holds
a receiver word no dispatch id is, and a compaction remaps every row's
entry.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read)

r[vm.machine.stale-copies-freed]
Whatever changes something an optimized copy relied on frees it at once,
its cells, dispatching rows and machine entry row written afresh to
enter baseline code: an install frees every copy whose key or recorded
dependencies name an application it rebuilt, the bulk forward every
copy, a full collection that reclaims an application id every key row
and copy naming it, a slide of a feedback base every copy that inlined a
body against it, and a breakpoint set or removed every copy naming the
body; each runs where no return address into the code zone exists.

Protects: [`runtime.install`](runtime.md#r-runtime.install),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`semantics.reflection.atomic-install`](language/semantics.md#r-semantics.reflection.atomic-install),
[`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.slot.guard`](runtime.md#r-runtime.slot.guard)

r[vm.machine.code-ids-kept-with-code]
A body with machine code keeps a code id -- and its rows in the machine
entry table, the optimization budgets and the dispatching rows -- for as
long as its code lives, a body a live optimized copy inlined included
even when no activation runs it; a full collection that frees the id of
a body nothing runs mints one again for each body whose code lives, and
the zone frees a copy's metadata, dependencies and poll entries with the
copy.

Protects: [`runtime.invoke`](runtime.md#r-runtime.invoke),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.observed-never-inlined]
A body with an unwind marker and every kernel method whose activation a
leaf reads or marks are never inlined, so every activation the exception
machinery reads is a whole frame; the handler search reifies only the
handler activation it answers and the unwinder reifies nothing.

Protects:
[`runtime.handler-search`](runtime.md#r-runtime.handler-search),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.exception.handler`](language/semantics.md#r-semantics.exception.handler),
[`semantics.exception.ensure`](language/semantics.md#r-semantics.exception.ensure)

r[vm.machine.budget-words-name-own-row]
Machine code never names an optimization budget row by code id: each
copy's budget word holds its body's row address, pointed before the code
first runs and afresh after every full collection at the id the body
then has, an unpointed word naming a word that is always negative, every
such word written only through the code zone's writer.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.machine.decline-before-write]
Every template, stub and prologue of machine code declines or exits
before it has written anything the interpreter's run of the instruction
would see -- an integer fast path on a non-small-integer operand, an
overflow or a zero divisor; an indexed access the interpreter would
refuse; an own-slot access whose receiver lacks the level; a prologue's
depth, room or safepoint refusal; a stub's every decline; a closure stub
whose argument copies would pass the ordinary end -- a float operation
having written only its own result and spare registers.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.number.exact`](language/semantics.md#r-semantics.number.exact),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order)

r[vm.machine.leaf-contract]
An instruction is lowered as a leaf only if its step runs no Newspeak
code, cannot collect, reads no other activation and pushes nothing; a
leaf that allocates places as the interpreter's cold path does, its
scavenge left to the next poll's exit, only the miss, initializer,
forcing and twin leaves compile and install code, and a leaf answers
"run again" only before it changed anything but a send's argument slots,
so a mirror row that ends the run or whose effect must not repeat stays
an exit.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`runtime.send`](runtime.md#r-runtime.send),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`semantics.send.evaluation-order`](language/semantics.md#r-semantics.send.evaluation-order)

r[vm.machine.own-slot-access-guarded]
Machine code's own-slot access compares the receiver's guard with the
one its frame recorded, calls the cold check otherwise, and accesses a
field only if it is one of the receiver's counted fields; the exit's
walk and a leaf's view leave a recorded guard and entered level alone,
and an initializing store reads the flags of the receiver's class's
application, not the entered level's.

Protects: [`runtime.slot.guard`](runtime.md#r-runtime.slot.guard),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`semantics.value.conditions`](language/semantics.md#r-semantics.value.conditions)

r[vm.machine.cells-follow-entries]
Every site entry, polymorphic slot and global cache entry has a cell
machine code reads, written in the same operation as its entry; a cell's
every word is a copy of its entry and its entry address a function of
the entry's guard and target and of the JIT's state alone, never of the
send or the receiver, written afresh whenever that state changes under
it -- an install, a retirement, an abandonment, a revival, a freeing, a
compaction's remap -- freed code's cells naming the miss stub, and the
megamorphic stub probing exactly as the global cache does through rows
written and swapped with its entries.

Protects: [`runtime.lookup`](runtime.md#r-runtime.lookup),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.cell-addresses-fixed]
A cell's address is fixed for the life of its reservation, every cell a
table grows by is written vacant before machine code can read it, and
machine code reads a site's cell only for a live range and a block's
only for one a site names.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup)

r[vm.machine.stub-does-what-act-does]
A stub a cell names does for its target what the interpreter's act does
and nothing more and stores into no frame; a compiled send enters its
callee with its own site's cell named, and a stub acts from the site's
entry only when the send hit that cell, a miss clearing the naming and
entering the callee with no cell, since the site's entry need not be the
decision for this receiver.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.slot.guard`](runtime.md#r-runtime.slot.guard)

r[vm.machine.switch-is-safepoint]
A push or a return the run loop switches into machine code at first
takes a real safepoint request made meanwhile, as the interpreter would
before its next instruction, since machine code polls only at its loops
and pushes; a mirror row whose effect relies on its return being a
safepoint stays an exit.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.ephemeron`](runtime.md#r-runtime.memory.ephemeron)

r[vm.machine.frame-records-kind]
A frame machine code pushes records its kind and operand -- a method's
selector, a factory's or an initializer's tag and current class -- in
its flags word from what its call handed it, since one code can be two
of its mixin's bodies, and the exit's walk keeps the recorded tag.

Protects: [`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`image.verify.frames`](image-format.md#r-image.verify.frames)

r[vm.machine.entry-returns-to-its-caller]
Every entry into machine code returns to the native frame that made it:
no native frame lies between an entry and its compiled frames but a leaf
call that returned before any exit, the exit resets the native stack
pointer to the one the entry saved, and a failure of an internal check
in a leaf aborts the process before it unwinds through machine code.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

Undetected: a native frame skipped without returning through it is
undefined behaviour no processor reports, and no sanitizer runs machine
code.

r[vm.machine.entry-block-current]
The entry block's lasting words -- the object space's, activation
stack's, machine words' and bump cursor's addresses, the exit stub, the
site cells, the polymorphic stub and blocks, the global cache's rows and
mask -- are written only when what they name changes, every such change
marking the block stale so the next entry writes them all; they name
only stubs at the zone's base, which never move, and no handle.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.invoke`](runtime.md#r-runtime.invoke)

r[vm.machine.machine-words-outside-mutator]
Every word machine code reads or writes between two leaf calls lies
outside the mutator: the machine words in a shared allocation accessed
with relaxed order but for the request word's atomic changes, the entry
block, eden's cursor, the cells, the id and entry tables, the budget
rows and the reservations of the object space and the activation stack,
whose addresses come from each reservation's own base, no view of a
reservation alive while machine code runs.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`semantics.object`](language/semantics.md#r-semantics.object)

Undetected: machine code's access to a word inside the mutator, which
the run loop holds exclusively while it runs, is undefined behaviour
invisible to every processor, and no sanitizer runs machine code.

r[vm.machine.class-words-rewritten]
The class objects machine code writes into new objects and compares
against -- the immediates' classes, the array, float, closure, frame and
future classes, held among the machine words as stored words the
collector does not trace -- are rewritten after every scavenge,
compaction, move and bulk forward, and the words handed between machine
code and a leaf are read only right after they are written, with no
collection between.

Protects:
[`semantics.object`](language/semantics.md#r-semantics.object),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.safepoint-request-bits]
The safepoint request word is a set of independent bits, each set by an
atomic or and cleared by an atomic and only by the party that served it,
so no writer erases another's; the safepoint clears only its own request
and the stress hold, the depth limit word machine code's prologue
compares with is zero exactly while a bit that lowers it is set and
otherwise the limit in force -- an evaluation's reserve added only while
it runs -- every write of the request or the limit followed by a
sequentially consistent fence and a second read, and a backward jump in
machine code compares the whole request word with zero.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`semantics.depth-limit`](language/semantics.md#r-semantics.depth-limit),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.machine.install-appends]
Code is installed by appending through the zone's writable view only, so
machine code holds no address an install moves and a leaf may install
code while machine code runs; on AArch64 the instruction cache is
flushed over every installed range.

Protects: [`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.body-entered-in-its-zone]
Every code zone has an identity of its own, never 0, which a body
records; a body compiled by another interpreter is compiled again into
this zone rather than entered at an offset that means nothing here.

Protects: [`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.zone-keeps-no-body-alive]
The code zone holds its bodies weakly and records no code id, asking the
heap for a body's id afresh, since a full collection may free an id and
mint it again for another body; a body with a compiled frame is one that
runs, and a cache entry holds its method and so its code, so no cell
names a dead body.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak)

r[vm.machine.no-return-address-at-safepoint]
At a safepoint -- a debugger's stop among them -- no compiled frame
keeps a return address into the code zone, so the zone may be changed
there and the native stack's use above a stop is bounded by the counted
depth alone.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.zone-compacted-at-safepoint]
The code zone is compacted, and a body freed, only at the run loop's
safepoint or between entries, where no return address into the zone
exists; a compile from inside a leaf only requests the compaction.

Protects: [`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.compaction-remaps-every-holder]
A compaction remaps every address outside the bodies -- every cell,
machine entry row and dispatching row -- through its plan before any
byte moves, a freed body's entry becoming the miss stub's and its row
empty; it fills every byte the survivors no longer cover with stops, so
an address a holder failed to remap traps, and it rewrites the JIT's own
records, the lean bodies' transfer words among them.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup)

r[vm.machine.frame-names-body-by-code]
A compiled frame names its body by its code and program counter, never
by an address, so a frame stopped in code a compaction, an abandonment
or a moved feedback base freed resumes interpreted; a leaf that finds
its calling body by an address answers the body whose code occupies that
address now, a remembered answer taken only when its body still holds
the address and is not freed.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.unwind`](runtime.md#r-runtime.unwind)

r[vm.machine.moved-base-frees-code]
A full collection that slides the site ranges frees the machine code of
every body compiled against a feedback base that moved or that no live
range holds, clears its machine entry row and writes afresh every cell
that named it, before machine code runs again; nothing else in the zone
holds a base.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.lookup`](runtime.md#r-runtime.lookup)

r[vm.machine.code-ids-read-from-sites]
A closure creation's and an outer local's code ids, and an outer local's
argument count, are read from a site among the body's data words stamped
with the code table's epoch they were asked in, never compiled into the
code, and asked again where the site's epoch differs from the heap's, no
heap's epoch being 0; a site is written only through the zone's writer
and moves and is freed with its body.

Protects:
[`semantics.literal.block`](language/semantics.md#r-semantics.literal.block),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local)

r[vm.machine.machine-entry-rows]
A row of the machine entry table is written only from the body its code
id names, when the interpreter pushes a block whose body has machine
code, with the feedback base the body was compiled against, and cleared
wherever that stops naming the body's code -- the id freed, the body
abandoned or freed, the zone replaced, the JIT switched on or off; the
bit beside each row is set and cleared with it, and a closure call
compares the closure's feedback base with the row's.

Protects:
[`semantics.literal.block`](language/semantics.md#r-semantics.literal.block),
[`runtime.send`](runtime.md#r-runtime.send)

r[vm.machine.block-frame-room]
A block frame a closure stub lays past the send's slots may reach higher
than the interpreter's push of the same block, so its prologue checks
the stack's room where the frame lies; the stub copies the block's
arguments only where they end at or below the ordinary end and declines
otherwise before writing anything, and a call the prologue refuses has
written only registers dead after any send.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`bytecode.register.leftovers`](bytecode.md#r-bytecode.register.leftovers),
[`bytecode.register.arguments`](bytecode.md#r-bytecode.register.arguments)

r[vm.machine.outer-local-at-its-frame]
Machine code reads and writes an outer local where the interpreter's
walk finds it: in a married frame the register on the activation stack,
a store needing no barrier; in a divorced frame the frame object's field
for that register, a store through the write barrier; in a suspended
frame its frame object.

Protects:
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure),
[`bytecode.instruction.load-outer-local`](bytecode.md#r-bytecode.instruction.load-outer-local),
[`bytecode.instruction.store-outer-local`](bytecode.md#r-bytecode.instruction.store-outer-local)

r[vm.machine.allocation-as-interpreter]
Machine code allocates by bumping eden's cursor inline against the limit
the interpreter's fast path bumps against, writing header and class word
as the interpreter does; past the limit a leaf makes the interpreter's
cold-path decision -- request a scavenge, allocate old and born
remembered once eden is spent -- and never collects, and a store into an
object machine code has just allocated takes no barrier.

Protects: [`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.memory.identity-hash`](runtime.md#r-runtime.memory.identity-hash),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.machine.float-rows-as-leaf]
A float operation machine code computes, in a row stub or an optimized
node, answers bit for bit what its row's leaf answers, or declines to
the leaf; a double lives in machine code only in registers every
convention lets a callee clobber, never across a leaf call, an exit or a
node's end.

Protects:
[`semantics.literal.float`](language/semantics.md#r-semantics.literal.float),
[`semantics.number.exact`](language/semantics.md#r-semantics.number.exact)

r[vm.machine.return-address-slot]
Every address a call reaches begins by saving the return address in its
slot, a stub's tail jump into one goes past that save, and a call into a
body's middle saves it itself; the stack pointer stays aligned, so every
walk of the native stack -- the exit's, the cut's, the lookup by return
address -- reads one slot per compiled frame.

Protects: [`runtime.unwind`](runtime.md#r-runtime.unwind),
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return)

r[vm.machine.cut-pops-as-returns-pop]
Machine code's non-local return changes nothing before it has found the
home among the compiled frames of the current stretch; it declines,
leaving the return to the one unwinder, for a body that is no block's, a
block that is the entry frame, a home not running, suspended or below
the entry frame, an unwind marker's activation between, a return address
into a stub before the home, a home that catches no return and a frame
of a method a method breakpoint names; found, it pops each frame as its
return pops it, divorcing a reified one and taking its count off, and
runs no Newspeak code, never collects and never pushes.

Protects:
[`semantics.block.non-local-return`](language/semantics.md#r-semantics.block.non-local-return),
[`semantics.block.cannot-return`](language/semantics.md#r-semantics.block.cannot-return),
[`semantics.exception.ensure`](language/semantics.md#r-semantics.exception.ensure),
[`runtime.unwind`](runtime.md#r-runtime.unwind),
[`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)

r[vm.machine.application-id-table]
A row of the application id table, by which machine code reads an
application's slot base and instance header, is written when its
application is registered, set to the id that names no application when
a full collection reclaims it, and never otherwise changes -- an edit
makes a new application -- its header word 0 wherever the allocation
would make no ordinary instance; an index past the rows recorded or a
word that is no small integer names no application and goes to a leaf
that decides.

Protects: [`runtime.slot.read`](runtime.md#r-runtime.slot.read),
[`runtime.slot.write`](runtime.md#r-runtime.slot.write),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate),
[`semantics.value.conditions`](language/semantics.md#r-semantics.value.conditions)

r[vm.machine.bounds-lowered-before-decommit]
Before any page of a reservation beside the object space is given back,
every bound machine code compares against there -- the activation
stack's ordinary end, the machine entry rows, the application id rows --
is lowered, so machine code reads only committed memory; a page
committed again is written before it is read, the stack shrinks only
between entries, and no code runs from a code-zone page given back
before an install writes it again.

Protects: [`runtime.depth-limit`](runtime.md#r-runtime.depth-limit),
[`runtime.send`](runtime.md#r-runtime.send),
[`semantics.block.closure`](language/semantics.md#r-semantics.block.closure)

r[vm.machine.code-holds-no-handle]
Machine code holds no handle -- a literal object is loaded through the
heap's tables -- and no absolute address of its own: stubs and leaves
are reached through the entry block, callees through cells, a body's
data words relative to its own instructions; so no collection patches
code and a compaction moves a body unchanged.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`semantics.object`](language/semantics.md#r-semantics.object)

r[vm.machine.tracing-changes-nothing]
Recording a trace event or a listing inside a leaf only queues it,
requests no safepoint, polls none and calls no host code; delivery to
the host's sink happens only outside machine code, and annotating a
compile for a listing emits no byte, so machine code and the run are the
same with and without tracing and printing.

Protects: [`runtime.send`](runtime.md#r-runtime.send),
[`runtime.memory.weak`](runtime.md#r-runtime.memory.weak),
[`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror)

## The VM mirror

r[vm.mirror.binding-only-in-shipped-resolution]
Only the resolution made for the shipped core library's source records
the binding that marks a send to the VM mirror, and only such a send
compiles to the mirror-send instruction; a program's module, the project
manifest, every edit an install compiles and every resolution for an
editor are resolved the ordinary way, so copying shipped text yields an
ordinary send to whatever that module's own slot holds.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.reflection.mirrors`](language/semantics.md#r-semantics.reflection.mirrors),
[`image.verify.mirror`](image-format.md#r-image.verify.mirror)

r[vm.mirror.shipped-text-by-exact-copy]
Text is compiled with the shipped resolution only when it is byte for
byte one of the binary's own shipped texts, and then the binary's own
text is what compiles; no text a user writes, even one byte away from a
shipped text, is resolved as shipped code.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`image.verify.mirror`](image-format.md#r-image.verify.mirror)

r[vm.mirror.shipped-mixins-never-edited]
The VM mirror's class and method table are built once at bootstrap and
never change, and every edit of a shipped mixin is refused, in the
reflective builder and again in the runtime, an edit being resolved
without the mirror-send binding; so a mirror send bound to its row, a
primitive method's row and a sent mirror send always run the same leaf,
and no edit records a binding that grants the mirror's leaves.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.reflection.mirrors`](language/semantics.md#r-semantics.reflection.mirrors),
[`runtime.install`](runtime.md#r-runtime.install)

r[vm.mirror.forward-refuses-shipped]
The bulk forward refuses, on either side, anything but an ordinary
fixed-pointer instance of a class that is no builtin and whose mixin is
not shipped, and every value the mirrors withhold -- the VM mirror and
the shipped module instances among them -- so a forward never puts the
mirror where a program reaches it nor changes an object whose layout the
runtime reads natively.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`runtime.forward-identity`](runtime.md#r-runtime.forward-identity)

r[vm.mirror.slot-checked-before-recording]
The mirror slot of the kernel root and of every shipped module given the
mirror is checked -- immutable, eager, holding the mirror -- before the
module's instance is recorded, at boot, on first demand or by the
loader, so an instance that fails the check is never recorded and every
object whose code sends to the mirror is one whose slot was checked.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror)

r[vm.mirror.one-instance-per-shipped-module]
Each shipped module has exactly one recorded instance in the shipped
world, the same object on every request; nothing is recorded before its
factory answered -- a demand interrupted by the depth limit records
nothing and the next demand instantiates the pending class again -- and
only an instance of the pending root class is recorded.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.program.platform`](language/semantics.md#r-semantics.program.platform),
[`image.load.shipped-world`](image-format.md#r-image.load.shipped-world)

r[vm.mirror.root-instance-unreachable]
No program reaches a shipped module's root instance or root class, nor
the mirror: the facets' class accessors answer nested classes only, no
code whose receiver is a root instance sends it a message it may not
understand, and neither the mirror nor the kernel instance is ever the
argument of a send to a receiver that may not understand it, because the
resulting does-not-understand hands its receiver and arguments to
whoever catches it.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.program.platform`](language/semantics.md#r-semantics.program.platform),
[`semantics.send.does-not-understand`](language/semantics.md#r-semantics.send.does-not-understand)

r[vm.mirror.facet-classes-private]
A shipped module that programs receive only as a restricted facet keeps
every nested class carrying the module's full authority private: no lazy
slot, class accessor or class of any object it hands out makes such a
class reachable, since a class object carries its enclosing object's
authority.

Protects:
[`semantics.program.platform`](language/semantics.md#r-semantics.program.platform),
[`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror)

r[vm.mirror.class-unnameable]
No program can name the VM mirror's class or `Top`: `Object` declares no
accessor for either, and the mirror's class has no declaration and so no
shipped name an image could carry.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.builtin-names`](language/semantics.md#r-semantics.builtin-names)

r[vm.mirror.not-instantiable]
The allocation of a factory body, which a program's send and a host's
instantiation both run, refuses the VM mirror's class, so there is
exactly one mirror; the refusal carries nil rather than the class, and
machine code's inline instantiation sends every builtin's own instances
through that allocation.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`runtime.instantiate`](runtime.md#r-runtime.instantiate)

Undetected: no test yet: from a shipped-library test fixture that
reaches the mirror's class, send it its factory and a host's
instantiate, and check both are refused with an error naming no class.

r[vm.mirror.no-wire-tag]
The instruction that runs a leaf with no lookup has no wire tag, so no
image decodes into one; the embedded shipped code carries it in a codec
of its own that only the boot reaches with the binary's own bytes.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`bytecode.verify.tags`](bytecode.md#r-bytecode.verify.tags),
[`bytecode.wire.tags`](bytecode.md#r-bytecode.wire.tags)

Undetected: no test yet: encode a body holding a mirror send through the
image encoder and check it is refused, and scan the pinned tag table for
a mirror-send entry.

r[vm.mirror.shipped-source-only-fast-paths]
Only a method of the shipped core library's source carries a fast path
-- a mirror row, a builtin class accessor, a quick method -- because
recognition runs only under the shipped resolution and the accessor form
can be written only by the kernel; every other method, and every method
decoded from an image, carries none, so a program never gains a method
that runs a leaf without an activation.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`image.untrusted`](image-format.md#r-image.untrusted)

r[vm.mirror.debugger-acts-in-program-frames]
Evaluating, setting a variable, restarting and forcing a return are
refused in a frame whose body belongs to a shipped module, decided from
its declaration's module before anything is compiled or begun; an
evaluated expression obeys access control as the frame's method does.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.access.private`](language/semantics.md#r-semantics.access.private),
[`semantics.access.protected`](language/semantics.md#r-semantics.access.protected)

r[vm.mirror.debugger-withholds-shipped-values]
A value whose way to the debugger passed through a shipped activation or
a field a shipped mixin declares, every value the shipped world
withholds (the mirror, its class and metaclass, every recorded shipped
module instance), every frame object and a closure of a shipped body are
withheld, a child inheriting it, and a withheld value is never marked,
named in an expression nor forced as a frame's return.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.gap.reflection.activation`](language/semantics.md#r-semantics.gap.reflection.activation)

r[vm.mirror.mirrors-withhold-shipped-values]
No mirror hands out a value it reached through a shipped level: a mirror
reads a value only through a level whose mixin is not shipped, a class
mirror reached by navigation holds the way to its class and never the
class, and the leaves that answer a slot's value or an enclosing object
refuse the mirror and every recorded shipped module instance whatever
the level.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.reflection.mirrors`](language/semantics.md#r-semantics.reflection.mirrors)

r[vm.mirror.shipped-decided-by-root]
Whether a module, a body or a value is shipped -- for withholding,
refusals and filters -- is decided by its module's root mixin's shipped
flag, never by its id or name, and a program's module declaring an id
under the reserved namespace, or given one, is refused before anything
runs.

Protects: [`runtime.vm-mirror`](runtime.md#r-runtime.vm-mirror),
[`semantics.module.identity`](language/semantics.md#r-semantics.module.identity),
[`image.verify.module-identity`](image-format.md#r-image.verify.module-identity)

## The image loader

r[vm.image.nothing-collects-while-placeholders]
A load allocates nothing in the object space while it reads up to the
graph, builds every shipped object the image names before one tenure
between two entries, allocates nothing between that tenure and the graph
pass, and the graph pass allocates every placeholder old and never polls
a safepoint; so nothing collects or moves while placeholders exist, and
every word a fill stores names an old object and needs no barrier.

Protects: [`image.bound.sizing`](image-format.md#r-image.bound.sizing),
[`image.bound.linear`](image-format.md#r-image.bound.linear),
[`image.untrusted`](image-format.md#r-image.untrusted),
[`image.load.shipped-world`](image-format.md#r-image.load.shipped-world)

r[vm.image.code-runs-after-stamping]
A frame record's body is filled only after the applications are rebuilt
and every decoded object's header is stamped, and decoded code runs only
after every structural pass -- the code table, the mixins, the graph,
the program instances -- has verified it.

Protects: [`image.load`](image-format.md#r-image.load),
[`image.run`](image-format.md#r-image.run),
[`image.verify.frames`](image-format.md#r-image.verify.frames),
[`runtime.send`](runtime.md#r-runtime.send)

## Detectors

What finds the breach of a rule beside its tests: each stress mode the
stress gate runs the suite under, each check a debug build makes, and
each check the compiler of the implementation's language makes, with the
rules whose breach it finds.

```spec-table vm-detectors
| Detector | Kind | What it checks | Rules |
|---|---|---|---|
| MOVES | stress | At the safepoints its schedule picks, shifts every object of the space by a varied offset and advances the move epoch, so a handle or stored word not rewritten names the old place and a debug build's epoch check refuses it where it is next used. | `vm.heap.collection-only-at-safepoint`, `vm.heap.roots-complete`, `vm.heap.format-decides-pointer-words`, `vm.heap.non-pointer-words-untraced`, `vm.heap.remembered-set-exact`, `vm.heap.symbol-canonical`, `vm.heap.reified-mixin-canonical`, `vm.heap.every-mover-realigns`, `vm.heap.bump-limit-within-committed`, `vm.heap.identity-hash-moves-with-object`, `vm.handle.nothing-carried-across-safepoint`, `vm.handle.host-holds-only-roots`, `vm.handle.stored-only-when-current`, `vm.handle.only-where-rewritable`, `vm.activation.frame-words-tagged`, `vm.activation.marriage`, `vm.compiler.code-holds-no-handle`, `vm.dispatch.caches-hold-no-handle`, `vm.machine.registers-held-between-frame-nodes`, `vm.machine.class-words-rewritten`, `vm.machine.code-holds-no-handle` |
| CACHES | stress | Vacates every inline cache, cell and verified slot-level pair at every safepoint, so every send fills afresh and a fill, transition or hit that departs from the lookup's answer changes an outcome the suite pins. | `vm.activation.unwind-marker-from-code`, `vm.dispatch.cache-records-lookup`, `vm.dispatch.key-covers-every-dependency`, `vm.dispatch.guard-names-dependency`, `vm.dispatch.privacy-decided-once-per-site`, `vm.dispatch.slot-guard-checked`, `vm.machine.cells-follow-entries`, `vm.machine.stub-does-what-act-does` |
| COLLECT | stress | Runs a full collection -- marking, weak settling, id reclaim, compaction, site-range slide -- at the safepoints its schedule picks, so a root, id or base a collection misses is lost, minted again or left stale under a pinned outcome. | `vm.heap.collection-only-at-safepoint`, `vm.heap.roots-complete`, `vm.heap.mark-bit-clear-outside-collection`, `vm.heap.forwarding-table-exact`, `vm.heap.resolved-future-replaced-in-fields`, `vm.heap.resolved-future-judged-by-value`, `vm.heap.id-reclaimed-only-when-unnamed`, `vm.heap.weak-settled-before-compaction`, `vm.heap.symbol-canonical`, `vm.heap.symbol-hash-from-text`, `vm.heap.reified-mixin-canonical`, `vm.heap.identity-hash-moves-with-object`, `vm.handle.nothing-carried-across-safepoint`, `vm.handle.stored-only-when-current`, `vm.activation.frame-words-tagged`, `vm.activation.dead-registers-nilled`, `vm.activation.top-at-window-end`, `vm.activation.marriage`, `vm.compiler.code-id-cache-current`, `vm.compiler.patch-kept-with-its-body`, `vm.dispatch.freed-ids-scrubbed`, `vm.dispatch.application-cache-key`, `vm.dispatch.family-range-own`, `vm.dispatch.slide-rewrites-every-base`, `vm.machine.back-into-optimized-code`, `vm.machine.code-ids-kept-with-code`, `vm.machine.budget-words-name-own-row`, `vm.machine.cell-addresses-fixed`, `vm.machine.class-words-rewritten`, `vm.machine.zone-keeps-no-body-alive`, `vm.machine.moved-base-frees-code`, `vm.machine.code-ids-read-from-sites`, `vm.machine.machine-entry-rows`, `vm.machine.application-id-table`, `vm.machine.bounds-lowered-before-decommit` |
| SCAVENGE | stress | Verifies the remembered set against every old object, then scavenges, at the safepoints its schedule picks, so a store that bypassed the barrier is reported and a young object reached only through it is lost. | `vm.heap.collection-only-at-safepoint`, `vm.heap.roots-complete`, `vm.heap.format-decides-pointer-words`, `vm.heap.resolved-future-judged-by-value`, `vm.heap.barrier`, `vm.heap.frame-copies-remembered`, `vm.heap.old-allocations-born-remembered`, `vm.heap.remembered-set-exact`, `vm.heap.no-forwarder-reached`, `vm.heap.bulk-forward-atomic`, `vm.heap.weak-settled-before-nursery-vacated`, `vm.heap.symbol-canonical`, `vm.heap.identity-hash-moves-with-object`, `vm.handle.nothing-carried-across-safepoint`, `vm.handle.stored-only-when-current`, `vm.activation.frame-words-tagged`, `vm.activation.dead-registers-nilled`, `vm.activation.top-at-window-end`, `vm.machine.baseline-frame-current`, `vm.machine.class-words-rewritten`, `vm.machine.outer-local-at-its-frame`, `vm.machine.allocation-as-interpreter` |
| WIDE | stress | Builds every instruction in its wide form, so the whole suite runs the wide arm, the four wide send forms and the reach derived for them. | `vm.activation.outer-reach` |
| SUSPEND | stress | Suspends every activation of the run into its frame object at the safepoints its schedule picks and resumes the top one, so every program runs on from suspended frames as a deoptimized one will. | `vm.heap.roots-complete`, `vm.heap.id-reclaimed-only-when-unnamed`, `vm.heap.frame-copies-remembered`, `vm.activation.depth-count-exact`, `vm.activation.no-suspension-outlives-run`, `vm.activation.marriage`, `vm.activation.frame-object-full-register-file`, `vm.activation.suspension-copies-registers`, `vm.activation.resume-derives-state`, `vm.activation.sender-walks-reach-suspended`, `vm.activation.program-counter-before-observation`, `vm.machine.back-into-optimized-code` |
| JIT | stress | Compiles every body at its first entry and enters its machine code at every push, even code that exits at once, so the entry, the stubs, the exit and the resumption run under every golden. | `vm.dispatch.key-covers-every-dependency`, `vm.dispatch.unguarded-sends-receiver-independent`, `vm.dispatch.exception-reports-change-nothing`, `vm.machine.observers-after-exit`, `vm.machine.call-writes-receiver-and-class`, `vm.machine.no-fallback-closure`, `vm.machine.depth-count-agrees`, `vm.machine.on-stack-replacement-at-loops`, `vm.machine.decline-before-write`, `vm.machine.leaf-contract`, `vm.machine.own-slot-access-guarded`, `vm.machine.cells-follow-entries`, `vm.machine.stub-does-what-act-does`, `vm.machine.block-frame-room`, `vm.machine.outer-local-at-its-frame`, `vm.machine.cut-pops-as-returns-pop`, `vm.machine.application-id-table` |
| EXIT | stress | Makes every poll in machine code -- every backward jump it takes -- take its exit as if a safepoint were requested, so machine code hands its frames to the interpreter far more often than a run does. | `vm.activation.program-counter-before-observation`, `vm.activation.reexecuted-instruction-wrote-nothing`, `vm.dispatch.declining-leaf-changes-nothing`, `vm.machine.observers-after-exit`, `vm.machine.baseline-frame-current`, `vm.machine.exited-frame-returns-through-loop`, `vm.machine.decline-before-write`, `vm.machine.switch-is-safepoint`, `vm.machine.frame-records-kind`, `vm.machine.safepoint-request-bits` |
| ZONE | stress | Compacts the code zone at the safepoints its schedule picks, freeing every body no activation runs, so every program runs with its bodies freed, compiled again and moved. | `vm.machine.back-into-optimized-code`, `vm.machine.reify-only-in-twin`, `vm.machine.dispatching-rows`, `vm.machine.code-ids-kept-with-code`, `vm.machine.cells-follow-entries`, `vm.machine.zone-keeps-no-body-alive`, `vm.machine.no-return-address-at-safepoint`, `vm.machine.zone-compacted-at-safepoint`, `vm.machine.compaction-remaps-every-holder`, `vm.machine.frame-names-body-by-code`, `vm.machine.code-ids-read-from-sites`, `vm.machine.machine-entry-rows`, `vm.machine.bounds-lowered-before-decommit`, `vm.machine.code-holds-no-handle` |
| OPTIMIZE | stress | Arms every body's optimization budget at 0, so its first spend requests an optimized copy the next safepoint compiles and installs. | `vm.compiler.mixin-relative-slot-index`, `vm.machine.depth-count-agrees`, `vm.machine.entered-only-at-entries`, `vm.machine.feedback-decides-layout`, `vm.machine.snapshot-holds-no-handle`, `vm.machine.copy-per-receiver-and-level`, `vm.machine.code-ids-kept-with-code`, `vm.machine.observed-never-inlined`, `vm.machine.budget-words-name-own-row`, `vm.machine.float-rows-as-leaf` |
| DEOPT | stress | Implies OPTIMIZE; optimized code leaves at every n-th interrupt point it passes, n drawn afresh at every safepoint from a fixed rotation, so every interrupt point's metadata and write-back are used. | `vm.activation.reexecuted-instruction-wrote-nothing`, `vm.machine.observers-after-exit`, `vm.machine.optimized-frame-is-interpreters`, `vm.machine.inlined-scope-current-class`, `vm.machine.inlined-callee-ends-sender-bindings`, `vm.machine.exit-writes-every-scope`, `vm.machine.eliminated-block-made-once`, `vm.machine.entered-only-at-entries`, `vm.machine.scope-resumes-at-boundary`, `vm.machine.feedback-decides-layout` |
| INVALIDATE | stress | Implies OPTIMIZE; after a fixed number of interrupt points optimized code leaves for a safepoint that frees every copy, so copies are freed while frames still run them and bodies are optimized afresh. | `vm.compiler.mixin-relative-slot-index`, `vm.machine.copy-per-receiver-and-level`, `vm.machine.unguarded-slot-reads-verified`, `vm.machine.stale-copies-freed`, `vm.machine.own-slot-access-guarded` |
| PAUSE | stress | Stops the run as for a debugger's pause at the safepoints its schedule picks, reads the whole stack and every value one level deep, evaluates in the top frame, and continues. | `vm.heap.stop-runs-no-code`, `vm.handle.debugger-holds-no-bare-handle`, `vm.activation.debugger-reads-no-dead-register`, `vm.activation.evaluation-above-every-window`, `vm.compiler.debug-tables-describe-their-body`, `vm.machine.observers-after-exit`, `vm.machine.written-frame-goes-on-interpreted`, `vm.machine.safepoint-request-bits`, `vm.machine.no-return-address-at-safepoint` |
| BREAKPOINTS | stress | At every safepoint patches every statement start of each running body not yet patched with a breakpoint that never stops, so every golden runs through the breakpoint arm and patched bodies. | `vm.compiler.patched-slot-runs-original-once`, `vm.compiler.patch-kept-with-its-body`, `vm.compiler.slot-readers-see-original`, `vm.dispatch.method-never-changed-in-place` |
| STEP | stress | At the safepoints its schedule picks, stops the run and takes a burst of steps into, over and out, reading the stack at every stop. | `vm.heap.stop-runs-no-code`, `vm.handle.debugger-holds-no-bare-handle`, `vm.activation.debugger-reads-no-dead-register`, `vm.activation.rewound-push-ran-nothing`, `vm.compiler.patched-slot-runs-original-once`, `vm.machine.safepoint-request-bits` |
| dead-register poison | debug | A push hands out and a pop gives back dead registers holding a poison word no value can be, and an instruction's read of that word aborts. | `vm.activation.dead-registers-nilled`, `vm.activation.top-at-window-end`, `vm.activation.program-counter-before-observation`, `vm.activation.debugger-reads-no-dead-register`, `vm.compiler.register-effects-declared` |
| leftover check of both lowerings | debug | Both lowerings assert on every body they emit that no instruction reads a register the leftover analysis taints and that every register above a send's slots is dead. | `vm.activation.dead-registers-nilled`, `vm.compiler.argument-slots-top-live-registers`, `vm.compiler.register-effects-declared` |
| activation generation | debug | An activation handle and a register base carry the generation of the frame they were minted for, in its flags word; every access checks it, and checks the index against the top, the frame's window and its header. | `vm.handle.space-relative`, `vm.handle.activation-never-stored`, `vm.activation.register-in-window`, `vm.activation.unwind-target-is-frame` |
| origin guard and move epoch | debug | Every handle carries the heap that minted it and the move epoch it was minted in; storing, comparing or reading one of another heap or an older epoch aborts. | `vm.heap.collection-only-at-safepoint`, `vm.handle.nothing-carried-across-safepoint`, `vm.handle.space-relative`, `vm.handle.stored-only-when-current`, `vm.handle.only-where-rewritable`, `vm.handle.debugger-holds-no-bare-handle` |
| vacated nursery poison | debug | A scavenge fills the nursery space it vacated with poison, so a word left naming it reads poison where it is next used. | `vm.heap.barrier`, `vm.heap.no-forwarder-reached`, `vm.heap.weak-settled-before-compaction`, `vm.heap.weak-settled-before-nursery-vacated`, `vm.machine.allocation-as-interpreter` |
| host entry checks | debug | The host entry asserts that the run loop is not already running, that the activation stack is as deep as it found it, that no suspension row is left and that no nested class's creation mark is outstanding. | `vm.heap.no-native-frame-below-safepoint`, `vm.activation.run-balanced`, `vm.activation.no-suspension-outlives-run`, `vm.activation.abandoned-continuation-undone` |
| mark bit check | debug | Every marking begins by checking that no object carries the mark bit. | `vm.heap.mark-bit-clear-outside-collection` |
| forwarded survivor check | debug | Every handle a compaction forwards is checked to name a survivor. | `vm.heap.forwarding-table-exact` |
| block bound check | debug | Every object access through a handle checks the header -- refusing a forwarded or free format -- and that the index ends where the header's word count says. | `vm.heap.no-forwarder-reached`, `vm.heap.objects-never-grow`, `vm.heap.access-within-object` |
| old fill check | debug | The image decoder checks at every placeholder fill that the stored word names an old object. | `vm.heap.old-allocations-born-remembered`, `vm.image.nothing-collects-while-placeholders` |
| new object poison check | debug | No word of a newly allocated closure, frame or boxed float may still be the nursery's poison when it is first read. | `vm.heap.allocation-initialized-before-read` |
| precomputed guard check | debug | Each precomputed allocation header's guard is compared with the one a lookup of the object's class answers. | `vm.heap.precomputed-header-matches` |
| dispatch id recomputation | debug | Every read of an object's dispatch id recomputes it from the object's class and asserts that the header agrees. | `vm.dispatch.header-guard-stamped`, `vm.dispatch.reserved-guards`, `vm.image.code-runs-after-stamping` |
| level facts check | debug | Registering an application checks its level facts and application row against its mixin. | `vm.dispatch.level-facts-agree`, `vm.machine.application-id-table` |
| byte array alignment check | debug | Every reader of a byte array's data asserts the data begin at the block's 32-byte alignment. | `vm.heap.every-mover-realigns` |
| immediate integer range check | debug | Making an immediate integer asserts that the integer fits the small-integer range. | `vm.heap.number-one-representation` |
| metaclass check | debug | Attaching a metaclass asserts that a metaclass itself gets none. | `vm.heap.class-word-is-metaclass` |
| declaration code check | debug | Every lexical send asserts that its code's declaration-code mark agrees with the frame's captured chain. | `vm.dispatch.lexical-origin-from-code` |
| read-only local assertions | debug | The lowering of an assignment asserts that its target is not one of the read-only locals, and the reference lowering's shape check holds the oracle's form. | `vm.compiler.value-register`, `vm.compiler.read-only-locals`, `vm.compiler.simultaneous-temporary-writable` |
| code id cache check | debug | A hit of a body's code id cache asserts that it names the body's own row. | `vm.compiler.code-id-cache-current` |
| cell verification | debug | Every cell is compared with its entry, whole, after every compaction of the code zone and at every SCAVENGE scavenge. | `vm.dispatch.reserved-guards`, `vm.machine.cells-follow-entries`, `vm.machine.cell-addresses-fixed`, `vm.machine.compaction-remaps-every-holder` |
| machine entry row check | debug | A machine entry row whose bit is set must be current when recorded, and every row is checked after a compaction. | `vm.machine.compaction-remaps-every-holder`, `vm.machine.machine-entry-rows` |
| dispatching row check | debug | Every dispatching row is checked against the copies of its body after every compaction and slide. | `vm.machine.dispatching-rows` |
| dispatching word check | debug | Before rows are given back, every dispatching word is checked to name a block of an id the table holds. | `vm.machine.code-ids-kept-with-code` |
| twin pair check | debug | After every compaction and slide, every lean body and twin pair, every transfer word, and that no cell or row names a twin, are checked. | `vm.machine.reify-only-in-twin` |
| leaf view check | debug | As a leaf's view of the run's state is released, it is asserted that no leaf reified a frame running lean code. | `vm.machine.reify-only-in-twin`, `vm.machine.leaf-contract` |
| entry block check | debug | At every entry into machine code, every lasting word of the entry block is compared with what it names. | `vm.machine.entry-block-current` |
| optimized code replay | debug | Every optimized body's allocation, write-backs and interrupt-point metadata, and every poll's entry, are replayed and checked against the frame words the interpreter would hold. | `vm.machine.optimized-frame-is-interpreters`, `vm.machine.inlined-callee-ends-sender-bindings`, `vm.machine.back-into-optimized-code`, `vm.machine.scope-resumes-at-boundary` |
| held register check | debug | Every temporary register optimized code uses is checked against the registers holding values, and the SSA graph is verified. | `vm.machine.registers-held-between-frame-nodes` |
| eliminated block check | debug | A graph that reads an eliminated block from a frame node, return, operand or type-case is refused. | `vm.machine.eliminated-block-made-once` |
| scope order check | debug | The exit's walk asserts that each scope's current class is computed before its header is written. | `vm.machine.inlined-scope-current-class` |
| zone entry check | debug | A compaction of the code zone asserts that no entry into machine code is in progress. | `vm.machine.zone-compacted-at-safepoint` |
| kept evaluation check | debug | At a run's end it is asserted that no debugger evaluation kept with its raised depth limit outlives the run. | `vm.activation.evaluation-depth-bounded`, `vm.machine.safepoint-request-bits` |
| brand doctests | compiler | Three doctests that must fail to compile, on entering the interpreter: a handle cannot leave its entry, be stored in a host's field, or be used by a host's send from inside an entry. | `vm.heap.collection-only-at-safepoint`, `vm.heap.no-native-frame-below-safepoint`, `vm.handle.nothing-carried-across-safepoint`, `vm.handle.host-holds-only-roots` |
| two handle types | compiler | The handle of a running activation and the handle of a frame object are distinct types, so storing the first where the second is taken does not compile. | `vm.handle.activation-never-stored`, `vm.activation.unwind-target-is-frame` |
| exhaustive trace impls | compiler | Every implementation of tracing names every field and matches every variant without a wildcard, so a new field holding a handle must be decided. | `vm.heap.roots-complete` |
| exhaustive format match | compiler | The enumeration of an object's pointer words matches every format exhaustively, so a new format must declare its words. | `vm.heap.format-decides-pointer-words`, `vm.heap.non-pointer-words-untraced`, `vm.heap.byte-array-own-format`, `vm.heap.class-by-format` |
| exhaustive reach matches | compiler | The outer reach of an instruction and of its eight-byte form are exhaustive matches, so a new instruction must answer its depth. | `vm.activation.outer-reach`, `vm.compiler.reach-derived-never-read` |
| exhaustive effect matches | compiler | An instruction's reads, write, jump target, fall-through, successors, argument slots and delivered message are exhaustive matches. | `vm.compiler.register-effects-declared` |
| non-zero handle type | compiler | A handle's offset is a non-zero integer type, so no handle can be 0. | `vm.handle.never-zero` |
| private value word | compiler | A value's word is private to its module and its kinds are matched exhaustively. | `vm.heap.value-tags` |
| compiles-alike destructuring | compiler | The comparison of a body compiled again destructures every field of the body, so a new field must be compared. | `vm.compiler.debug-tables-describe-their-body` |
| shipped codec exhaustive | compiler | The embedded code's encoder destructures every type and matches every enumeration without a wildcard. | `vm.compiler.reach-derived-never-read` |
| breakpoint only a compact form | compiler | A breakpoint is a form of the eight-byte slot only, never an instruction, so the exhaustive matches over instructions never see one. | `vm.compiler.slot-readers-see-original` |
| future leaf match | compiler | Whether a leaf declines for a future is a match over every leaf by name. | `vm.dispatch.future-seen-through` |
| borrowed slices | compiler | The object space and every reservation are lent as bounds-checked, borrow-checked slices, none outliving a change of what is accessible. | `vm.heap.access-within-object` |
```
