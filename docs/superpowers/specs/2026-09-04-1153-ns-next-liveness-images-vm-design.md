# ns-next — Liveness, Images, and VM Architecture Constraints

Status: active — binding constraint for all milestones
Date: 2026-09-04
Author: Benjamin

## Summary

This document is a **cross-cutting constraints spec**. It does not describe a
single milestone; it states the properties the `ns-next` runtime and VM must
preserve across *every* milestone, from the Milestone 1 tree-walking
interpreter to a future optimizing JIT. It exists because these properties are
cheap to design for early and expensive — sometimes impossible — to retrofit.

`ns-next` is a native Rust runtime for Newspeak (Gilad Bracha's Smalltalk
descendant). The Milestone 1 design is in
`2026-09-01-2252-ns-next-runtime-design.md`; the frontend (lexer, AST, parser, CLI
`parse` / `parse-corpus`) is implemented. This spec constrains the runtime crate
(`ns-runtime`) that comes next, and the VM milestones after it.

## Requirements

1. **Maximum liveness.** Classes, methods, activations, the compiler, and the
   module loader are all reified and manipulable at runtime. There is no hard
   compile-time / run-time boundary: loading a module is evaluating it in the
   live system.

2. **Edit-and-continue debugging.** The debugger changes existing code at
   runtime with no restart. In particular: a method that has local
   slots/temporaries can be edited *while an activation of it is live on the
   stack* — including adding new local slots — and that same activation
   continues in the new version of the method.

3. **Mirror-based reflection with a MirrorBuilder.** Reflection is
   capability-style mirrors onto classes, objects, and activations. A
   MirrorBuilder mutates classes at runtime — add/remove instance slots and
   methods, change superclasses — while instances of those classes are already
   live in the heap.

4. **Portable, serializable images.** A snapshot of the entire live world
   (object graph + every live activation + VM-level state) must resume in the
   exact same logical state on a *different CPU architecture*. The image is
   architecture-neutral; resumption is bit-exact at the level of observable
   program state.

5. **Images are optional; source is first-class.** It must be fully possible to
   work from source code with no image at all — unlike classic Smalltalk, which
   had no source files and trapped all state in the image. The system supports
   *both* source code and serializable images, and neither is second-class.

6. **Performance must eventually approach Java.** Not in Milestone 1, but the
   architecture must not foreclose it.

7. **Real concurrency, not a single global event loop.** The system must
   eventually run genuinely in parallel across cores. Raw shared-memory OS
   threads are explicitly not the goal — see "Concurrency" below for what is.

## The unifying principle

**The bytecode / interpreter level is the single source of truth for program
state.** Any JIT is strictly a runtime accelerator that must always be able to
reconstruct interpreter-level frames — i.e. it must support deoptimization.

This one decision is what makes requirements 2, 3, 4, and 6 compatible instead
of contradictory. Concretely, there is one mechanism — **an abstract-state map
recorded at every safepoint** (bytecode index, virtual temp values, receiver,
argument values, sender link) — and it has four consumers:

| Consumer | Uses the abstract-state map to… |
|---|---|
| Debugger | reconstruct an inspectable/editable view of a suspended activation |
| JIT deopt | rebuild an interpreter frame when a speculation is invalidated |
| Image snapshot | serialize a running activation architecture-neutrally |
| MirrorBuilder | (via the mirror API) reach activation state during class mutation |

The debugger, the MirrorBuilder, and image snapshot are all clients of **one
reflective core** over the reified interpreter model.

- **Snapshot** = deoptimize everything to the interpreter level, then serialize
  the logical object graph (including reified activations).
- **Load** = deserialize, begin interpreting, re-JIT hot code as it warms
  again.
- **Debugger entry on a frame** forces that frame (and usually its callers) to
  deoptimize to the interpreter first. Debugging and JIT-optimizing the *same*
  frame simultaneously is explicitly not a goal.

## Reified activations

Newspeak-level activation state must never live only in Rust locals or the Rust
call stack. Each activation is an explicit, heap-reachable structure:

```
Frame {
    receiver:  Value,
    args:      Vec<Value>,       // or folded into temps
    temps:     Vec<Value>,       // indexable AND growable
    pc:        Pc,               // explicit; statement position recoverable
    sender:    Option<FrameRef>, // the reified call chain
    home:      Option<FrameRef>, // for a block: its method activation, for non-local return
    method:    MethodRef,        // the *current* method object, re-resolved, never inlined permanently
}
```

