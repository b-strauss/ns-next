# ns-next — Design

`ns-next` is a native Rust runtime for [Newspeak](https://newspeaklanguage.org/):
lexer, parser, resolver, bytecode compiler and register VM, and a versioned image
format. Rust 1.98.0, edition 2021.

This folder is the **living description of how the system is designed and how much
of it exists**. It is updated in place. There is no series of dated design
documents to reconcile: when a design changes, the document covering that aspect
changes with it, and git carries the history.

## How to read these documents

Three conventions make a document's claims checkable rather than merely
plausible.

**Present tense means it exists.** If a document says the application cache is
keyed by the superclass's application, that is what the code does today. Nothing
is described in the present tense as a way of describing an intention.

**Unbuilt work is marked, not paraphrased.** A section describing something that
does not exist yet is headed `Planned: …` and says in its first line what would
change. Something built but incomplete opens with **Partial:** and names what is
missing. Planned work is described in the document whose mechanism it changes,
because that is where a reader needs it; [`docs/OPEN-WORK.md`](../OPEN-WORK.md)
is the index of all of it, every open item in one list with a pointer to where
it is explained.

**Every document names its code.** Each opens with a status block listing the
modules it describes, so a claim can be checked by opening a file. A document
that cannot name its code is describing an intention, and should say so.

## Principles

Four rules decide between design options before any aspect's own reasoning
does.

**The specification is a requirement.** Behaviour the specification requires
is needed regardless; a deviation is a defect, and the only open question about
fixing it is when. Code that is "correct within the current model" is not
correct when the model itself deviates. Before calling something a deviation,
check the specification's text and, where cheap, the runtime's behaviour with a
probe program. Valid syntax behaves consistently: no construct is left out of a
fix, an optimization or a rule because it is rare, since rarity describes
today's programs, not the language. When a rule is safe in one syntactic
position, do the analysis for its sibling positions instead of excluding them.

**Performance decides among options that preserve the semantics.** It is the
project's highest priority, and options are compared by the performance the
finished system reaches -- after the inline caches and the JIT tiers -- not by
what the next step gains; an interim speedup the end state replaces is a
separate, lesser question. Correctness the compiler checks and lower run-time
cost both outrank how pleasant the Rust is to write, and a higher engineering
effort is accepted for them. Observable behaviour is never traded for speed:
the model is the inlined conditional, which optimizes the common case behind a
guard and falls back to a real send ([compiler.md](compiler.md)).

**Newspeak's own semantics come before a runtime rule.** When the question is
"how do we stop a program from doing X", first arrange the shipped classes --
lexical nesting, access control, enclosing-object authority, facets instead of
whole modules -- so that X is unreachable or harmless, and add a check to the
VM or the instantiation path only when the language cannot express the
guarantee. The factory pattern ([core-library.md](core-library.md), "The
platform") is this rule applied. Guarantees about untrusted input -- decoding
an image, load-time checks -- are the exception: the language cannot protect a
loader from its bytes, so they stay runtime checks.

**`unsafe` is allowed behind a safe abstraction.** On a performance-critical
path `unsafe` Rust is acceptable when the abstraction over it is safe again and
the unsafe code is confined to where it is needed, as `crates/ns-memory` is
([memory.md](memory.md), "The memory layer"). `#![forbid(unsafe_code)]` in the
other crates is an engineering choice, not an axiom: an option is never ruled
out merely because it needs `unsafe`; the argument against it has to name the
guarantee it gives up or the complexity it adds.

## The map

| Aspect | Document | Status | Planned next |
|---|---|---|---|
| Lexer, parser, AST | [frontend.md](frontend.md) | Built: the syntax is the specification's grammar (§4.2 to §8.1), strictly, read as a PEG over tokens, every rule pinned by a parser test and every accepted form by a golden; where the specification contradicts itself its prose decides, and two deviations are recorded (a modifier needs no whitespace after it; `///` starts a line comment wherever it stands); a syntax tree lossless except for layout, every comment attached to its node; for an editor, a recovered module and its comments, each with its text, for every source | A pattern literal lowered to the `Pattern` send §5.1.9 defines, not to **nil** ([frontend.md](frontend.md), "What is kept but ignored, and what a reprint drops"); chains (`:\|`) and object literals, both marked *Unimplemented* in the specification |
| Formatting: the printer, `ns-next-analyzer fmt`, the repository's layout | [formatting.md](formatting.md) | Built: a pretty printer over the syntax tree in `ns-syntax` (`print_module`, `format`) with twenty-five fixed rules and no options -- 100 columns, two spaces, outside-in breaking, block arguments hugged -- laid out from a document of groups and alternatives; `ns-next-analyzer fmt` and `textDocument/formatting` run it, and every `.ns` outside `corpus/` and `prior-art/` is held to be its fixed point | range formatting and formatting on type |
| Values, objects, mixins, classes, instantiation | [object-model.md](object-model.md) | Built, §6.2's mixin application included: each `<:` clause adds an ordinary class level, so lookup, `super`, `outer` and the caches need nothing new. §3.2's class hierarchy is reified: every class object's class is its metaclass, over `Class`, and every metaclass's class is `Metaclass`. All four of §3.1.1's value conditions are enforced, refused where an instance is made, and a module's root may name only `Object` or `Value`, or write `Object <: Value`. §6.3.2's simultaneous slots, through pull futures: a clause's slots initialize simultaneously, so well-founded mutual recursion between them resolves ("Simultaneous slots"). A value is one tagged word, Spur's: a handle, a 61-bit `SmallInteger`, a `SmallFloat64` or `nil`, `true` and `false`, every other number a `LargeInteger`, a boxed `Float` or a `Fraction`; a heap object stores bare eight-byte words, and a store checks the value it stores ("Values and objects"). Every object is a header word, a class word and its fields, its kind a format -- class objects, closures and reified frames included (road step 3a, "The object layout") -- in one VM-owned object space, a handle the object's 64-bit byte offset (road step 3b, "Handles into the object space"); `Symbol` is a subclass of `String`. `become:` is an eager bulk forward behind one VM-mirror leaf, and a slot access whose index was decided without looking at the receiver checks a recorded guard (road step 5b1, "`become:` is an eager bulk forward", "What a slot access checks"); its caller is road step 5b2a's atomic install, which re-applies every class object of an edited mixin in place and migrates the instances whose layout changed by their fields' declaring level and name, an activation entered before it passing the slot check at its level's successor ("Everything stays editable", "What a slot access checks"). A full collection reclaims every application, guard id, module tree of mixin ids, superseded version and code id nothing names any more, gathered while marking, and prunes the application cache by them; every table mints its smallest free row first (road step 5b2c, "An application has two ids"). `new` is compiled code, as both references run it: the factory entry carries a synthesized body that allocates under a header precomputed on the application, runs the class's initializer and answers the instance, and each mixin carries a full, an own-part and a standalone initializer body, each level's invoking its superclass's from inside its own activation, levels that run nothing passed in a loop; a declaration without a body is the mixin application it writes, its own code resolving in its own scope, and a nested-class accessor's cache hit is a guard and one field load ("Instantiation", "Mixin application", "The nested-class accessor") | the JIT tiers ("Planned: the road to a high-performance VM") |
| Name resolution, lookup, access control, sends | [dispatch.md](dispatch.md) | Built, the inline caches included: primitives as method-table entries (3a) — since M4's step 6 on the VM mirror's class alone, with kernel primitive methods run in `act` without an activation — per-heap caches with a global cache behind them (3b), the five refinements decided before 3c — flat metaclasses, the primary factory as a metaclass entry, the lexical global-cache key, unserialized call-site numbers and the cache's least-recently-used groups — and the dispatch id in the object header (3c), read from the header word's low 22 bits since road step 3a ("The guard in the header word"); M4's step 3 is complete; a §5.7 member set is now keyed by a real class identity, and a setter send skips every local that declares no setter (a parameter or an immutable local) | Open: whether the class side gets a scope of its own ("Open: the class side's scope"); guarding a private lexical site on the mixin rather than its application, once a program has one to measure |
| AST to bytecode: instructions, lowering, inlining | [compiler.md](compiler.md) | Built; call sites numbered per code family, the capture analysis and converted register layout closure conversion needs, an inlined fast path for the nine binary integer leaves, and a kernel mirror send's failure block lowered inline (`SendMirror`); stated limits on a body's registers, instructions and arguments and a declaration's `<:` clauses ("Limits"), and the leftover analysis (`bytecode/leftovers.rs`), which finds the registers a frame may hold that a callee left, both lowerings checking every body against it and the decoder every image's ("What a callee leaves") | dropping `IntegerBinary`'s `Jump` |
| Activations, the VM loop | [execution.md](execution.md) | Built, including the trampoline; nothing re-enters the loop from Rust, the loop runs over the frame's state in locals, one entry per stretch between two safepoints, and polls a safepoint, its sends go through the inline caches, and the optimized lowering converts closures — a captured variable is copied into the closure or lives in its scope's indirection vector, and only by-reference code keeps a returning frame's registers; a body is stored and run as eight-byte instructions with side tables, read back as `Instruction`s through a decoded view, each send kind joining its own send path ("The compact instruction encoding"); a running activation is suspended into its frame and resumed from it lazily, each sender when something returns into it, and a stress mode runs every program on from suspended frames (road step 7b, "Suspending and resuming"); every running activation is a frame of tagged words in one reservation, every word of which is traced, with no stack map, walked frame by frame (`Heap::trace_stack`; road step 7a, "Representation"), measured no slower than the stack before it ("What frames as words moved"); a send's arguments are contiguous slots or consecutive sources, read in place through an argument window, and a send's own callee lies on its slots, which are its first registers, so that its arguments are copied into no window of its own, neither a push nor a pop writing a scratch register, which the leftover analysis decides the collector's trace nils (road step 7a′, "Overlapping register windows", "What contiguous arguments moved", "What overlapping windows moved", "What the leftover analysis moved"); the baseline JIT on x64 and on AArch64 Linux and Apple (road step 8's six stages, "The baseline JIT's foundation", "Sends, returns and the exit that materializes", "Stage 4: closures, non-local return, allocation and the barrier", "When a body is compiled"): `ns-memory`'s doubly mapped code zone and the entry into machine code, `ns-assembler`'s operations and its x64 and AArch64 encoders, and in `jit/` the macro assembler of the VM's idioms and its register roles, the stubs and the baseline compiler's one exhaustive `match`, whose templates cover moves, immediate literals, jumps with a poll on every back edge, `SmallInteger` arithmetic, the cached sends, which call through machine-readable cells beside the inline caches' entries into a callee's machine code or a stub (polymorphic, megamorphic, accessor, mirror method, leaf), the returns and the leaf-safe mirror sends; a body is compiled at its second entry and runs as machine code up to its first instruction without a template, an exit at any depth of compiled frames materializes them all, an interpreter's send calls a compiled callee's machine code through a call stub and every other entry crosses a bridge whose block lasts between entries, so that every compiled body is entered ("The entry at a send"), one that keeps leaving is abandoned and revived by the decay of its exits or by its interpreted entries; stage 4 runs allocation with the write barrier inline, the own slots, instantiation and the initializers, `MakeClosure` and a closure's activation as a compiled call, the outer locals, the futures and the cut of a non-local return in machine code, every body compiling lean with a reifying twin compiled on demand, and the code zone is compacted and frees the code of dead bodies ("The code zone"); stage 5 switches an interpreted frame into its body's machine code at a backward jump, compiling the body at the 40th if it has none, takes a frame that left its code for a poll's safepoint back into it at the loop's head, and makes a compiled push's checks of the depth and of a safepoint request one compare; stage 6 assembles all of it for AAPCS64, roles mapped one to one from x64's and the return address in a 16-byte slot on `sp`, Windows on ARM staying interpreted; every golden runs through the JIT under three stress modes, and the suite runs on AArch64 under QEMU; measured against `master` in "What sends in machine code moved", "What the entry at a send moved" and stage 4's four measurements and "What on-stack replacement moved", x64's machine code byte for byte unchanged by stage 6 and AArch64 unmeasured, since QEMU times nothing; the optimizing JIT's first three stages (road step 9), its foundation ("The optimizing tier's foundation"): in `jit/optimizing/`, a body whose budget -- a row per code id in `heap/optimization_budgets.rs`, which its baseline code spends through a data word of each copy's, pointed afresh after every full collection -- runs out is snapshotted at the next safepoint, built into an SSA graph in one pass, its values kept in machine registers between interrupt points by a linear scan that writes every value back into the frame before each, so that the frame is the interpreter's at every one, every instruction but register arithmetic emitted by the baseline's own template, the caches' feedback read as layout only (a float-fed `IntegerBinary` sent alone); installed beside its baseline code, entered at pushes and compiled calls, left for the baseline code wherever a frame goes on mid-body, abandoned alone when it keeps exiting; two stress modes, the optimize and the deopt mode, and the optimizer's goldens; measured -1.45% on the geometric mean against `master` ("What the optimizing tier's foundation moved"); and speculation ("Speculation"): a method body's optimized code a copy per receiver application and entered application, entered by a receiver-guarded cell, the push and, through the dispatching rows of `heap/optimized_copies.rs` and a shared dispatching stub, a `super`, a private lexical and an interpreted send, its own slots read at fixed offsets; a site, an integer fallback and a conditional's non-Boolean way the caches' executed and trapped bits say never ran a guard that traps; a second trap at a site or the trap limit making a copy not entrant, an abandonment raising its version too, each version waiting four times longer, the last compiled without traps and a failure past it refusing the pair for good; an edit, a bulk forward and a reclaimed id freeing the copies they make stale at once; the invalidate stress mode and the speculation's goldens; measured -3.17% on the geometric mean against `master` ("What speculation moved"); and inlining ("Inlining"): a plan over a tree of snapshots of the body and its callees, ordered by frequency under limits measured, a cached send a type-case of up to four inlined callees in the cache's order, accessors, quick methods, `==`, `class`, `SmallInteger` operations and indexed access nodes, every inlined scope lying in the physical frame where the interpreter would push its activation, a block compiled with its home and eliminated where nothing lets it escape, the exit writing every scope's header and making the blocks it finds live, the recompilee found by walking up the stack, a frame that left at a poll gone back into its optimized code there, and a copy freed when anything it inlined changes; measured -34.0% on the geometric mean against `master` ("What inlining moved"); and `Float` in machine code ("Floats in machine code"): `jit/floats.rs` computes `Float`'s arithmetic, orderings, `=`, `sqrt`, sign, class tests, roundings and `asInteger` and a `SmallInteger`'s mixed rows bit for bit as their leaves do, a result outside the `SmallFloat64` range boxed in eden, through a row stub per row in baseline code and as a tagged `Node::FloatOperation` in optimized code, every x86_64 binary built for x86-64-v2 and refusing a processor without it at start; measured -10.1% on the geometric mean against `master`, Mandelbrot 75%, NBody 72% and CD 16% faster ("What floats in machine code moved"); and a debugger's breakpoint, which patches its body's slots in place, freeing the body's machine code and every copy that inlined it (`jit::free_for_patch`), the baseline lowering a patched slot as an exit to the run loop's `Breakpoint` arm (`vm.rs`, deciding in `interpreter/breakpoint.rs`) and the optimizing tier neither optimizing nor inlining a patched body ("Invalidation"; [debugger.md](debugger.md), "Where the original lives") | The optimizing JIT's seven later stages ("Planned: the optimizing JIT", "Stages"), inlined frame objects and the non-local return into an inlined home the next, built to the deoptimization contract road step 7c wrote ("Planned: the deoptimization contract") |
| Object lifetime, identity, collection | [memory.md](memory.md) | Built — the memory layer, `crates/ns-memory` ("The memory layer"), with the JIT's code zone, mapped twice, and the entry into machine code ([execution.md](execution.md), "The baseline JIT's foundation"), and in it the object space: every object end to end in one reservation, byte-offset handles, bump allocation (road step 3b, "The object space"); frames objects in it, the activation stack in a reservation of its own and the side tables of continuations and of suspended activations, the safepoint, tracing by format, host roots, the stress modes, the lifetime brand; two generations (road step 4, "Generations"): a nursery of eden and two survivor spaces below the old space, a copying scavenger with age-based tenuring, a write barrier and remembered set behind the store accessors, large objects allocated old, and a mark-compact full collection that marks in the header and slides the survivors of both generations into the old space through a live-word bitmap; an object is reclaimed once nothing reaches it, and both collectors clear a `WeakArray`'s slots and mourn an `Ephemeron` whose key nothing else reaches ("Weak references"); inline allocation (road step 6, "Inline allocation"): eden's top is a bump cursor the reservation holds, a young object whose header holds its count is one comparison against a limit below the scavenge threshold, its header precomputed for instances, closures, frames and boxed floats, a body's code id cached per heap, and every other allocation the cold `allocate_block`, and machine code bumps the same cursor and makes the barrier's decision inline; every table beside the space has a growth limit that requests a full collection, the symbols a program makes take no number and die with their objects, and the reservations beside the space give back what a peak committed by a band and a damping ("The trigger and the growth policy", "Giving committed memory back", "Weak references") | The optimizing JIT, which emits the same bump ([execution.md](execution.md), "Planned: JIT tiers") |
| Serialization: the packaged application, the project file | [image-format.md](image-format.md) | Built: one artifact, an application packaged just before `main:args:`, carrying every module of a project; a project is the directory its `newspeak.json` describes (`main`, `modules`, `scripts` for `--script`), described by `schemas/newspeak.schema.json`, which the agreement test `crates/ns-cli/tests/project_schema.rs` holds the reader to ("Packaging"); frames are object records, and the decoder sizes every record from its own bytes before it allocates any ("Two passes"); a load tenures once before the graph pass, which allocates old ("The image under generations"); an image names every object the binary owns -- a shipped module's instance, a class its root declares, their metaclasses and levels -- by its declaration, resolved by table lookup against the loading binary's shipped world, of which the loader first builds, by the binary's own code, the modules and classes the image names that the boot did not, under a rule of what may be named where, and its header carries that world's fingerprint ("Shipped names"); an image carries its program's source unless built with `--strip-source`, each declaration's own text in a table of its own ("Source"), and a program a MirrorBuilder edited is written as it now is, the code an older version runs in the code table and every frame naming its body (road step 5b2b, "Edited programs in the image") | Suspend and resume ("Planned: suspend and resume") |
| Builtins and the platform | [core-library.md](core-library.md) | Built, the kernel completed to Primordial Soup's core API ("The kernel completed to Primordial Soup's core API"), as Newspeak source over a VM mirror: every builtin method is Newspeak in `crates/ns-runtime/core-library/Kernel.ns` -- a forwarder to one of the mirror's `MIRROR` rows, which are the leaves, or a quick method or ordinary method that reaches no leaf, as all of `True`'s, `False`'s and the nil protocol's are; `Closure`'s `whileTrue:` and `whileFalse:` and `Array>>do:` are Newspeak loops, so that the JIT tiers see through them ("Real bodies for control flow and `do:`"). Its hierarchy is §3.2's and §3.1.1's — `Top`, then `Object`, then `Value` over the value classes with `Number` among them, and `Behavior` over `Class` and `Metaclass` — and `Value` derives `=`, `==` and `hash` from an instance's state. Every class object's class is its own metaclass, its class side applied to `Class`, and every metaclass's class is `Metaclass`; a class object is a value when its enclosing object is. `Object` carries §3.2's accessors for the predefined classes, so a bare `String` is a send answered without an activation; they and `class` are protected, so holding an object does not hand out its class. §8.6's exceptions are Newspeak over kernel-private activation objects: `Exception`, `Error`, `MessageNotUnderstood`, `Halt`, `Message`, `ArgumentError`, `CannotReturn` and `CannotInstantiate`, reached through `platform kernel` and the two roots also bare, through `Object`'s accessors, with ANSI's `on:do:`, `ensure:`, `ifCurtailed:`, `return:`, `retry`, `resume:` and `pass`; every VM error and every message nothing understands is signalled, and the depth limit keeps a reserve for signalling its own; `platform kernel` answers `Proxy` and the public `Future` too, whose `computing:` makes the runtime's future, and every core class answers its own `isKindOf…` question, `false` answering one it does not declare. The platform is an instance of the nested `Platform` of the shipped module `crates/ns-runtime/core-library/PlatformFactory.ns`, whose root is booted with the mirror and the kernel instance from the table of shipped modules (`interpreter/core_library.rs`) and never handed out (the factory pattern); its transcript, `collections`, `kernel`, `mirrors`, `time` and `typedArrays` members are lazy slots, none of its module's classes is handed out (`time` hands out `Stopwatch`), and an image names every shipped class by its declaration. The transcript, the clock, the collections, the typed arrays and the mirrors are the facets of shipped modules of their own under `core-library/platform/` -- `TranscriptFactory.ns`, `TimeFactory.ns`, `CollectionsFactory.ns`, `TypedArraysFactory.ns` and `MirrorsFactory.ns` -- each with one public member, `build`, and every module root is `= Value`; the boot builds `Kernel` and `PlatformFactory`, and every other shipped module is built once, on its first demand, by `PlatformFactory`'s root through the mirror's `shippedModuleClassNamed:` and `recordShippedModule:` and reached afterwards through `moduleInstanceNamed:` ("The module table and the boot"), each from the compiled code `cargo xtask shipped-code` generates and the binary embeds on an optimized boot ("The embedded shipped code"); the collections module's its `List`, `Map`, `IdentityMap` and `Set` are Newspeak written here, under the kernel's abstract `Collection` with `Array`, each with Primordial Soup's protocol and signalling `ConcurrentModification` when changed while iterated. A `String` is its code points, one byte or four each ("Strings are code points in two widths"), with Primordial Soup's protocol in code points and the UTF-8 conversions `asUtf8` and `String fromUtf8:`, and a NaN equals a NaN. `ByteArray`, a builtin whose bytes lie at a 32-byte address that every mover realigns, shares the sequence protocol with `Array` through the kernel mixin `IndexedCollection`; `StringBuilder` and `DataView` are ordinary kernel classes over a `ByteArray`, reached through `platform kernel`, and `TypedArray` with its ten classes is the module `TypedArraysFactory.ns`, reached through `platform typedArrays` ("`ByteArray`, `StringBuilder` and the views"). Integers are unbounded, as §5.1.1 requires: one that does not fit a `SmallInteger`'s 61 bits is a `LargeInteger`, a kernel subclass of `Integer` whose arithmetic (`crates/ns-runtime/src/arbitrary_integer/`) is adapted from `num-bigint` and `num-integer` ("Large integers"). The numbers are a symmetric tower of `Integer`, `LargeInteger`, `Float` and `Fraction`, a builtin made only in normal form: any two compare by exact value and hash alike when equal, arithmetic without a `Float` is exact, so `7 / 2` is `(7/2)`, and `Number`, `Integer`, `Float` and `Fraction` carry Primordial Soup's whole protocol ("The numeric tower"). `WeakArray`, a builtin of the format `WeakPointers`, and `Ephemeron`, a builtin of the format `Ephemeron` whose value and finalizer live only while something else reaches its key, are weak in both collectors and written to an image as a collection would leave them; `WeakMap` is Newspeak over ephemerons, a weak structure refuses a value as a key, the finalizer is held and never run, and `platform kernel garbageCollect` runs a full collection ("Weak references") | No step of the road; what is open is in [OPEN-WORK.md](../OPEN-WORK.md), "Planned: language and library" |
| Mirrors, live editing, actors | [liveness-and-reflection.md](liveness-and-reflection.md) | Mirrors that read are built (road step 5a, "Mirrors that read"): `platform mirrors`, the facet of the shipped module `MirrorsFactory.ns`, hands out `ObjectMirror` and `ClassMirror`, from which a program navigates to mixin, class declaration, method and slot mirrors and reads names, access modifiers, slot declarations, the superclass chain, source and the values of its own objects' slots; no mirror hands out a value it reached through a shipped level, a class mirror reached by navigation holds the way to its class rather than the class, and a mixin is reified as one kernel-private `Mixin` object per mixin, written to an image as a record of its own. Every version of a mixin answers the one id it was first registered under, so a fresh mixin can be installed under an old one's id (road step 5b1, "An edit replaces the mixin under its id"). A MirrorBuilder and an atomic install are built (road step 5b2a, "A MirrorBuilder and an atomic install"): `ClassDeclarationMirror>>asBuilder` stages edits of a program's class -- its header, its methods on either side, its lazy slots -- as source, checked as they are staged, and `install` recompiles the class's subtree from its module's edited text and commits in one leaf with no safepoint, every application of the edited mixin re-applied, a successor recorded for each level whose slots stayed, and the instances whose layout changed migrated by their fields' declaring level and name in one bulk forward, or changes nothing; a shipped class is refused. Every declaration holds its own text, its nested declarations cut out, and a module's text is assembled from them; an edited program is written to an image and loads back edited, and a program loaded from an image with source is edited like one read from source (road step 5b2b, "Edited programs in images"). A full collection reclaims the ids live editing leaves behind, so a program that edits and installs in a loop keeps its tables at a bounded size (road step 5b2c, "What the id has to carry"). The rest is planned | actors; reflection beyond road step 5b2a: a superclass change, the nested classes, a class declared from source with no predecessor ([OPEN-WORK.md](../OPEN-WORK.md)) |
| The debugger: breakpoints, stepping, inspection, evaluation, edit-and-continue, the protocol and the IDE | [debugger.md](debugger.md) | Partial — stages 1 to 3 are built ("The debugging API at the safepoint", `crates/ns-runtime/src/debugger/`, its tests `crates/ns-runtime/tests/debugger.rs`): `Interpreter::attach_debugger` hands out a `DebugRemote` that another thread pauses the program with, wherever it runs, interpreted or in machine code, optimized code included, by arming the poll words, now atomics behind an `Arc` whose request word is a set of bits (the VM's request, the stress hold, the debugger's pause, commands, held step and requested stop), under one arming lock per interpreter; the safepoint serves the debugger first, the commands queued while the program runs and then the pause, whose stop deoptimizes a frame stopped at a poll of optimized code in every JIT build and blocks on the command channel; a stop answers the run's frames, each frame's name and receiver, and, from its body's metadata, the statement and the send it stands at in its module's current text and its named variables over its whole lexical chain, by number without metadata, and an instance's fields and an `Array`'s elements, paged, every value held as a `Root` for the stop and named by an id no later stop reuses; `Continue` goes on, `Terminate` ends the run through the unwinder, its `ensure:` blocks run; and the pause stress mode stops every program at the safepoints its schedule picks and reads its whole stack. Stage 3, breakpoints and stepping, is built as well: a line breakpoint (`debugger/breakpoints.rs`), set by its module's name and an offset into its text, served while the program runs, at a stop and between runs, pending until its module is built and set again on the bodies an install replaces, patches its statement's slots in place and stops the interpreter there, the run loop's arm sharing an out-of-line arm and running the original through its ordinary arm, its decision in `interpreter/breakpoint.rs`; a patch frees the body's machine code and every optimized copy that inlined it (`jit::free_for_patch`), the baseline lowers a patched slot as an exit, the optimizing tier neither optimizes nor inlines a patched body, and a method's fast path is off while a breakpoint stands in its body; step over, step into and step out (`debugger/stepping.rs`) patch one-shot breakpoints for the stepping frame at its statement starts, its inlined blocks' and its caller's return point, a step into holding a request bit until the next push, and report the value of the last call; a step whose frame an unwind pops, or whose frame returns otherwise than to its caller's send, goes on as a step into from the frame the program goes on in, reported by the unwinder and the run loop's cold arms; and no step lands in a frame the step filters filter (`Command::SetStepFilters`), by default the shipped modules', a stop it would make there going on as a step into from the first unfiltered frame below; run to cursor (`Command::RunToPosition`) stops once at a position resolved as a line breakpoint is, in any activation; and a method breakpoint (`Command::SetMethodBreakpoints`), named by its class declaration, side and selector, pending until its module declares it, stops at its method's entry and before each of its returns, its inlined blocks' non-local returns included, and reports an exit an unwind makes where the program goes on; and exception breakpoints (`Command::SetExceptionBreakpoints`, `debugger/exceptions.rs`), decided by two mirror rows the kernel's handler search sends (a leaf, and the exit that reads the handler it found), stop an uncaught exception before its `defaultAction` runs, a caught one before its handler runs, each filtered by class declaration along the exception's class chain, and an uncaught `halt`, which goes on resumed with nil, the frames above the first one below the exception's machinery marked hidden; the search reports the handler it found to a step in progress, which goes on into the handler block; and the breakpoint stress mode (`NS_NEXT_STRESS_BREAKPOINTS`) patches every statement start of every body it finds running at a safepoint with a breakpoint that never stops, each gone past through the run loop's arm; and the step stress mode (`NS_NEXT_STRESS_STEP`) stops every program at the safepoints its schedule picks and takes a burst of steps into, over and out from there, the shipped filter on and off, reading the stack at every stop. The metadata (stage 2, "Compiling the metadata again", `debugger/metadata/`): both lowerers collect a body's statement starts, its sends with their receiver and selector spans and its named variables when the debugger compiles the body's declaration's level again from its module's current text, under the interpreter's lowering and then the other, each body's tables kept, in a cache cleared at every install, only when the body compiled again is alike (`bytecode::compiles_alike`); an oracle holds every body of every golden, from source and through `build` and `load`, and of the shipped modules to compiling again alike with tables that hold their invariants. Its design is decided in seventeen decisions (one debugging API in Rust with the DAP agent in `ns-cli` and `ActivationMirror` as its clients, breakpoints patched into the body in place, stepping by one-shot breakpoints and the safepoint, statement starts as stop locations with every send recorded, metadata compiled again on demand and verified, DAP over a localhost socket, pause arming the poll words atomically, an expression evaluated as a block in the stop's scope, and a stop waiting at the run loop's safepoint, and for stage 2 a block's frame showing its whole lexical chain, a body found by its declaration and equal by its code, a factory's frame placed by its class header, named variables only, the metadata checked on three levels, and selector spans in the syntax tree, and for stage 3 the shipped code filtered from steps, program classes on request, and a step out stopping at the next place the program reaches); the mechanisms they imply are worked out, and fourteen smaller questions are open ("Open questions"); what stage 3 costs a program without a debugger was measured ("What stage 3 moved": the geometric mean +0.36%, `bench_signal` +0.3% at forty iterations, nothing separated, the unwinder testing once per unwind whether the debugger follows it) | stage 4, the DAP agent in `ns-cli` and the plugin's `XDebugProcess` ("Planned: the order of building it") |
| Editor support and the analyzer: the language server, the JetBrains plugin, `ns-next-analyzer` | [editor-support.md](editor-support.md) | Built — highlighting, doc comments (a block of `///` lines with Java-style tags, coloured, rendered in place as Markdown, and shown in a hover beside the signature of the declaration a name names), errors as you type, code folding, hints with their quick fixes and a structure view with breadcrumbs in every commercial JetBrains IDE from 2026.1.4, kept through syntax errors; the server bundled for Windows x64. For `newspeak.json`, the schema from `schemas/`, and a "Newspeak" run configuration with play buttons that run or benchmark the project with the `ns-next` the settings name; a module `ns-next` can run from its source gets a play button beside its `main:args:` and "Run" in its context menu, from the document symbols the structure view shares. The server is `ns-next-analyzer lsp`; the same binary parses (`parse`, `parse-corpus`), prints a compiled body (`disasm`, [compiler.md](compiler.md)) and an image's structure (`inspect`, [image-format.md](image-format.md)), and never runs a program; `fmt` formats sources and the server answers `textDocument/formatting` ([formatting.md](formatting.md)) | range formatting and formatting on type, go to definition and find usages, more platforms; a heap dump and inspection of JIT-compiled code as further analyzer commands |

Real programs to measure the runtime on, beside the microbenchmarks under
`benchmarks/micro/`, live in `benchmarks/`: today
`benchmarks/are-we-fast-yet/newspeak/`, the fourteen Are We Fast Yet benchmarks,
Primordial Soup's Ton80 Richards and DeltaBlue, their harness and the SOM
collection library they share (`Som.ns`), with
their Java and Python counterparts under `benchmarks/are-we-fast-yet/java/`
and `benchmarks/are-we-fast-yet/python/`, which
[execution.md](execution.md), "The Are We Fast Yet benchmarks", describes.
Every benchmark program is held to the outcome recorded beside it --
`benchmarks/micro/<name>.toml`, `benchmarks/are-we-fast-yet/newspeak.toml` -- by
`crates/ns-cli/tests/run_benchmarks.rs`, under both lowerings, and each Java
and Python counterpart to the same line of `benchmarks/are-we-fast-yet/newspeak.toml`,
in the nextest profile `benchmarks-verify` only, which needs a JDK and a `python`
on `PATH`: a benchmark is
sized to be measured, not to run on every commit. The profile verifies; it
measures nothing.

## Architecture

Seven crates, with the name-resolution pass as the seam the bytecode compiler
plugs into, and the JetBrains plugin beside them:

```
crates/
  ns-syntax/    lexer, tokens, a lossless AST with its comments attached,
                spans, diagnostics, the printer (print_module, format);
                for an editor, parse_for_editor: a recovered module and
                its comments
  ns-runtime/   resolver, object model, dispatch, compiler, VM, the leaves, image;
                core-library/Kernel.ns, the core library in Newspeak;
                jit/, the baseline JIT: the compiler's one match over
                the instructions, the VM's idioms, the sends and the
                stubs their cells name, the closures, outer locals and
                futures, the cut of a non-local return, the exit's
                materialization, the trigger and abandonment, the code
                zone's compaction
  ns-cli/       the `ns-next` binary, which runs programs: run, run-corpus,
                load, bench, build
  ns-tooling/   what both binaries share: loading a source, collecting `.ns`
                files, printing diagnostics
  ns-analyzer/  the `ns-next-analyzer` binary, which looks without running:
                lsp (the language server: diagnostics, semantic tokens,
                fold ranges, document symbols, formatting, hints and
                their quick fixes), dump-tokens,
                parse, parse-corpus, disasm, inspect, and fmt, which
                rewrites sources in the formatter's layout
  ns-memory/    reserved, committed and protected memory, and the JIT's code
                zone, mapped twice, with the entry into machine code;
                the only crate with unsafe
  ns-assembler/ the machine operations the JIT emits, double-precision
                ones among them, and the x64 and AArch64 encoders,
                knowing nothing of the VM
editors/
  jetbrains/    the JetBrains plugin: a Kotlin lexer mirroring ns-syntax's,
                highlighting, a structure view, and the connection to
                ns-next-analyzer over the native LSP API
```

Inside `ns-runtime`, one file cuts across every other: `heap.rs`. It owns the
object space (`heap/space.rs`), every object end to end in one reservation of
`ns-memory` -- header word, class word, fields -- and the activation stack
every running activation lives on, frames of tagged words in a
reservation of its own (`heap/frames.rs`); an activation becomes a frame
object in
the space only when something reifies it. `ObjectReference`,
`ClassReference` and `FrameReference` are opaque handles, each an object's
64-bit byte offset in the space, and `ActivationReference` its frame's word
offset on the stack; each one's inner field is private to the `heap`
module (`heap.rs` and
its submodules, `heap/value.rs` among them, which puts an object's offset into
a value's word), so every read and write goes through a `Heap` accessor
because the compiler refuses anything else. That seam exists so the store
behind a handle can be replaced by changing one module rather than every
caller; replacing `Rc<RefCell<_>>` with an arena, and the arena with the
object space, is the evidence that it works, since the first changed no call
site outside `heap.rs` and `types.rs`, and the second one call site outside
the `heap` module, the image decoder's. The handle types are not hidden, on purpose: the two activation
handles are distinct types in every signature — a closure, a frame and an image
record take only a `FrameReference` — so storing an `ActivationReference` in
anything that outlives its call is a compile error. And every movable handle
carries the brand of the entry that minted it, a lifetime obtainable only inside
`Interpreter::enter`, which `heap/world.rs`'s `World::enter` gives in safe Rust
— the crate forbids `unsafe` code — so a handle held across a safepoint, or kept
by a host outside an entry, is a compile error too. Start at
[memory.md](memory.md) for what the heap guarantees — a scavenger collects
the nursery and a mark-compact full collection both generations, reclaiming
an object once nothing reaches it, only at a safepoint of the run loop or
between entries —
and at [execution.md](execution.md), "Activations", for the stack.

`ns-syntax` does not depend on `ns-runtime`. Nothing under `crates/ns-syntax`
should change for runtime work.

### From source to a running program

```
Interpreter::bootstrap          the builtin classes, the VM mirror, and
                                core-library/Kernel.ns compiled into them
                                and instantiated with the mirror, then
                                the other shipped modules; an optimized
                                boot decodes their compiled code from the
                                embedded shipped code, every other boot
                                compiles their source
source bytes
  -> ns-syntax::lex             Vec<Token>
  -> ns-syntax::parse           Module (AST + spans)
  -> ns-runtime::resolve        ResolvedModule (bindings + slot layout)
  -> Interpreter::load_module   the module's root class object, its code
                                compiled onto the declarations: one
                                CompiledCode per body, on its mixin, by
                                the interpreter's lowering -- optimized,
                                or the reference lowering it is checked
                                against
  -> assemble                   the root sent `new`, or `packageUsing:` a
                                manifest of the project's other modules:
                                the primary factory, then each level's
                                initializer
  -> main: platform args:       the platform's one way into the program
```

A project's modules go through the front end one after another, in module
order, into one interpreter, and the embedded `Manifest.ns` with them.
`build` stops just before the send of `main:args:` and serializes the
assembled application instead; `load` rebuilds that world without running
the front end at all and sends it `main: platform args:`. See
[image-format.md](image-format.md), "The artifact" and "Packaging".

### The binaries

`ns-next` runs programs: `run`, `run-corpus`, `load`, `bench`, `build` — one
module each under `crates/ns-cli/src/commands/`. `ns-next-analyzer` looks at
Newspeak without running it: `lsp`, `dump-tokens`, `parse`, `parse-corpus`,
`disasm`, `inspect` and `fmt`, one module each under
`crates/ns-analyzer/src/commands/`; `disasm` compiles or decodes and `inspect`
reads an image without loading it, and neither runs the program's code
([editor-support.md](editor-support.md),
"The editor tooling, `crates/ns-analyzer`"); `fmt` is the one command that
writes, rewriting sources in the formatter's layout
([formatting.md](formatting.md)). Each `main.rs` holds only the
argument parser, the dispatch to the commands, the global allocator
([memory.md](memory.md), "The allocator of the Rust side") and the thread
every command runs on — a thread of its own because the symbol interner is thread-local,
spawned with the stack the front end needs ([execution.md](execution.md),
"The depth limit"). What both binaries share is the crate `ns-tooling`:
`source.rs` loads and resolves one source file, collects the `.ns` files under
a directory and prints diagnostics; it is a crate of its own so the language
server's dependencies never reach `ns-next`. What `ns-next`'s commands share
lives beside them: `project.rs` finds a project's `newspeak.json` and reads
it, with its rules, its scripts and its path rules; `command_line.rs` splits
a script into arguments as a POSIX shell splits a command line;
`program.rs` reads a program -- one source file or a project, its modules in
module order -- compiles it and assembles its application with `new` or
`packageUsing:` a manifest, and `entry.rs` sends `main: platform args:` and
reports its outcome. The project file's schema is
`schemas/newspeak.schema.json`, the one copy:
`crates/ns-cli/tests/project_schema.rs` holds the reader and the schema to
one another, and the JetBrains plugin bundles it
([image-format.md](image-format.md), "Packaging").

## The goldens

`goldens/` is this runtime's recorded behaviour. A golden is a `.ns` program
or a project golden, a directory holding a `newspeak.json`
([image-format.md](image-format.md)), and a `.toml` beside it holding the outcome that program must produce:
`status`, `stdout` and `stderr`, each defaulting to the success case (`0`,
empty, empty), so an ordinary golden's file is just its `stdout` and one that
raises writes its `status` and `stderr`. Both lowerings must produce it, byte
for byte and exit status included (`crates/ns-cli/tests/run_goldens.rs`),
and so must machine code, which the suite runs wherever the JIT is built.
A new golden's `.toml` is recorded from the interpreter, with
`NS_NEXT_NO_JIT=1`, so that what pins machine code was never produced by
it.
That runner is a `harness = false` binary on `libtest-mimic`, one test per
golden per lowering named by its path below `goldens/` --
`golden::optimized::<path>` and `golden::reference::<path>`, with `<path>`
such as `language/block/blocks_and_return`, so nextest runs the two lowerings
of a golden in parallel rather than one trial running both in sequence -- and
in a debug build `golden_with_a_two_entry_global_cache::<path>` as
well, which only the nextest profile `caches` runs -- so the suite lists each golden, reports every failure with the expected
and actual stream, runs them in parallel, and runs one or a family by name
(`cargo nextest run -p ns-cli --test run_goldens lazy`); a skipped golden is
listed as skipped, and `--run-ignored all` runs it. The build/load round
trip is the same, one test per golden
(`crates/ns-cli/tests/golden_round_trips.rs`), in the nextest profile
`images` rather than the per-commit suite.
Every golden, the ones recorded as raising and the programs under
`goldens/library/mirror-builder/` that edit themselves included, also
goes through the round trip, `build` then `load`, which holds the started
image's three streams against what `run` answers on the source; a golden
whose assembly fails must fail its `build` with `run`'s status and report.
The image carries the program's source, as `build` writes it by default,
so a program that edits itself after the load edits the text the image
carries ([liveness-and-reflection.md](liveness-and-reflection.md),
"Edited programs in images").

The editor hints have a second runner of the goldens, the behaviour oracle
(`crates/ns-analyzer/tests/hinted_goldens.rs`, [editor-support.md](editor-support.md),
"Hints"): it applies every hint to a golden's sources, runs the changed
program under both lowerings and holds it to the same recorded outcome. It
runs the full-size benchmarks too, so it has a nextest profile of its own,
`hints`, which the default profile leaves out; its runs of the benchmarks
are in the profile `benchmarks-verify`.

Every golden under `goldens/runtime/optimizer/` is a program whose hot body
the optimizing compiler compiles in a plain run (a loop of tens of
thousands of turns, a method called thousands of times), covering what a
value in an optimized frame must survive: sends, closures, a non-local
return, an overflow into `LargeInteger`, scavenges, an exception through
the frame, the depth limit and one mixin applied twice -- and, for the
speculation, a speculation that fails after warm-up: types changing in
a loop, an overflow, a non-Boolean, a branch never run taken late, a
customized copy per level, per receiver and behind a `super` send, and
an edit while an optimized frame is below -- and, for inlining, callees
always inlined, a type-case that widens, inlined recursion that
overflows, an exit inside an inlined callee, an edit below an inlined
frame, eliminated blocks seen after an exit and made on a way not run,
blocks writing their home's locals and returning from it, and a loop
that goes back into its optimized code at its polls. Its outcome is
recorded from the interpreter like any other, and a debug build also runs
it as `golden::optimizer::<name>` with `NS_NEXT_STRESS_OPTIMIZE=1`, which
requests every body for optimization at its first spend.

A golden's root class is instantiated with `new`, and a project golden's
root with `packageUsing:` its manifest; either reaches the platform only as
`main:args:`'s first argument (§8.4), as every program does
([image-format.md](image-format.md), "Packaging"). A project golden is
the directory `goldens/<category>/<name>/`, its `newspeak.json` in it and
`<name>.toml` beside it; its modules live in the directory too, or in a tree
outside `goldens/`, which its `newspeak.json` names by relative paths. The
modules are not goldens themselves, and `goldens/tool/manifest/` pins the
manifest.

A fifth key, `arguments`, lists the program's arguments, which every runner
passes after `--` to each command that sends the entry -- `run`, and `load` in
the round trip -- so `main:args:` receives them (§8.4,
`goldens/language/module/spec_8_4_main_args_receives_the_program_arguments`); most
goldens have none and receive an empty list.

A fourth key, `skipped`, carries why the runtime does not answer this one
yet -- for instance (an illustration: no golden is skipped today)
`skipped = "not implemented yet: a pattern literal evaluates to nil"`. The
sweeps pass it over and count it; everything else about it still holds,
including the pairing check, so a `skipped` cannot switch a half-written
golden past discovery. The other three keys
still record what the program *should* answer, which is the point: a gap
gets its golden the day it is found, stating the whole expectation, and
closing the gap is deleting one line. A reason is required, and a blank
one fails. A skipped golden is an ignored test, counted in the suite's
summary; `cargo nextest list -p ns-cli --run-ignored only` names them.

A sixth key, `reaches_a_table_limit`, says why a golden keeps live rows
of a table beside the object space past its growth limit's floor; the
sweep holds every other golden to its run requesting no collection for
a table ([memory.md](memory.md), "The trigger and the growth policy").
One golden has it, `a_chain_deeper_than_the_depth_limit_overflows`. A
reason is required, and a blank one fails.

| Directory | What belongs in it |
|---|---|
| `language/` | what the compiler, the resolver and the object model decide |
| `language/syntax/` | the grammar's rules: the unreserved words, literals and their signs, symbols' forms, precedence, cascades, type parameters |
| `language/module/` | a module's root: every kind of root the root rule refuses, a module with a language id and no category, and `main:args:`'s arguments |
| `language/builtin-names/` | which names a program sees: no `Character`, no `Top`, no mirror class, and a member of a program's own named like a builtin, which wins |
| `language/class/` | classes and their objects |
| `language/class/instantiation/` | factories, superclass clauses and their arguments, the initializer and the order instantiation runs in, a clause argument's effect before §6.1's mismatch, and a chain deeper than the depth limit |
| `language/class/class-side/` | class sides and metaclasses: an implicit class side, `outer` from one, class-side `super` reaching `Class`, a class side's `doesNotUnderstand:`, `self class` on a class side as its metaclass, a class's and a metaclass's names, and the factory as a class-side member; the chains of metaclasses and the refusal to instantiate a class object, which no program can reach with `class` protected, are checked from the host by `crates/ns-runtime/tests/representation.rs` |
| `language/class/nesting/` | nested classes: `outer`, redeclaration, overriding, nested `super`, a slot overriding a nested class |
| `language/class/mixin/` | §6.2's mixin application: lookup and `super` through the applied levels, `outer` from an applied mixin, the order of several `<:` clauses, the initializer order, the level's factory selector, the errors a clause raises, a level reached through a mirror, a declaration without a body and the scope its own code resolves in, and a module that declares itself a value with `<: Value` |
| `language/slot/` | slots |
| `language/slot/accessor/` | slot accessors and setters, an immutable slot, and §6.3.3's lazy slots |
| `language/slot/simultaneous/` | §6.3.2's simultaneous slots and temporaries: futures, data cycles, ill-founded recursion and an abandoned clause |
| `language/slot/local/` | a method's locals and the closures that capture them: a closure made before a local is initialized or assigned, a closure in a local's own initializer, a multi-line string a local takes -- the shapes the editor hints' behaviour oracle depends on |
| `language/method/` | a method body: its answer without a return, its parameters and its temporaries (§3.5, §3.6, §5.1.8) |
| `language/send/` | sends and lookup: `doesNotUnderstand:`, a native declining to `Object`'s method, public, protected and private access (§5.5, §6.3.1), an outside `class` or class accessor send not understood and a bare class name answering everywhere, lexical before inherited (§5.7), `self` and `super` sends |
| `language/block/` | blocks and closures: what a block captures -- copied variables, indirection vectors, the fallback closures of an inlined conditional --, a closure outliving its method, non-local return and its `cannotReturn:`, an immutable block parameter |
| `language/control-flow/` | conditionals, `and:`/`or:`, loops and `Number`'s iteration, inlined and through their fallback, and a non-Boolean receiver or condition |
| `language/evaluation-order/` | an operand or an assignment's value that keeps what it evaluated to while a later send writes its variable |
| `language/expression/` | what surrounds an expression without changing what it means: parentheses around an inlinable conditional's receiver and blocks |
| `library/` | the core library: `Kernel.ns`, `PlatformFactory.ns` and the five modules under `platform/` |
| `library/object/` | `nil`, `isNil`, the identity hash and `identityHash`, `yourself`, the `isKindOf…` answers, a `Proxy` subclass forwarding every message, and `Future computing:` |
| `library/number/` | numbers |
| `library/number/integer/` | the inlined integer fast path beside the ways its guard declines, which must answer what the primitive answers; floor division and its remainder, `bitAnd:`, `bitOr:`, `bitXor:`, `<<` and their errors; `gcd:`, `>>`, digits, radix texts, parsing and modular arithmetic |
| `library/number/large-integer/` | large integers: promotion, their arithmetic at every algorithm's size, mixed operands, literals and division by zero |
| `library/number/float/` | `Float`'s arithmetic, mathematics and rounding, its texts and `Float parse:`, an integer meeting a `Float`, a NaN equal to every NaN |
| `library/number/fraction/` | `Fraction`: its normal form, its factories, rounding and conversion |
| `library/number/tower/` | the tower: exact comparison and the hash across the four classes, exact arithmetic without a `Float`, the rounding divisions over every pair, and a `Fraction` with no value |
| `library/string/` | strings and symbols: a String's code points in either width, its searches and iteration, literals, index and search errors, and equal symbols |
| `library/collection/` | `Collection`, `Array`'s protocol, creation and index errors, and the collections module behind `platform collections`: the protocols of `List`, `Map`, `IdentityMap` and `Set`, their errors, a change during an iteration, `List`'s queue and growth, a user subclass, and one module shared by every platform |
| `library/exception/` | §8.6's exceptions: signalling, handling, `return:`, `retry`, `resume:` and `pass`, the handler environment, `ensure:` and `ifCurtailed:` on every way out, VM errors and `doesNotUnderstand:` caught, the error classes caught by name with what they carry, and an unhandled one ending the run |
| `library/value/` | §3.1.1's value semantics: a value's derived equality and hash, its lack of identity, and every one of its value conditions, those decided at class creation included, each refused at instantiation, and a class object that is a value because its enclosing object is |
| `library/builtin-subclass/` | what a user subclass of a builtin inherits, falls back to and how it hashes, and §5.10's handler passed by through `super` |
| `library/platform/` | the platform's parts: the transcript, the stopwatch, a second platform, and the platform as a value |
| `library/mirror/` | the mirrors behind `platform mirrors`: one facet for every platform, an object mirror reading an object's class and slots without sending it anything and only the slot a name names, a class mirror's superclass chain and inherited members, a mixin mirror's own members and declaration, a synthesized class side as one mixin, method mirrors equal across applications, a core-library method's source, and the shipped boundary: nothing a shipped level holds handed out, a program class over a shipped one reading only its own slots, and a class reached by navigation held back, `Top` included |
| `library/weak/` | weak references: `WeakArray`'s protocol and refusals, a slot cleared after `garbageCollect`, an `Ephemeron`'s accessors, a value it keeps while its key lives, a chain of ephemerons collected together and a subclass that holds strongly, and `WeakMap`'s protocol, reinsertion and an entry forgotten with its key |
| `runtime/` | what the VM guarantees beyond the language's rules |
| `runtime/cache/` | an outcome an inline cache could change, each added before any cache existed |
| `runtime/depth-limit/` | the activation limit: what still runs at 1000 deep, what overflows, the reserve a handler runs in, and a recursion through the core library the two lowerings count alike |
| `tool/` | the tools around `ns-next run`: projects |
| `tool/manifest/` | projects: a root assembled with `packageUsing:` and the manifest of the other modules, a name the manifest does not hold, and a manifest the application keeps through `build` and `load`; each project's modules in the directory of its name |

A golden named `spec_<section>_...` pins one named rule of
`newspeak-spec.pdf`, its section in the file name; it lives with its theme.

The goldens are a tree. A directory that holds files is a category and holds
goldens only; one that holds none groups categories or further groups. The
set is found by walking it — no list, no count, so adding a golden needs no
edit anywhere, and `cargo nextest run -p ns-cli --test run_goldens
library/number` runs one subtree. A project golden is a
directory holding a `newspeak.json`, the one directory a category may hold;
its modules live in it, or in a tree outside `goldens/`; neither's contents
are goldens, and neither is walked. A file at the
root, a directory beside goldens or an empty directory is refused by name
rather than skipped, because a golden nothing discovers is not a golden at
all. `every_golden_is_a_complete_pair` covers the other half: an orphan on
either side is a golden that stopped being discovered because one half was
renamed, moved or re-extended. Deleting both halves together is not caught,
and shows up in a diff as two deletions.

A golden's rationale — why that outcome is the right one, and which
specification section it comes from — is a `///` comment at the top of
the `.ns`, so that it sits with the program it
explains rather than beside it; for a project golden, the `.ns` is its root
module. The `.toml` holds the recorded answer and nothing else. A new Are
We Fast Yet benchmark is added to the harness tuple, its project file and
`benchmarks/are-we-fast-yet/newspeak.toml`, because the harness names each benchmark
when it is instantiated; `run_benchmarks` takes the list of benchmarks it
runs one by one from that `.toml`.

## Gates

These are non-regression gates, not suggestions. Each has caught a real
regression. Which gate runs when -- every commit, a commit touching a given
part, the end of a batch -- is the table "The gates" in `CLAUDE.md`, the one
place it is kept; the suite there is every crate's, ns-analyzer's included,
whose `disasm` and `inspect` tests guard the compiler's output and the image
format. The stress modes run the suite again, each with one of the
runtime's own stress switches set, and the stress gate runs it once more
with the JIT off, the one run in which the interpreter runs every body:

```bash
NS_NEXT_STRESS_MOVES=1 cargo nextest run --workspace    # moving everything at the run loop's safepoints
NS_NEXT_STRESS_CACHES=1 cargo nextest run --workspace   # vacating every inline cache at every safepoint
NS_NEXT_STRESS_COLLECT=1 cargo nextest run --workspace  # collecting at every safepoint, sliding the site ranges whenever one is free, every reservation given back down to a page above its high mark
NS_NEXT_STRESS_SCAVENGE=1 cargo nextest run --workspace # scavenging at every safepoint after a check of the remembered set
NS_NEXT_STRESS_WIDE=1 cargo nextest run --workspace     # every instruction in its wide form
NS_NEXT_STRESS_SUSPEND=1 cargo nextest run --workspace  # suspending every activation into its frame at safepoints
NS_NEXT_STRESS_JIT=1 cargo nextest run --workspace      # every body compiled at its first entry, and entered at every push even when its machine code exits at once
NS_NEXT_STRESS_EXIT=1 cargo nextest run --workspace     # every poll in machine code -- every backward jump it takes -- taking its exit, as if a safepoint were requested
NS_NEXT_STRESS_ZONE=1 cargo nextest run --workspace     # compacting the code zone at safepoints, every body no activation runs freed (every other compaction keeping each body that has optimized code, slid), the reservations given back as under COLLECT
NS_NEXT_STRESS_OPTIMIZE=1 cargo nextest run --workspace # every body's optimization budget armed at 0, so that the next safepoint compiles its optimized code once its baseline code first spends
NS_NEXT_STRESS_DEOPT=1 cargo nextest run --workspace    # OPTIMIZE, and optimized code leaving at every n-th interrupt point it passes, n drawn afresh at each safepoint
NS_NEXT_STRESS_INVALIDATE=1 cargo nextest run --workspace # OPTIMIZE, and optimized code leaving for a safepoint at every tenth interrupt point it passes, which frees every body's copies -- a body freed often sitting out ever more rounds -- before it installs what was requested since
NS_NEXT_STRESS_PAUSE=1 cargo nextest run --workspace    # stopping as for a debugger's pause at safepoints, the whole stack and every value one level deep read, then continuing
NS_NEXT_STRESS_BREAKPOINTS=1 cargo nextest run --workspace # every statement start of every body found running at a safepoint -- every body whose first entry is interpreted -- patched with a breakpoint that never stops, each gone past through the run loop's arm
NS_NEXT_STRESS_STEP=1 cargo nextest run --workspace     # stopping as for a pause at safepoints and taking a burst of steps into, over and out from there, the shipped filter on and off, the stack read at every stop
NS_NEXT_NO_JIT=1 cargo nextest run --workspace          # the JIT off: every body interpreted, in every build
# and `cargo run -p ns-cli -- run-corpus corpus/modules` under each mode, as
# plain: "ran 6, failed 0, skipped 205" -- every row under both lowerings
# `cargo xtask gates stress` runs the sixteen suites at once, each into its own
# log, `.temp/stress-NO_JIT.log` the last's, in the nextest profile `stress`
# (`.config/nextest.toml`) without `ns-syntax`, `ns-assembler` and
# `ns-memory`, which cannot reach the runtime, the tests that ended a measured
# run alone started first, narrowed by a profile generated from it: every mode
# but WIDE without the tests no switch reaches and `ns-cli`'s
# `debug_metadata`, which boots an interpreter but runs no program, WIDE
# without those of them that compile nothing (below, "The tests a gate
# leaves out")
```

How long the end of a batch takes, each group run alone with `cargo xtask
gates <group>`, its builds included, on the development machine on
2026-10-29: `images` 14 s, `caches` 4 s, `benchmarks` 70 s,
`release-checks` 49 s, `codegen` 61 s (about 6 s of it the shipped-code
step its build needed), and on 2026-11-13 `stress` about 1,050 s --
1,038 s the suite under the fifteen modes it runs and with the JIT off
at once, without the tests no switch reaches (1,233 s with them, 1,642 s
on the whole suite), beside another build on the machine, and
`run-corpus` plain and under each mode a few seconds more -- and on
2026-11-13, in Docker with a warm image, `linux` about three and a half
minutes (the copy 7 s, the suite step 80 s with its build, nextest's run
29 s against 52 s for the whole suite, the doctests 1 s, the Newspeak
benchmark programs 83 s, which the slowest Newspeak benchmarks bound as
before) and `aarch64` about 8 minutes (the suite step 441 s, its build
included, nextest's run under QEMU 377 s; the whole suite's 662 s the
same day), so `end-of-batch` about forty minutes in all. A cold run builds the image
first, about ten minutes on 2026-11-04, nearly all of it the downloads
from the snapshot service (QEMU's build itself 50 s), and compiles the
workspace once per target.

The groups `linux` and `aarch64` run in Docker, in x86_64 containers of
the image `xtask/docker/Dockerfile` describes, so that their environment
is the repository's code: the checkout is mounted read-only and rsynced
into a working copy in a named volume first, so uncommitted work is
checked too: `linux` the suite for `x86_64-unknown-linux-gnu`, System V's
convention and Linux's memory layer, and the Newspeak benchmark programs
(the profile `benchmarks-newspeak`, without their Java and Python
counterparts, which run nothing of ns-next's), and `aarch64` the suite
cross-compiled for `aarch64-unknown-linux-gnu` and run through the image's
own pinned `qemu-aarch64` as the target's runner, the only run of
AArch64's machine code -- both suites without the tests that reach no
platform-dependent code (below), and `linux` the doctests besides, since
those of `Interpreter::enter` boot an interpreter and reserve memory
(`aarch64` runs none: `cargo test --doc` would build them for the host);
what the host needs is `CLAUDE.md`'s, "Working in this
repository", and [execution.md](execution.md), "Tests", has the image and
why its QEMU is the one it is.

### The tests a gate leaves out

A stress switch changes only what the runtime does once it runs, the wide
encoding only what the compiler builds, and a platform only the code that
asks it something, so a test that reaches none of these answers the same
under every mode and in every container, and the stress and Docker groups
leave it out. Which tests those are is measured, never judged by reading
the test, since many reach an interpreter through a helper. In a debug
build run with `NS_NEXT_RECORD_REACH=<directory>`, the first record of
each kind in a process writes a marker there named after the test nextest
says is running (`NEXTEST_BINARY_ID`, `NEXTEST_TEST_NAME`, which a child
`ns-next` inherits; `ns_memory::reach`, re-exported as
`ns_runtime::reach`): `runtime` when the process makes an interpreter
(`Interpreter::with_kernel`) or reads a switch other than WIDE
(`jit::stress_*_from_env`, `JitState::new`'s `NS_NEXT_NO_JIT`),
`compiler` when it reads WIDE (`stress_wide_from_env`, which every body
the compiler builds asks), and `platform` when it calls into the system
through the memory layer (a reservation, the page size, the code zone, the
native stack) or reaches the other code that differs by platform: the
JIT's register roles (`Roles::of`, which every machine code built takes,
mostly the host convention's), whether the build has a JIT, the cut's
return address size, a project's paths (`ns-cli`'s `project.rs`:
`locate_program`, `same_path`, `resolve_written_path`), the start of a
binary through the target's runner (`target_runner.rs`'s
`command_through`), the order a directory lists its files in
(`ns-tooling`'s `collect_ns_files`), the formatter's replacing rename
(`ns-analyzer`'s `fmt.rs`), a URI's path style (`file_uri.rs`) and the
processor check (`ns-tooling`'s `processor.rs`). That the readers stay
wired is held by `crates/ns-runtime/tests/reach_wiring.rs`: every string
naming a switch in a crate's sources stands in a reader that records or
is called only from `with_kernel`, after its record, and an
`Interpreter` is built nowhere else.
Each is one relaxed load after the first, where a body is compiled or
memory reserved, never on the run loop's path, and nothing in a release
build; a marker is written under a temporary name and renamed into
place, so that a killed process leaves none half-written; after
`runtime`, which decides alone, a process writes no further marker, so that most tests leave one file and the record costs the
`commit` group's suite no measurable time.

`cargo xtask unreached-tests` runs the default profile's suite with the
record on and writes `xtask/unreached-tests.txt`: every test that recorded
no `runtime`, under the heading of what it did record, but `ns-memory`'s,
which is the platform layer itself. `cargo xtask nextest-config` appends
three profiles to `.config/nextest.toml` in `target/xtask/nextest.toml`
-- a file, since the filter would pass Windows' 32 KiB command line --
each inheriting its parent's overrides and narrowing its filter:
`stress-reached` (`stress` without every listed test, for every mode but
WIDE), `stress-wide-reached` (`stress` without the listed tests that do
not reach the compiler) and `platform` (`default` without the listed tests
that reach no platform-dependent code, for `linux` and `aarch64`, which
write it in the container). The `commit` group runs its suite with the
record on and fails, naming them, when a listed test now reaches more than
the list says or did not run (a renamed or removed test's entry), and
when a golden trial of a program that runs to status 0 recorded no
`runtime` -- the sentinel that the record still sees the child `ns-next`;
it names, without failing, the tests that reach nothing but are not
listed. A test the list does not name, a new one, runs everywhere.

The list is measured on the development machine, Windows on x64, and the
check runs there too. A test that exists only on another platform
(`#[cfg(unix)]`, say) is in no list and runs everywhere, but a test whose
reach differs by platform -- one that makes an interpreter or reaches
platform-dependent code only on Linux or AArch64 -- is judged by its
reach on Windows, and the Docker groups leave it out if that reach was
none. The platform sites above are named, not proven complete: code that
differs by platform only inside the standard library is not marked.
`debug_metadata` is left out of every mode but WIDE besides: it boots an
interpreter, so the record keeps it, but runs no program, only the boot
every other test runs.

A check that runs over many inputs is split into trials that nextest runs
in parallel, not kept as one test that runs alone at the end of the suite
while the other cores idle: each is a `harness = false` binary on
`libtest-mimic`, as the golden runners are --
`crates/ns-analyzer/tests/corpus.rs` (the analysis over every module and
every cut of one, the kernel in windows of its bytes, the other modules in
groups), `crates/ns-runtime/tests/image_truncations.rs` (every truncation
of three images, in parts) and `crates/ns-cli/tests/image_corruptions.rs`
(the corruption sweep of a built image, in parts). A trial is a process of
its own, so cheap inputs share one. What bounds the suite's wall time is
the longest test that cannot be split: the compilations of a body past the
register ceiling, a few seconds each in a debug build.

The invocations above are Git Bash syntax; the fifteen stress modes do
anything only in a debug build, `NS_NEXT_NO_JIT` in every
build ([memory.md](memory.md), "The move stress mode", "The gates the collector needs", "Generations";
[dispatch.md](dispatch.md), "Invalidation"; [execution.md](execution.md),
"The compact instruction encoding", "Suspending and resuming", "The code
zone", "The optimizing tier's foundation", "Speculation", "Inlining";
[debugger.md](debugger.md), "The pause stress mode", "The breakpoint stress
mode", "The step stress mode"). Under the optimize stress
mode the back-off of a body whose optimized code was abandoned or made
not entrant is void, the budget times 4 to the version still 0, so a
body is optimized again at its next spend until it is refused, one
failure past its last version. Clippy's gate
includes `min_ident_chars` (`clippy.toml`), which refuses identifiers of one
or two characters: names are written out
(`CLAUDE.md`, "Conventions"). It also includes `cast_possible_truncation` and
`cast_sign_loss`, so a narrowing cast outside tests states the bound that
makes it exact, and `allow_attributes_without_reason`, so every lint
exception states its reason.

Never judge performance from a debug build; `cargo build --workspace --release`
is the one to measure, and every binary a measurement compares — the current
tree included — is built with the same explicit `RUSTFLAGS`, which pin
function alignment so that code placement cannot confound the comparison;
the shipped build is unpinned ([execution.md](execution.md), "What removing
the re-entries moved", the protocol). The suite's tests that hold a type to
the size the hot path was measured at run only without debug assertions;
`cargo xtask gates release-checks` runs the suite so, in the profile
`release-checks`, which is `release` without its fat LTO and single codegen
unit -- a type's size does not depend on either, and the build takes well
under a minute instead of minutes. Nothing is measured with it. The same
group lints `ns-runtime` for `aarch64-pc-windows-msvc`, a target without
a JIT, where nothing under `cfg(ns_jit)` is compiled, so that the
interpreter keeps building where the JIT is not -- no other gate builds
that configuration -- and for `aarch64-apple-darwin`, Apple's JIT, which no other gate
builds (`memory` lints its `MAP_JIT` code zone).

`cargo xtask gates codegen` reads the machine code instead of running it:
`cargo asm` (`cargo-show-asm`) writes the listing of `ns-runtime`'s lib in
the `release` profile for `x86_64-pc-windows-msvc`, into `target/codegen`,
and each out-of-line function of the send, push and return path is held to
a budget of instructions and of 16- or 32-byte stack loads over narrower
stores (`xtask/src/codegen.rs`, whose table says which functions and why
each budget is what it is). A function over its budget fails the gate; a
change that passes one for a reason is measured and raises the budget in
the same commit, and a toolchain update re-measures every budget by the
same rule in the commit that bumps the toolchain. The listing is the
rlib's, before the binary's fat LTO, so where LTO inlines across crates
the shipped binary's code can differ; the gate holds the code the crate
itself decides. `cargo xtask codegen <tree>` checks another checkout, an
older commit exported there, against the same budgets.

## Deviations from the language specification

Places where the implementation differs from
`docs/references/newspeak-spec.pdf`. Recorded so a deviation is a decision
rather than an oversight -- and, where a row says so, to keep a defect nobody
has had time to fix from being a surprise.

Most rows only under-reject, over-accept or narrow what can be expressed.
Four do worse: the binary float behind a non-integral literal, the
synchronous asynchronous send, the pattern literal that evaluates to
**nil** and the string literal that is not normalized to NFC each answer a
**correct** program **wrongly, at exit status 0**. Read all four before
trusting a program's output.

| Deviation | Spec | Where it is explained |
|---|---|---|
| Value objects exist and §3.1.1's second bullet is enforced, but nothing copies one across an actor boundary | §3.1.1 | [liveness-and-reflection.md](liveness-and-reflection.md) |
| `///` starts a line comment wherever it stands outside a string or a comment, so a binary selector ends before it: §4.2's `binSel` admits any run of special characters, `///`, `+///` and `<///>` among them, and no such selector can be declared or sent | §4.2 | [frontend.md](frontend.md), "Comments" |
| A non-integral literal is a binary `f64`, not the exact-decimal `FixedPointNumber` the section names | §5.1.1 | [core-library.md](core-library.md) |
| There are no character literals and no `Character` class: `"c"` is a one-character string, as the section itself announces and §5.1.5's `"σ"` form states; a double-quoted string has no escape, and a quote in one is written `"""`, not the section's `""""` | §5.1.4 | [frontend.md](frontend.md) |
| A string literal holds the code points its source text holds, not their Unicode normal form C: a literal written with a decomposed `é` (`e` and a combining accent) has two elements and is not equal to one written precomposed, where the section makes both the same NFC string | §5.1.5 | [frontend.md](frontend.md) |
| A tuple literal is a mutable `Array`, not a shallowly immutable `ReadOnlyTuple`; there is no `ReadOnlyTuple` class. The section itself calls the same state of its implementation a bug; a program that stores into a tuple is not refused | §5.1.7 | [core-library.md](core-library.md), "Decisions" |
| An asynchronous send runs synchronously and answers the value rather than a promise; there are no actors | §5.6 | [frontend.md](frontend.md) |
| An access modifier or `lazy` needs no whitespace after it, where the grammar's `accessModifier` and `lazyModifier` end in `whitespace`: `\| public= 1. \|` is refused like `\| public = 1. \|` rather than declaring a slot named `public` | §6.3.3, §6.3.4 | [frontend.md](frontend.md) |
| A pattern literal parses as §5.1.9's grammar says but evaluates to **nil**, where the section makes `<_>` `Pattern wildcard` and `<l>` `Pattern literal: l`; there is no `Pattern` class | §5.1.9 | [frontend.md](frontend.md) |
| `outer N` with no message after it answers the enclosing object `outer N m` would send to, as canonical Newspeak does, where the section defines `outer N` only as a send's receiver | §5.9 | [dispatch.md](dispatch.md) |
| Mirror-based reflection changes one class at a time, and not every part of it: a `ClassDeclarationBuilder` edits a program's class -- its header's slots, factory and initializer, its methods on either side, its lazy slots -- and installs all its edits atomically, but no mirror changes the class of an individual object or reaches the current activation, several builders' edits are not installed as one, and a superclass change, the nested classes and a class declared from source with no predecessor cannot be edited | §8.2 | [liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors that read", "A MirrorBuilder and an atomic install" |
| A module root whose clauses break the root rule (anything but the seven working combinations of `Object` and `Value`: none, `Object`, `Object new`, `Value`, `Value new`, `Object <: Value`, `Object new <: Value`) is a resolve error for every module, before anything runs, where the section evaluates the clause "as if it was a top level expression" and so lets it fail only when evaluated. A module root is not an ordinary class, and a program must not run when its artifact cannot be built | §6.1 | [object-model.md](object-model.md), "The root rule" |

None of these has a golden recorded as `skipped`: no golden is skipped
today. A deviation that gets one states what the specification requires the
program to answer, so closing the gap is deleting the `skipped` line, and
the expectation written the day the gap was found is what then has to hold.
The asynchronous send could not have one: §5.6 says the result is "a
promise that is immediately returned to the sender", and with no promise to
observe there is no answer to record that would not be invented.

## What lives elsewhere

- **`CLAUDE.md`** — the invariants that are easy to break, each of which has
  already produced a real defect. Load-bearing for anyone working in the
  repository; the reasoning behind each one is in these documents.
- **`docs/PROGRESS.md`** — milestone chronology: what was completed and
  when. These documents describe the design; that one records the
  milestones.
- **`docs/OPEN-WORK.md`** — under "Next", the next piece of work; then
  everything still to be done: planned features,
  specification deviations, open design questions, unexplained measurements
  and known gaps, each with where it is explained.
- **`docs/superpowers/plans/`** — implementation plans, dated. A plan is an
  execution record of finished work and is legitimately historical, so it is not
  a living document and is left as written.
- **`docs/references/`** — the primary sources, with `PROVENANCE.md` recording
  what each one is and why it is here.