- **`temps` is a heap `Vec<Value>` addressed by index and able to grow.** Adding
  a local slot to a running method means growing the active frame's `temps`,
  initializing the new entries to `nil`, and remapping the PC.
- The M1 tree-walker may use Rust recursion for control flow, but the Frame
  chain above must exist and be walkable independently of the Rust stack.
- From M2 on, frames may run on a native stack for speed (see Cog's
  "married/divorced contexts") but must be *reifiable on demand* into the
  structure above whenever reflection, debugging, or snapshot touches them.

## Object model

- `Object` header points to its class; the **class owns the slot layout**. Slot
  access is resolved through the *current* layout, so a layout change plus
  instance migration is expressible.
- **Stable object identity and an identity hash from the start** — required for
  `IdentityDictionary`/`IdentitySet`, for image roots, and for `become:`.
- **`become:`-style identity swap / object forwarding** must be supported.
  Adding an instance variable to a class with live instances allocates
  new-shape instances, copies fields, and atomically swaps identity. This is a
  real constraint on GC choice:
  - MMTk supports object forwarding.
  - A copying/compacting collector can implement it.
  - A conservative non-moving collector makes `become:` hard — avoid.
- `Value` is `Nil | Bool | SmallInt(i64) | Float(f64) | Obj(ObjRef)` for M1
  (`Rc<RefCell<_>>`, cycles leak — an accepted M1 limitation). A later milestone
  replaces this with a tagged machine-word representation plus a real GC; the
  image format (below) is defined against the *logical* value model, not this
  representation, so that swap does not break images.

### The tree-walker and a moving collector (decided 2026-09-06)

Milestone 4 replaces `Rc<RefCell<_>>` with an arena and a real collector, and
that collector moves objects — `become:`, forwarding and instance migration are
requirements above, which is what rules the conservative non-moving option out.
A moving collector must not only *find* every root but *update* it, and the two
engines are in very different positions for that.

The VM keeps every intermediate in the frame: `run_code` writes each result
through `write_local` into `frame.temps`, and frames are heap objects chained by
`sender`/`home`. A precise collector reaches every root of a running activation
by walking that chain — which is what the reified frame chain exists for.

The tree-walker keeps intermediates in Rust locals. `eval_expr` returns a
`Value` up the Rust stack, a dynamic array literal collects into a `Vec<Value>`
there, and `eval_send` holds the evaluated receiver in a Rust variable across
`eval_args` — which allocates — and then holds receiver and arguments across
the send itself. Under `Rc` those references keep their objects alive and never
need updating. Under a moving collector they are roots it cannot see and
addresses it cannot rewrite.

Keeping the tree-walker past that point is possible, but not free: it needs a
handle layer — a shadow stack in the shape of V8's `HandleScope` or JNI local
references — registering every intermediate in a heap-side list the collector
scans and fixes up. The mechanism is well understood; the cost is that the
discipline has to hold across all of `interpreter/eval.rs` and
`interpreter/instantiate.rs` and be re-established on every later change, and
that a missed handle is heap corruption that shows up only under collection
pressure. The rest of the object-model swap is not the problem: arena and
handles hit both engines the same way, and the shared orchestration in
`interpreter/` is ported once.

**Decision (2026-09-06): the tree-walker is kept, and it gets that handle
layer.** The extra engineering was weighed and accepted deliberately. Two
independent engines that must agree byte for byte are a verification property
worth a permanent cost, and V8 is the existence proof that an interpreter can
hold its roots in handles next to a moving collector without the design coming
apart. Milestone 4 therefore does not choose between the collector and the
tree-walker; it implements root discipline for both. The reified frame chain is
the root set for the VM, a handle layer is the root set for the tree-walker's
Rust-stack intermediates, and the collector scans and updates both.

What this does *not* extend to is the image. The tree-walker executes the AST,
so it runs only when execution starts from `.ns` source, where the AST is
present by construction; a loaded image runs bytecode. That asymmetry is the
boundary of what an AST engine can do, not a step towards retiring it.

## Dispatch and caches

- Method dictionaries are mutable at runtime. Dispatch resolves selector →
  method through the dictionary (or through a cache that is invalidated when the
  dictionary changes). **Never bake a method-body pointer into anything
  durable.**
- Every cache — inline caches, global method-lookup caches, and later JIT
  speculative assumptions — carries a **dependency key** (typically class +
  selector). A global "class shape changed" / "method changed" / "superclass
  changed" event flushes all caches keyed to the affected class, and for a
  superclass change, to its whole subtree.
- Inline caches attach to **call sites**. Every send in the resolved tree / the
  bytecode has a stable call-site identity so caches and type feedback can be
  attached and, on snapshot, discarded.

## Debugger concept

1. **Suspend.** An uncaught Newspeak error or a breakpoint reifies the
   activation chain as a chain of context mirrors (using the reified Frame
   structure above).
2. **Inspect.** Read receiver, arguments, temps, and the sender chain through
   mirrors.
3. **Edit and continue.**
   - Recompile the edited method (new bytecode, new temp count, new
     statement→PC table).
   - For every affected context — the active one and any pending frames running
     the same method — grow `temps` if slots were added, set new temps to
     `nil`, and remap the PC via the statement→PC table.
   - **Resume only at statement boundaries.** This keeps PC remapping tractable:
     the debugger restarts the current statement (or the whole method, with
     arguments and receiver preserved) rather than resuming mid-expression.
   - Continue execution — interpreted; the method re-JITs later if hot.
4. **Other operations** on a context mirror: restart frame, force return value,
   step (into/over/out).
5. **Breakpoints.** Either bytecode patching (replace an op with a trap op) or a
   per-method "debug mode" flag that forces interpretation with a per-statement
   callback.

## Mirrors and MirrorBuilder

- A **mirror** is a read/write handle onto a class, an object, or an activation,
  operating through the same reified model (slot layouts, method dictionaries,
  Frame structures) that the debugger uses.
- A **MirrorBuilder** applies class changes:
  - *Add/remove instance slot* → new slot layout on the class → migrate every
    live instance (`become:`) → flush caches keyed to the class.
  - *Add/remove/replace method* → update the method dictionary → flush caches
    keyed to (class, selector).
  - *Change superclass* → re-resolve method lookup for the subtree, flush its
    caches, and re-layout instances if inherited slots changed.
- The debugger's "edit and continue" is a special case: a mirror on the
  suspended activation plus a method replacement plus the ability to resume.

## Image format

**Not** the classic "image is a snapshot of VM memory / `mmap` the file"
approach — that cannot be cross-architecture. The image is an **explicit,
self-describing, architecture-neutral serialization format that is decoded on
load**, never a memory dump.

- No native code, no JIT-compiled machine code, no raw pointers, no baked-in
  pointer tagging or host endianness in the image.
- **Bytecode is architecture-neutral and versioned**; it is part of the image.
- Canonical encodings: little-endian integers, IEEE-754 `binary64` for floats,
  UTF-8 for strings (interned on load), a fixed `SmallInt` value range (`i64`
  per the M1 spec).
- Preserve identity hashes. Preserve stable identities for well-known objects
  (`nil`, `true`, `false`, the base classes, the class table) via a
  root / special-objects array at a fixed position in the image.
- Serialize **reified activations** (the Frame chain, at bytecode level) so a
  running world — including an open debugger session — round-trips.
- Version the format; refuse or migrate on a version mismatch.
- **Bootstrap the initial image from source** — build the base classes and
  well-known objects from `.ns` plus Rust primitives (as modern Pharo does),
  rather than hand-growing an image that then becomes irreproducible.

## Source and images together

Newspeak modules are already files upstream (unlike Squeak), so `ns-next` is
well positioned for a file-first model.

**Source files are the system of record for code. Images are snapshots of
running state** (heap + activations + whatever code has been loaded or modified
live).

- Every class and method retains its source text and a **provenance record**:
  where the source came from (file + span) or "entered in the debugger at time
  T".
- An image contains source too and can always be **exported back to `.ns`
  files**.
- "Changed live but not yet written back to a file" is a **visible, first-class
  state** — surfaced by tooling, not hidden. This is the specific failure of
  classic Smalltalk that this design rejects.
- A session may be configured to **auto-write live source changes back to
  files** (Tonel/Git-style), so the file tree stays authoritative.
- Working with no image at all is the normal case: `ns-next run` / a live
  session that loads modules from files and never takes a snapshot.

### Deployment artifacts without source (open, recorded 2026-09-06)

The bullets above say an image *contains* source. Newspeak upstream does not
work that way, and the difference is worth stating before Milestone 3
settles it deliberately.

Bracha's position is **two artifacts, not one**: "Ideally, you have images and a
binary format", where "serialized top level classes can serve as a binary
format" ([An Image
Problem](https://gbracha.blogspot.com/2009/10/image-problem.html)). His
complaint about Smalltalk is not that an image exists but that it is the *only*
artifact — "it becomes very difficult to disentangle one's program from the
state of the ongoing computation".

Deployment upstream is therefore a serialized object graph, not a source tree:
"A serialized instance of BraveNewWorldExplorerApp acts as our binary. The
Newspeak runtime loads such a serialized object, deserializes it, and invokes
its main:args: method" ([Serialization
Killer](https://gbracha.blogspot.com/2010/02/serialization-killer.html)). It
works because "top level classes, also known as module declarations, are
stateless. Hence the serialized form of these class objects is stateless as
well, fulfilling a key requirement for a binary code representation", and
because serializing an object carries its behaviour with it — classes reference
mixins, mixins reference methods, and all of them are objects. In the current
toolchain the application class is saved as a `.ns` file and built into a
`.vfuel` binary that is served alongside the Primordial Soup VM
([newspeak](https://github.com/newspeaklanguage/newspeak)).

A deployed `.vfuel` **does carry full source text**, checked directly rather
than assumed. Four artifacts from `newspeaklanguage.org/samples/` were
inspected, and long text runs are a near-constant share of each:
`CounterApp.vfuel` 1.27 MB (44%), `TodoMVCApp.vfuel` 1.28 MB (44%),
`Particles.vfuel` 1.26 MB (44%), `HopscotchWebIDE.vfuel` 3.00 MB (46%). The
IDE was checked first and is the misleading sample — it browses source for a
living — so the three ordinary applications are the ones that settle it:
nothing is stripped at packaging time.

What they contain is not only the application's own code. `CounterApp` — a
counter with a button — holds 152 distinct class definitions with their source
and doc comments, `AST2ByteCodeCompiler` and `ASTTraverser` among them: a
deployed counter ships the compiler. That follows from the mechanism rather
than from an oversight, since serialization starts at the application object
and everything is reachable from the platform it holds; there is no step at
which anything is judged unnecessary at run time. It is also why roughly
1.27 MB is the floor for any deployed app.

So upstream's second artifact is
*not* a source-free one; it is a serialized object graph that happens to
include the source strings hanging off its method and class objects, next to
the bytecode those same objects carry (the serialized image contains the source
of the VM's own method representation: "public bytecode <ByteArray> (* Must be
slot 3, known to the VM. *)").

That makes the source-free variant an open design question for `ns-next` rather
than something to copy. The mechanism that would make it possible is already in
Newspeak's architecture: reflection is mirror-based, so source access is a
*capability* rather than a property of every object (Bracha & Ungar, "Mirrors:
Design Principles for Meta-level Facilities", OOPSLA 2004 —
`docs/references/mirrors.pdf`). Whether upstream
offers a build that exercises that separation was not established here.

### Direction: a source-free image is the target (decided 2026-09-06)

Unlike upstream, `ns-next` will have a source-free image, and **a decompiler is
the preferred way to make such an image readable again** — not an embedded copy
of the text. That inverts the bullet above: the requirement to keep is "an
image can be **exported back** to `.ns`", and a decompiler satisfies it from
bytecode. No spec here describes one yet, so it is new work, and it is what
makes this direction honest rather than a quiet loss of capability.

Three things make this cheaper here than it would be upstream, all checked
against the Milestone 2 code:

- **Nothing in the bytecode engine looks a binding up by span.** Of the eight
  `bindings.get(&span)` call sites, five are in `compiler/lower.rs` and run at
  compile time; the other three are in `interpreter/eval.rs`, i.e. in the
  tree-walker. Compiled code has its registers, hop counts and selectors baked
  in. So `HashMap<Span, Binding>` — the structure that makes the source text
  load-bearing today — is simply not needed by a bytecode-only image.
- **Every selector is already in the image.** Dispatch is virtual and by name,
  so a selector cannot exist only in the AST — the run-time lookup would not
  find it. `image/encode.rs` already writes the selector of every send
  instruction, the key of every method-dictionary entry, and every
  lexical-send path segment as a string.
- **Very little of a class is read at run time.** `instantiate.rs` needs the
  factory pattern (selector and arity), the slot count, and whether the class
  has initializer code at all; `instance_slots`/`slot_index`/`init_exprs` are
  otherwise read on the tree-walk path.

**The delta is a class table.** Class name, slot names with their indices,
factory selector and arity — the replacement for "reconstruct the `ClassRef`
graph by re-running the front end". Everything else a source-free image needs
is already in the Milestone 2 format: the object graph with identity hashes,
the frames, the code table (`num_args`, `num_registers`, `statement_pc`, call
sites, a `source: Span` per body) and the root list, which already carries
every method's selector.

Four changes carry that delta, and they belong in a single format break:

- **The class table itself**, as above.
- **The class tag has to index it.** Today a tag is either a builtin kind or
  `UserRoot` (`image/encode.rs`, `encode_class_tag`), because `load_module`
  loads no nested classes and there is exactly one user class to name. Its own
  comment already records the consequence — a second user-defined class would
  encode as `UserRoot` and decode as the root, a wrong class rather than a
  decode error — and that the milestone which loads nested classes must
  replace tag 1 with an index into a table of declarations. That is
  Milestone 3.
- **`load` stops re-parsing.** It currently reads the stored source and re-runs
  parse → resolve → bootstrap → `load_module` to rebuild the `ClassRef` graph
  and the bindings map (`image/mod.rs`); it decodes the class table instead,
  and `LoadedImage.bindings` goes away with the tree-walk path.
- **The class table is untrusted input like everything else.** Every class
  index, slot count and arity is validated before it indexes or allocates, and
  `verify_graph` resolves each object and frame against the table rather than
  against a re-parsed module.

**A symbol table belongs with it.** Selectors are the most repeated strings in
the format — name-based virtual dispatch puts the same selector at very many
call sites — and each occurrence is written inline today. A table at the head
of the image, referenced by index, deduplicates them and turns interning at
load time into one pass over that table instead of one `Symbol::intern` per
send instruction.

**Milestone 3 refines this delta and carries it out**
(`2026-09-06-1659-ns-next-nesting-mixins-source-free-images-design.md`). Two
things there differ from the sketch above. The table is a *mixin* table, one
row per class declaration, because the language spec requires a distinct class
object per enclosing instance and only the mixin is shared between them; and
`FORMAT_VERSION` is not bumped, because the project has no external users and
images that predate a change are disposable. The untrusted-input guarantee is
unaffected: it comes from validating every id, tag, count and length before
use, never from the version field.

**Reflection survives this.** Object mirrors (class of, slots by name), class
mirrors (name, slots, selectors, arity, instantiation) and activation mirrors
over the reified frames all work from the class table plus what is already
serialized, and `statement_pc` still maps a `pc` to a statement. The retained
`Span`s keep **provenance** — file and range — so a debugger can point at the
original source when it happens to be available, without the image carrying it.

**Two tiers, and reflection works in both:**

1. **Full** — source and AST, as today. Development.
2. **Reflective, source-free** — class table, code, spans as provenance. The
   deployment target: reflection and debugging keep working, readability comes
   from the decompiler.

**A debug variant may still embed the source** as optional metadata for tier 2,
for the case where a debugger should show original text without shipping the
files. That is a packaging switch, not a second format.

**There is deliberately no "minimal" tier that strips reflective metadata.** An
earlier draft proposed one — drop `statement_pc`, spans and slot names for a
smaller artifact. It is the wrong axis. Code cannot know whether someone is
holding a mirror on it, so removing the metadata does not remove the
capability; it leaves a granted capability answering with nothing. In a
capability language the sanctioned way to prevent inspection is for the
platform to **withhold the mirror**, which is a decision at the boundary where
capabilities are handed out, not a build flag that silently changes what the
program means.

Two consequences make this concrete rather than principled:

- The image mechanism is itself reflective. A deployed application that
  snapshots its own running state — which this document asks for — has to walk
  objects and their classes. An artifact stripped of class shape cannot take a
  snapshot, so a "minimal" tier would break the very feature the tier system
  exists to serve.
- Ecosystems that made metadata removal a build-time switch (Java with
  ProGuard, .NET trimming) all had to invent keep-annotations so that programs
  could tell the tool what not to delete. That is accidental complexity bought
  by putting the switch in the wrong place.

If binary size ever forces the question, decide it then against a measurement:
the class table and statement map are small next to the bytecode and the object
graph. And if the goal is ever to make a deployed program hard to read, that is
consistent *renaming*, not removal — mirrors keep working and report the
renamed identifiers.

**The tree-walker is not an obstacle to this.** It executes the AST and so
cannot run from a source-free image — but it never has to. It is the engine for
running `.ns` source directly, where the AST exists by construction, and images
run bytecode. The tree-walker is retained permanently (see "The tree-walker and
a moving collector"), so the wording elsewhere in these specs that calls it
"permanently maintained" is accurate as written.

A side benefit: a source-free artifact removes the front end from the load
path, and with it the re-parsing of attacker-controlled text that the
Milestone 2 image format has to defend against today.

## Concurrency: actors, not threads

Newspeak already has an answer to concurrency, and it is not shared-memory
threading. It is visible directly in the vendored corpus
(`corpus/modules/Actors.ns`, `ActorsForJS.ns`) and partially in the grammar
already: the `<-` eventual-send operator (`<-:` in the token stream; parsed
and currently erased for Milestone 1) is the syntactic surface of an
actor/vat model with asynchronous message sends and promises, in the lineage
of the E language (Mark Miller) that strongly influenced Newspeak's design.

**The language-level model, as the corpus shows it:**

- An actor never holds a direct reference to another actor's objects — only a
  `FarReference`. There is no shared mutable state across actors.
- Cross-actor sends go through `eventualSend:arguments:`, which enqueues a
  `PendingDelivery` in the target actor's mailbox and immediately returns a
  promise (`InternalLocalPromise` / `InternalRemotePromise`).
- The promise is resolved later through a `Resolver` — "the authority to
  resolve a Promise and the control facet of its eventual resolution."
- Each actor runs its own sequential mailbox-processing loop
  (`internalActor startEventLoop` in the reference implementation).

**This eliminates data races by construction, with no locks in the language.**
Code inside one actor is always sequential and never has to reason about
concurrent mutation of its own state.

**Why this is not "a JS event loop," if built correctly on ns-next:** the
reference implementations (Squeak backend, JS backend) multiplex every actor
onto a *single* host event loop only because their hosts — one Squeak VM, one
browser thread — are themselves single-threaded. That is a constraint of those
hosts, not of the language semantics: nothing in the actor model requires a
single shared loop, only that actors not alias each other's mutable state.

`ns-next` is a fresh native VM without that constraint, so it should give
actors real parallelism:

- **Per actor:** one logical event loop (drain the mailbox, process, repeat).
  User code inside an actor stays simple — no locks, no threads to reason
  about.
- **Across actors:** an **M:N scheduler** multiplexing many lightweight actors
  onto a real OS thread pool sized to the core count — the same shape as Java
  virtual threads (Project Loom) multiplexing onto carrier threads, Erlang/BEAM's
  process scheduler, or Go's goroutines onto `GOMAXPROCS` threads. Because
  actors share no mutable state, running many of them truly in parallel across
  cores is safe by construction.

The result is many small, genuinely parallel event loops instead of one global
one — not a JS-style single event loop, and not raw shared-memory threading
either.

**How this interacts with the rest of this spec:**

- **`become:` / live class mutation:** instance data is actor-isolated, which
  narrows the blast radius of an instance migration. Classes and code remain
  shared (Newspeak has no global object namespace, but code is not the same as
  object identity) and continue to go through the dependency-keyed cache
  invalidation described above.
- **GC:** an actor is a natural per-heap GC scope. Stop-the-world pauses can
  stay local to one actor instead of the whole world — directly serves the
  near-Java performance goal, the same way per-process heaps keep BEAM's GC
  pauses small.
- **Images:** an actor/vat is the natural snapshot unit. A whole-world snapshot
  is the union of every actor's state plus its mailbox and any in-flight
  promises; a partial/single-actor snapshot is also meaningful. This resolves
  the image-granularity question in "Open questions" toward "per-actor is the
  primitive; whole-world is a composition of per-actor snapshots," though the
  exact treatment of in-flight cross-actor promises at snapshot time still
  needs design work.
- **Debugger:** suspend/inspect/edit-and-continue naturally scopes to one
  actor's reified Frame chain, consistent with "Reified activations" above.
  Whether a suspended actor's mailbox keeps draining into other, unaffected
  actors while it is paused is an open question (see below) — the answer
  should be yes for actors other than the one being debugged, to keep the
  "not a single global loop" property under debugging too.

**Explicit non-goal:** raw OS threads sharing one mutable object heap are not
the default concurrency model. Classic Smalltalk's single-OS-thread green
scheduler (see below) worked around exactly the hazard this design solves
properly: a highly mutable, live-editable, globally reachable object graph is
treacherous under shared-memory concurrency. If narrowly-scoped data
parallelism over provably non-aliased or frozen data (e.g. a `parallelDo:`
primitive over an immutable collection) is ever added, it is a deliberate,
narrow exception — not the default model, and not exposed as general-purpose
shared-memory threads with locks.

**What other Smalltalk implementations do, for contrast:** Squeak/Pharo's
`Process` / `Semaphore` / `Delay` / `ProcessorScheduler` are green threads —
cooperative plus priority-preemptive — multiplexed onto a *single* OS thread.
There is no mainstream real parallel execution of Smalltalk code across cores;
the same globally-shared, live-mutable object graph that motivates this
document's liveness requirements is exactly what makes that hard to add safely
after the fact. Newspeak's actor isolation is the principled fix, not a
workaround.

## Performance and the tiered execution model

Near-Java performance for a fully-live dynamic language comes from **inline-cache
feedback + speculative optimization + deoptimization** (the V8 / HotSpot path),
not from whole-program ahead-of-time assumptions — which liveness forbids
anyway.

- **Tier 0 — interpreter.** Always correct, always the deopt and snapshot
  target. Bytecode from M2.
- **Tier 1 — baseline JIT.** One code template per bytecode op. Candidate
  backends to evaluate: copy-and-patch stencils (simple, ~baseline quality) or
  Cranelift (more work, more headroom).
- **Tier 2 — optimizing JIT.** Speculative inlining driven by IC feedback,
  escape analysis, unboxing; guarded by deopt.
- **Deopt metadata** (safepoint ↔ machine-code position ↔ abstract interpreter
  state) is carried from M2 onward.
- A **snapshot discards all JIT state**; warmup re-happens after load. This is
  acceptable — snapshotting is rare.
- The GC (generational, compacting, `become:`-capable; MMTk a strong candidate)
  is a pure runtime concern. **The image serializes the logical object graph,
  not the heap layout**, so GC changes do not affect image compatibility.

## What to build in, and when

| Milestone | Constraint work |
|---|---|
| **M1** — tree-walking interpreter | Reified Frame chain, walkable independently of the Rust stack; growable indexed `temps`. Dispatch through mutable method dictionaries; no durable method-body pointers. Class-owned slot layout; slot access via current layout. Stable identity + identity hash. Retain source + provenance per class/method. |
| **M2** — bytecode + interpreter VM | The safepoint abstract-state map (debugger + deopt + image). Statement→PC table per method. Architecture-neutral, versioned bytecode. Define and version the image serialization format; bootstrap from source. Stable call-site identity. |
| **M3** — nested classes + source-free images | Mixins shared per declaration; a mixin application cached per (mixin, superclass application) and carrying layout and lookup; a distinct class object per enclosing instance, holding its enclosing object. Superclass clause evaluated, one initializer activation per level. Mixin table and symbol table in the image; the source leaves the load path. |
| **M4** — inline caches + GC | Dependency-keyed, globally-flushable caches, keyed on the mixin application rather than the class object. `become:` / forwarding support in the GC and object model; instance migration. Precise root tracking for both engines: the reified frame chain is the VM's root set, and the tree-walker gets a `HandleScope`-style handle layer for the intermediates it holds on the Rust stack (see "The tree-walker and a moving collector"). |
| **M5–M6** — JIT | Deopt metadata from every safepoint. Tiered promotion driven by IC feedback. Snapshot tears down JIT state cleanly. |

## Developer tooling for the VM itself

Distinct from the Newspeak-level debugger (which serves *programs running on*
ns-next), the VM implementation needs its own introspection tooling — the
`d8`-equivalent developers use to see what the runtime itself is doing:
bytecode disassembly, inline-cache transition traces, deoptimization reasons,
generated-code/IR dumps, heap and GC statistics, and a REPL to poke at a live
image.

This is cheap here specifically because of the unifying principle above: the
mirror API and the per-safepoint abstract-state map already expose everything
such tooling needs. A `d8`-style shell is, in the overwhelming majority of its
commands, a thin CLI over the **same reflective core** the debugger, the
MirrorBuilder, and image snapshot already use — not a separate introspection
path. Treat "can the VM-developer tooling be built entirely on mirrors, with no
private backdoor into runtime internals" as a running completeness check on the
mirror API: a tool that needs to bypass mirrors to see something is evidence
the reflective API is missing a capability, not evidence that tooling needs
special access.

**Where it lives:** as subcommands/flags on the existing `ns-next` binary, not
a separate binary — `ns-next` has exactly one embedding today, unlike V8, which
splits `d8` out because V8 embeds into many hosts. Revisit only if tooling later
needs privileged access (e.g. raw memory introspection) that the production
binary should not ship with; that would be a deliberate exception, not the
default.

**Grows incrementally with the milestones already listed above:**

| Milestone | Tooling added |
|---|---|
| M1 | `ns-next repl` — evaluate Newspeak expressions against a running interpreter. (`ns-next parse --ast` already exists from the frontend.) |
| M2 | `ns-next disasm <file> <selector>` — bytecode disassembly. Close to free: the format is already architecture-neutral and versioned for the image. |
| M3 | A `run-corpus` gate over a curated whitelist of real corpus modules, run under both engines; a `--bench` mode with warm-up-aware counters, giving M4 a before-number. |
| M4 | Inline-cache transition tracing (uninitialized → monomorphic → polymorphic → megamorphic) per call site; heap/GC statistics (pause times, promotion rates per generation). |
| M5–M6 | Deoptimization tracing (reason + site, read directly off the abstract-state map); generated-code / IR dumps per tier. |

The `--bench` mode in the M3 row doubles as the harness for the
tree-walker-vs-bytecode microbenchmark already recommended for the M1→M2
transition. It sits in M3 rather than later on purpose: M4 is the inline-cache
milestone, and an optimization without a before-number is not a measurable one.

## Tensions and non-goals

- **Debugging and JIT-optimizing the same frame at once** is not supported;
  debugger entry deoptimizes the frame.
- **Whole-program AOT optimization** is out — it is incompatible with runtime
  class mutation.
- **OS-level process snapshotting (CRIU and similar)** is *not* the image
  mechanism: it is architecture- and build-specific. Images operate at the
  managed-heap / bytecode level only.
- **Milestone 1** still uses `Rc<RefCell<_>>` (cycles leak) and `i64` integers
  (overflow raises) per the M1 spec. This document does not change that; it only
  requires that the M1 object model expose stable identity and a revisable slot
  layout so the later swap to a real GC and a tagged representation does not
  break images or reflection.

## Prior art

The Newspeak papers this design argues from are vendored under
`docs/references/`, with their venues, source URLs and copyright status in
`docs/references/PROVENANCE.md`. The two that carry the most weight here are
`dyla-final.pdf` (method lookup and scope with inheritance and nesting — the
Milestone 1 dispatch algorithms) and `mirrors.pdf` (reflection as a capability
— the reason a deployed image stays reflective).

- **Cog** (Pharo/Squeak JIT) — "married/divorced contexts": frames run on a
  native stack but are lazily reified into heap context objects when reflection
  or debugging touches them.
- **Pharo Spur** — portable image format and bootstrap-from-source;
  **Fuel / Ston** — object serialization shape.
- **Newspeak upstream itself** — deployment as a serialized application object
  (`.ns` source builds a `.vfuel` binary; the runtime deserializes it and sends
  `main:args:`), resting on module declarations being stateless. See
  "Deployment artifacts without source" above; this is the closest prior art
  for a second, source-free artifact alongside the live image.
- **TruffleSqueak / GraalSqueak** — full Smalltalk debugging together with
  near-JVM performance via Graal partial evaluation and deopt: existence proof
  that requirements 2 and 6 combine.
- **V8's `d8`** and HotSpot's diagnostic flags (`-XX:+PrintCompilation`,
  `-XX:+PrintInlining`, JITWatch, JFR) — the shape of VM-developer tooling to
  match, built here on top of the mirror API instead of a separate
  introspection path.
- **The E language** (Mark Miller) — origin of eventual send and the vat model
  that Newspeak's actor library already follows in the vendored corpus.
  **Erlang/BEAM**'s M:N process scheduler and per-process heaps, and **Java
  virtual threads** (Project Loom) — the carrier-thread multiplexing shape to
  match for running many actors in real parallel across a small OS thread
  pool.

## Open questions

- Register vs. stack bytecode for Newspeak's uniformly message-based semantics.
- Whether M1 should already emit a trivial bytecode (to exercise the
  abstract-state-map design) rather than pure AST walking.
- Image granularity: "Concurrency" above leans toward per-actor snapshots as
  the primitive, composed into a whole-world image — but the exact treatment
  of in-flight cross-actor promises and mailboxes at snapshot time still needs
  design work.
- Identity-hash width and its interaction with a future tagged-pointer
  representation.
- The source-free image is decided (see "Direction for `ns-next`" under "Source
  and images together"); what is open is the **decompiler** that has to provide
  "export back to `.ns`" in its place — when it is built, how faithful its
  output must be, and whether tier 2 keeps enough metadata (statement ranges,
  slot and argument names) for it to produce something a person wants to read.
- Whether a suspended (debugged) actor's mailbox keeps draining, and how far
  the M:N actor scheduler should be visible to or configurable by user code
  (e.g. actor placement/affinity) versus staying fully opaque.
- How much of the actor/vat runtime (mailboxes, promises, far references, the
  M:N scheduler) belongs in Rust core versus how much can stay Newspeak code
  over Rust primitives, mirroring how `Actors.ns` layers over a
  platform-provided `ProcessActor` in the reference implementation.
