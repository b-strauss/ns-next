# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, bytecode compiler and register VM, and a versioned image format.
Rust 1.98.0, edition 2021, seven crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`;
  the AST is lossless except for layout -- parentheses, literal spellings,
  type annotations as tokens, the prologue, `<-:`, members in source order,
  a message's selector tokens' spans -- and every comment is attached to its node (`attach.rs`;
  `docs/design/frontend.md`, "AST", "Comments"); a doc comment's tags and
  summary are `doc_comment.rs`'s, the grammar the JetBrains plugin's
  `NewspeakDocTags` repeats (`docs/design/editor-support.md`, "Doc
  comments"); `parse` refuses a source
  with errors, `parse_for_editor` answers a recovered module for the editor,
  its recovery bounded by a work budget (`docs/design/frontend.md`,
  "Recovery"); the printer (`printer/`: `print_module` writes a module back
  as source, `format` parses and reprints one) has one layout and no options
  (`docs/design/formatting.md`).
- `crates/ns-runtime` — resolver, object model, dispatch, the compiler (`compiler/`,
  checked against the reference lowering in `compiler/reference.rs`), the
  instructions (`bytecode.rs`, stored and run in the eight-byte encoding of
  `bytecode/compact.rs`; what a callee leaves in a frame's registers is
  `bytecode/leftovers.rs`'s analysis), the VM (`vm.rs`), the baseline JIT
  (`jit/`, on x64 and on AArch64 Linux and Apple, the targets `build.rs`
  sets `cfg(ns_jit)` for, Windows on ARM interpreted; every x86_64
  binary built with `target-cpu=x86-64-v2` (`.cargo/config.toml`), which
  `ns-tooling`'s `processor.rs` checks through CPUID first in each
  binary's `main`, refusing a processor without it; AArch64 run under
  QEMU only and unmeasured: a body compiled at its second entry through
  the one exhaustive `match` over `Instruction` in `jit/baseline.rs`, each
  instruction a template, a leaf of `jit/leaves.rs` or an exit that hands
  the frame to the interpreter, and every compiled body entered wherever
  it runs -- an interpreter's send calls it through the call stub
  (`Mutator::enter_sent` in `interpreter/mod.rs`, `jit::called`, and a
  block's through the closure call stub, `jit::called_closure`), a push
  and a frame returned to cross a bridge whose entry block lasts between
  entries -- unless its code exits at once; an interpreted backward jump
  switches its frame into the body's machine code at the loop's head,
  on-stack replacement, the body compiled at the 40th such jump if it has
  none (`jit::back_edge_ready`, `jit::at_back_edge`), and a frame that
  left its code for a poll's safepoint goes back into it there once the
  safepoint is served (`vm::after_safepoint`); a cached send calls through its site's
  machine-readable cell (`interpreter/inline_cache/cells.rs`) into the
  callee's machine code or a stub the cell names (`jit/sends.rs`,
  `jit/targets.rs`: polymorphic, megamorphic, accessor, mirror method,
  leaf, refusing, and the closure stubs, which enter a block's machine
  code through the machine entry table, `heap/machine_entries.rs`; and
  the row stubs, a `Float` row's -- and a `SmallInteger` receiver's
  mixed row's -- computing its operation bit for bit as the row's leaf
  does, a result outside the `SmallFloat64` range boxed in eden, by the
  code `jit/floats.rs` writes once for them and for optimized code's
  `Node::FloatOperation`, the rows it computes classified by
  `jit/float_operations.rs`, compiled without a JIT too, and continuing at
  the mirror method stub wherever it declines; `execution.md`, "Floats in machine code"), the
  callee returns by `ret`, the miss leaf decides
  what the cell did not and may compile the callee, every body compiles
  lean and its `MakeClosure` transfers the frame into a reifying twin
  the twin leaf compiles on demand (`jit/closures.rs`, with the outer
  locals and the futures), a non-local return is cut by
  `jit/cut.rs`, and an exit at any
  depth of compiled frames materializes them (`jit/materialize.rs`); a
  body that keeps exiting is abandoned, its code freed, and revived when
  the decay of its exit count at every collection, or its interpreted
  entries counted inline, say so; the code zone is compacted, Cog's way,
  when a compile finds no room, and a full collection frees the code of
  dead bodies (`jit/zone.rs`); the
  VM's idioms and the register roles in `jit/macro_assembler.rs`, the
  entry, call, exit, miss and cut stubs in `jit/stubs.rs`, the trigger,
  the entry, which bodies are entered and abandonment in `jit/mod.rs`;
  `execution.md`, "The baseline JIT's foundation", "Sends, returns and
  the exit that materializes", "The entry at a send", "Stage 4: closures,
  non-local return, allocation and the barrier", "The code zone"), the
  optimizing JIT's first three stages beside it (`jit/optimizing/`: a body whose
  budget -- a row per code id in `heap/optimization_budgets.rs`, which
  its baseline code spends through a data word of each copy's -- runs
  out is snapshotted at the next safepoint (`snapshot.rs`), built into
  an SSA graph (`build.rs`), its values kept in machine registers by a
  linear scan between interrupt points (`liveness.rs`, `allocate.rs`,
  `metadata.rs`) and emitted through the baseline's templates
  (`emit.rs`), installed beside its baseline code, entered at a push or
  a compiled call and left for the baseline code wherever a frame goes
  on mid-body, but for a frame that left at a poll's safepoint, which
  goes back in there unless the poll has no entry, the exit was forced
  by a stress mode, or the safepoint rebuilt the stack, reified one of
  its scopes, freed or reinstalled its copy, slid its feedback base or
  left its scopes no room (`vm::after_safepoint`, `jit::resumed_at_poll`;
  `execution.md`, "Inlining", "Back into optimized code at a poll"); a method's body optimized a copy per receiver
  application and entered application (`jit/mod.rs`'s
  `CustomizationKey`, `JitSlot::copies`), entered by a receiver-guarded
  cell, the push and, for a `super`, a private lexical or an
  interpreted send, the body's dispatching entry and the shared
  dispatching stub (`jit/targets.rs`) over the rows of
  `heap/optimized_copies.rs`; its own slots read at fixed offsets, a
  site the caches' executed and trapped bits say never ran a guard that
  traps, a copy that keeps trapping or exiting retired with its version
  raised, past the last compiled without traps, and the copies an edit,
  a bulk forward or a reclaimed id makes stale freed at once; and
  inlining: a plan over a tree of snapshots of the body and its callees
  (`tree.rs`, `plan.rs`), a cached send a type-case of inlined callees,
  accessors, quick methods, `==`, `class`, indexed access and tagged
  `Float` operation nodes (an `Integer` arm whose row also takes a
  `Float` an `IntegerBinary` with its `mixed` operation), each
  inlined scope laid out where the interpreter would push its
  activation, a block compiled with its home and eliminated where
  nothing lets it escape, the exit writing every scope's header and
  making the blocks it finds live (`jit/materialize.rs`), the
  recompilee found by walking up the stack (`jit::find_recompilee`),
  and a copy freed when anything it inlined changes
  (`CopyDependencies`); `execution.md`, "The optimizing tier's
  foundation", "Speculation", "Inlining") and
  the send machinery it calls
  (`interpreter/`: inline caches in `inline_cache.rs` in front of the lookups
  in `resolve.rs`, the one unwinder in `unwind.rs`, the VM mirror in
  `mirror.rs` and its leaves in `builtins/`, a MirrorBuilder's edit
  prepared from its module's current text in `edit.rs` and installed
  atomically in `install.rs`), the debugging API at the run loop's
  safepoint (`debugger/`: `Interpreter::attach_debugger` hands out a
  `DebugRemote`, whose thread pauses the run and reads its stack and
  values at the stop, each frame named, placed and its variables named
  by its body's metadata, which `debugger/metadata/` compiles again from
  the module's current text through either lowerer's collector, keeps
  per body only when the body compiled again is alike
  (`bytecode::compiles_alike`), caches weakly and clears at every
  install; and breakpoints and stepping: a breakpoint patches its slot
  of the body in place, the original kept beside the slots for every
  reader but the run loop's fetch, whose `Breakpoint` arm asks
  `interpreter/breakpoint.rs` and stops at the safepoint, the patched
  body's machine code freed and its method's fast path switched off
  while it stands (`debugger/breakpoints.rs`: line, pending, run to
  cursor and method breakpoints, keyed by body); steps are one-shot
  breakpoints and a request bit held until the next push, a frame left
  otherwise reported by the unwinder and the run loop's cold arms, the
  shipped modules filtered (`debugger/stepping.rs`); exception
  breakpoints are decided by two leaves the kernel's handler search
  sends (`debugger/exceptions.rs`); `debugger.md`, "The debugging API
  at the safepoint", "Compiling the metadata again", "Where the
  original lives", "Step over in recursion and blocks", "Exceptions"),
  the core library
  (`core-library/Kernel.ns`, `PlatformFactory.ns` and, under `platform/`,
  `TranscriptFactory.ns`, `TimeFactory.ns`, `CollectionsFactory.ns`,
  `TypedArraysFactory.ns` and `MirrorsFactory.ns` (the mirrors, whose
  leaves are `builtins/reflection.rs`), every root `= Value`; `Kernel` and
  `PlatformFactory` booted by `interpreter/kernel.rs` and
  `interpreter/core_library.rs`, the other five built on first demand by
  `PlatformFactory.ns`'s lazy slots through two mirror leaves, or by the
  loader when an image names them),
  `LargeInteger` arithmetic (`arbitrary_integer/`, adapted
  from `num-bigint` and `num-integer`), the heap (`heap.rs`, one object space
  of variably sized objects in `ns-memory`'s reservation (`heap/space.rs`),
  a nursery of eden and two survivor spaces below the old space,
  each object its words -- header word, class word, fields, in road step 3's
  layout (`heap/header.rs`, `heap/object.rs`; the seam's value types in
  `heap/records.rs`, the per-heap tables of mixins, applications and compiled
  code in `heap/tables.rs`, whose rows and ids a full collection reclaims
  (`heap/reclaim.rs`, `interpreter/reclaim.rs`; `object-model.md`, "An
  application has two ids")) -- named by handles that are 64-bit byte
  offsets; an activation stack of frames of tagged words in a reservation
  of its own (`heap/frames.rs`), reified on demand into frame objects; a
  Cheney scavenger with age-based tenuring (`heap/scavenge.rs`) beside a
  mark-and-slide collector, which compacts both generations into the old
  space, the mark bit in the header and the forwarding through a live-word
  bitmap (`heap/compaction.rs`), both run only at run-loop safepoints or
  between entries, and both trace only an object's strong words
  (`heap/object.rs`'s `strong_pointer_ranges`), clearing a `WeakArray`'s
  slots (format `WeakPointers`) and mourning an `Ephemeron` (format
  `Ephemeron`) whose key nothing else reaches; the symbols a program
  makes, numberless, in a weak table of their own (`heap/symbols.rs`);
  every table beside the space held to a growth limit that requests a
  full collection, and the reservations beside it -- the stack, the
  cells, the two id tables of machine code, the code zone -- giving back
  what a peak committed after a full collection, by a band and a damping
  (`heap/limits.rs`; `memory.md`, "The trigger and the growth policy",
  "Giving committed memory back"); the write barrier and the
  remembered set in `heap.rs`'s store accessors, and machine code's
  inline barrier beside them; `heap/value.rs`, a value
  as one tagged word, Spur's, and `Word`, the bare word a heap object
  stores; the step state `Mutator` in `interpreter/mod.rs`, held at rest
  by `World` in `heap/world.rs` and reached only through
  `Interpreter::enter`) and the image
  format (`image/`). `shipped_code/` encodes and decodes the shipped modules'
  compiled mixins; `build.rs` embeds the artifact `cargo xtask shipped-code`
  writes (`core-library/shipped-code.bin`, gitignored) with a fingerprint of
  the sources that compile to it, and an optimized boot decodes it -- every
  other boot, and a stale or missing artifact, compiles the shipped source
  (`core-library.md`, "The embedded shipped code").
  `docs/design/README.md`, "The map", says which document covers each part.
- `crates/ns-cli` — the `ns-next` binary, which runs programs: `run`, `run-corpus`,
  `load`, `bench`, `build`. A program is one `.ns` file or a project of several
  modules: a directory whose `newspeak.json` names them, the current directory
  when no program is given (`project.rs`, `program.rs`). The file is described
  by `schemas/newspeak.schema.json`, which `crates/ns-cli/tests/project_schema.rs`
  holds the reader to and the JetBrains plugin bundles.
- `crates/ns-analyzer` — the `ns-next-analyzer` binary, everything that looks at
  Newspeak without running it: `lsp`, a language server for `.ns` files
  (diagnostics, semantic tokens, fold ranges, document symbols, formatting,
  hints and their quick fixes, and a hover with a declaration's signature
  and doc comment);
  `dump-tokens`, which the JetBrains plugin's lexer is checked against;
  `parse`, `parse-corpus`, `disasm` and `inspect`; and `fmt`, the one command
  that writes, which rewrites `.ns` files in the formatter's layout
  (`--check` only lists the files that would change). It never runs a program: `disasm` compiles a module or
  decodes an image, `inspect` reads an image without loading it, and neither runs
  the program's code (the interpreter `disasm` bootstraps instantiates only the
  shipped core library). Its server dependencies
  (`lsp-server`, `lsp-types`) stay in this crate and never reach `ns-runtime` or
  `ns-next`; `serde_json` is shared by `ns-analyzer` and `ns-cli` (the project
  file is JSON) (`docs/design/editor-support.md`). `hints/` holds the hints:
  a local that can take its first value in its declaration, a local that
  can be read-only, each with its fix, and `apply_all`
  (`docs/design/editor-support.md`, "Hints").
- `crates/ns-tooling` — what `ns-next` and `ns-next-analyzer` share: loading and
  resolving a source file, collecting `.ns` files, printing diagnostics, and
  the start check of the processor's x86-64-v2 features (`processor.rs`). It is a crate
  of its own so the analyzer's server dependencies never reach `ns-next`; it depends
  on `ns-syntax` and `ns-runtime` only.
- `crates/ns-memory` — the memory layer (page-range reservation on Windows and
  POSIX), the bump cursor eden's young allocations take their memory from,
  and the JIT's code zone (`code_zone.rs`): one reservation mapped twice,
  an executable view and a writable one its one writer stores through, the
  entry into machine code and the shim through which machine code calls a
  Rust leaf; and the native stack's remaining room, which every entry into
  machine code checks (`native_stack.rs`). The only crate with `unsafe`; `ns-runtime`, `ns-assembler`,
  `ns-tooling` and `ns-analyzer` forbid it.
  The heap's object space is its user (`docs/design/memory.md`, "The memory
  layer", "The object space"), and the JIT (`execution.md`, "Executable
  memory").
- `crates/ns-assembler` — the abstract machine operations the JIT emits,
  double-precision ones among them (SSE2 and SSE4.1's `roundsd` on x64,
  a `FloatRegister` and float conditions with their own flags contract),
  and two encoders, x64 (`x64.rs`) and AArch64 (`aarch64.rs`, which keeps
  `x16`, `x17`, `x18` and `x30` for itself), knowing nothing of the VM:
  labels, branches sized in one pass, a stated contract on the condition
  flags, the call convention's operations (`enter_called` begins every
  address a call reaches, `jump_into_called`, `call_unentered`), every
  encoding constant named and sourced (Conventions, below), every encoding
  checked against `iced-x86` and `yaxpeax-arm`, dev-dependencies only
  (`execution.md`, "The foundation and the one extension point").

`xtask/` — the repository's own tasks, run as `cargo xtask <task>`: the gates
of "The gates" below, grouped, and `shipped-code`, which writes that
artifact (`--check` only says whether it is current).

`editors/jetbrains/` — the JetBrains plugin (Kotlin, Gradle wrapper inside): a
lexer mirroring `ns-syntax`'s, compared against it token by token over every
module (`LexerComparisonTest`), the connection to `ns-next-analyzer` (Reformat
Code included, through the server's formatting), and a structure view of its
own over the server's document symbols.

`benchmarks/are-we-fast-yet/newspeak/` — the fourteen Are We Fast Yet benchmarks,
Primordial Soup's Ton80 Richards and DeltaBlue, their harness and the SOM
collection library they share (`Som.ns`), one
project, with their Java counterparts under
`benchmarks/are-we-fast-yet/java/`, compiled with plain
`javac`, and their Python counterparts under
`benchmarks/are-we-fast-yet/python/`; `benchmarks/micro/` — the
microbenchmarks `ns-next bench` times.
Both are held to the outcomes recorded beside them, the Java and Python
counterparts to the same record, by
`crates/ns-cli/tests/run_benchmarks.rs`, in the nextest profile
`benchmarks-verify` only, which verifies them and measures nothing
(`docs/design/execution.md`, "The Are We Fast Yet benchmarks").

Read `docs/design/README.md` first — it maps the living design documents and says
how much of each aspect exists. `docs/PROGRESS.md` is the milestone chronology.
`docs/OPEN-WORK.md`'s "Next" names the next piece of work, and the rest of it
lists everything still to be done: remove an item when work closes it, add one
when a review, measurement or design leaves something open.

## Invariants that are easy to break

Each has already produced a real defect. The rule is stated here; the mechanism
and its history are in the design document named with it. Read that section
before changing code the rule governs.

**Program outcomes are pinned, not re-derived.** Every `goldens/**/*.ns`
is held to the `status`, `stdout` and `stderr` in the `.toml` beside it, under
both the optimized and the reference lowering, and `run-corpus` likewise. A
change that alters one is a behaviour change: establish that the new output is
right before updating the golden, never because the code now produces it
(`docs/design/README.md`, "The goldens"; `compiler.md`, "The reference lowering").
A new golden's `.toml` is recorded with `NS_NEXT_NO_JIT=1`, from the
interpreter, so that machine code is held to what it never produced.

**`vm.rs` holds no dispatch logic.** Send, `doesNotUnderstand:`, lexical, super
and native dispatch, frame construction, `instantiate` and the inline caches live
in `interpreter/`; the VM's loop only calls them, so the reference lowering and a
later JIT reach the same dispatch and raise the same errors. Never copy dispatch
logic, lookup logic or error text into `vm.rs` or `compiler/`; widen to
`pub(crate)` instead. A send hands `feedback_base`, `family_size` and the
`CallSite` to `interpreter/inline_cache.rs` and does nothing else with them;
`SendMirror`'s arm calls `interpreter::mirror::send_inlined`, which decides the
leaf.

**Compiling an already-resolved module can never raise a `NewspeakError`.** It is
a total, deterministic lowering; a compiler-invariant violation is `panic!` /
`debug_assert!`. A program-level error is deferred to execution
(`Literal::UnresolvedIdentifier`, and `Literal::BodyTooLarge` for a body past
the `u16` register ceiling), so a method never run never fails.

**An expression's value register must still hold that value when its consumer
executes.** A named local's register is live storage; `lower_operand` copies it,
and an assignment through a setter copies its value, whenever something lowered
afterwards can write that register — including when a setter send runs user code
before the very next instruction. Whether something can write it is a question
about the register (`a_send_can_write(register)`), not the send: only an
assignment in the same body (inlined blocks included) and the fallback closure
of an inlinable conditional that assigns the local can. Parameters and immutable
locals are never copied; that rests on `ScopeStack::resolve_setter` and is
checked by `lower_assign`'s `debug_assert!`s against `read_only_locals`. Getting
it wrong is silent wrong output at exit status 0 (`execution.md`, "Fewer
protective copies").

**A register a frame does not read before it writes it is dead, and only
the collector's pre-pass makes it safe to trace.** Neither a push nor a
pop writes a scratch register -- at or above its body's `local_count` --
nor the registers a callee that lay on its sender's slots gives back, so
such a word may be one a callee or a popped frame left. The pre-pass,
which every trace of the stack makes over each frame before it traces
the frame's words (`Heap::trace_stack`), nils every register
`bytecode::leftovers` finds tainted before the program counter the frame
stores; a suspension nils the same in its copy, a divorce every scratch
register. The decoder refuses image code that reads a tainted register,
both lowerings `debug_assert!` it on every body, and a debug build writes
a poison word no value can be into dead registers and refuses an
instruction's read of it. A new instruction declares its register
effects in `bytecode.rs`'s exhaustive matches (`registers_read`,
`register_written`, `jump_target`, `falls_through`, `successors`, and
`Instruction::argument_slots` for a send), or the analysis misjudges
it. A miss is a stale handle traced -- an object since moved or
reclaimed -- or an object nothing reaches kept alive, which a
`WeakArray` shows (`compiler.md`, "What a callee leaves"; `memory.md`,
"What the collector traces").

**An image is untrusted input.** No byte sequence may panic or abort the
process: validate every id, tag, count and length before it indexes, allocates
or slices; bound any recursion the input controls; never add a pre-allocation
not bounded by `bounded_count`. `image::load` verifies structure and reconciles
mixins before any decoded code runs (`image-format.md`, "Verification"). The
decoder sizes every record from its own bytes in a first pass and allocates
nothing before it (`image-format.md`, "Two passes"), then fills each
placeholder in place, so no record is built beside its placeholder. The
decoder allocates its graph only after a tenure between entries: before
the tenure the loader checks every shipped name against the binary's
declarations, refusing an unknown one before it builds anything, then
builds the binary's own modules and classes the image names that the boot
did not -- the binary's code, which the image only selects -- and
resolves each name by table lookup; nothing allocates between the tenure
and the graph pass, which allocates old and never polls a safepoint, so
nothing collects while its placeholders exist and the peak argument stands
(`image-format.md`, "The image under generations"). The
transient peak of a rejected load is about 10.7x of the graph bytes plus ≤4x
of the symbol section in a release build, and about 16x of the graph bytes
plus ≤4x of the symbol section in a debug one, plus ≤6x of the text table
in an image with source in both, plus the leftover analysis's fixed term
of at most 2.02 MiB (3.02 MiB in a debug build), one body at a time and
before the graph pass (`memory.md`, "The
amplification bound of a rejected load", has the breakdown and what is not
counted; `decode_value`'s doc comment the derivation, `texts::decode`'s
the text table's; both count a class
tag at two bytes where every live one is five, so the figures are upper
bounds). The guarantee
covers decoding only: `ns-next load` runs the packaged application's code,
recursion bounded by the 100,000-activation depth limit plus its 1,000 reserve
(`execution.md`, "The depth limit"), and an image that names the shipped
world has the loader build what it names by the binary's own code and look
its names up, running nothing of the image's before the application starts.

**Every store into an existing object goes through `heap.rs`'s store
accessors and their write barrier, or, in machine code, through the
inline barrier (`MacroAssembler::store_with_barrier`), which makes the
same decision and calls `Heap::remember` through a leaf.** A scavenge
reads only the young
objects and the remembered old ones, so a store that writes a word into an
old object any other way leaves the young object it names unreachable to
the next scavenge -- moved or reclaimed, silently, at exit status 0. Only an
initializing store into an object just allocated skips the barrier, the
interpreter's and machine code's alike, which
is why every old allocation outside the image decoder is born remembered;
besides those, only the collector's own rewrites (a scavenge, a
compaction, a move) and the bulk forward behind `become:`
(`Heap::forward_identity`), which keep the remembered set themselves, and
the replacement of resolved futures, which runs only before a full
collection empties the nursery, write words without it.
`NS_NEXT_STRESS_SCAVENGE`'s verification of the remembered set, and a
debug build's poison over the nursery a scavenge vacated, are what find a
breach (`memory.md`, "Generations").

**Every mover realigns an `AlignedBytes` object's data.** A `ByteArray`'s
data begin at the first 32-byte address in its block, an offset computed
from the block's address and never stored, so whoever copies such an
object to another address -- the scavenger's `copy`, the compaction's
slide, the move stress mode's shift -- moves its data to the new block's
alignment (`heap/object.rs`'s `realign`). A mover that skips it leaves the
data at the old offset within the block: wrong bytes at exit status 0. A
new mover, or a new format with data aligned the same way, takes the same
fix-up (`memory.md`, "Every mover realigns a `ByteArray`'s data").

**A wire tag is assigned once and never renumbered or reused.** Every tag lives in
`crates/ns-runtime/src/image/opcodes.rs`, pinned by `tests/opcode_tags.rs`; a new
tag takes the next free number, a removed one retires its number. Renumbering
would let a stale artifact decode into a different, valid program.

**A handle means nothing against a `Heap` that did not mint it, nor across a
move.** An `ObjectReference`, `ClassReference` or `FrameReference` is a 64-bit
byte offset into its heap's object space, an `ActivationReference` the word
offset of a frame on its activation stack. Never hold one across the death of
its interpreter — the offset answers a different, valid object in another
heap. Reach the real `Heap` (a `&Heap`
parameter, or read through `interpreter.enter` while it lives); a reader of
compiled code alone takes the mixin, which holds no handle (`compiler.md`,
"Readers of compiled code take the mixin"). A host keeps a value across a run
only as a `Root`. The debug-only origin guard, move epochs and the stress modes
catch mistakes; a release build does not. A new Rust-side field holding a
handle or a stored `Word` needs its `Trace` impl, and a new pointer word in an
object its format's arm in `heap/object.rs`'s `pointer_ranges`, which marking
and forwarding both read (`memory.md`, "A handle is space-relative", "What
the collector traces", "The move stress mode").

**The brand is the compiler's, and needs no cast.** `World::enter` hands the
`Mutator` to a closure generic over `'gc`, in safe Rust. Never add a public way
to change a handle's brand (`heap::value`'s `pub(super)` `at_rest`/`branded` are
the only conversions; there is no `unbind`), and never give `Mutator` a way to
reach the `Interpreter`. A stored `Word` carries no brand, so `Heap::load` and
`Heap::store` are part of the seam and stay `pub(crate)`. Three `compile_fail`
doctests on `Interpreter::enter` pin it (`memory.md`, "The brand").

**A running activation is an `ActivationReference`; only a `FrameReference` is
ever stored.** An activation's offset is reused after its pop; a closure's
captured frame, a frame object's sender and home, and image records take a
`FrameReference`, the handle of a frame object in the object space
(`Heap::reify`; there is no separate store for frames). The run loop
pops every activation it runs on every exit. **Every instruction that reads or
writes another frame's registers by reference must answer its depth, never 0,
in `bytecode::outer_reach_of`, and its eight-byte form in
`bytecode::compact::narrow_outer_reach`**, or the frame drops its registers on
return and every read answers `nil` at exit status 0. Both matches are
exhaustive, so the compiler makes a new instruction decide; the round-trip
tests' `assert_derived_as_decoded` holds the two to each other
(`execution.md`, "Activations", "Closure conversion").

**An unwind target is a `FrameReference`.** Non-local return, `return:`,
`resume:`, `retry` and the depth reserve name the target's frame; the unwinder
asks `Heap::frame_activation` afresh each time it carries on, because `ensure:`
and `ifCurtailed:` blocks run meanwhile and can reuse the offset. Release builds
do not check generations (`execution.md`, "Unwinding").

**Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` / `{:?}` an
`EvaluationResult`.** Use `Heap::describe` in Newspeak-facing text. `Signal`,
`HostSignal`, `HostError` and `Root` have no `Debug` on purpose, so these do not
compile; tests use a local `expect_ok` helper — copy a neighbour's.

**A class object is per enclosing instance; behaviour is the
`MixinApplication`, shared.** The application cache is keyed by the
superclass's *application*, never by its class object, or every instance mints
its own application (`object-model.md`, "Three layers, not one").

**Compiled code lives on the mixin, and `Instruction::InitializeSlot`'s
`own_index` is mixin-relative**, resolved against the activation's `slot_base`
at run time, because one mixin's code is shared by every application of it;
`vm_initializer.rs` pins it (`compiler.md`). The slot base is that of the
application the activation was *entered* at, never the class object's
current one, and **every slot access whose index was decided without looking
at the receiver checks a recorded guard first** -- the own-slot
instructions, a private or super slot accessor, and a super send's
`doesNotUnderstand:` slot handler by a walk -- or a receiver the bulk
forward replaced is written in the old layout at exit status 0. A guard
is recorded only for a receiver verified to have the level (the push,
and a cache fill, walk for it), and a nested class's hidden cache slot is
looked up again after its superclass clause ran
(`object-model.md`, "What a slot access checks"). A customized copy's
own-slot reads (`Node::OwnSlotLoad`, `End::ReturnUnlessNil`) need no
guard: the copy's entry verified the receiver's application, any
forward of the receiver runs in a mirror leaf machine code exits for,
which ends the frame's optimized code, and the decoder holds every
instance of a program class to its application's slot count
(`execution.md`, "Speculation", "Own slots at fixed offsets";
`image-format.md`, "Verification"). Nor do an inlined callee's slot
accesses: an always-inlined getter's, setter's and lazy getter's
field (`Node::FieldLoad`, `Node::FieldStore`), the one the lookup
resolved for the arm's receiver application -- a private or super
accessor taken without a test included -- and an inlined scope's
own-slot read (`Node::OwnSlotLoad` at the plan's base for that
application), since the type-case's guard, or the scope's known
receiver where no test is taken, verified the object's application,
and the same forward and decoder rules hold (`execution.md`,
"Inlining", "The snapshot tree and the plan"). An inlined scope's
own-slot templates keep their guard, reset at the scope's entry.

**`outer` walks the *lexical* declaration chain, never the receiver's dynamic
class** — `current_class`, then each class object's declaring-class field
(`Heap::class_declaring`). A level a `<:`
clause adds takes its enclosing object and declaring class from the class the
clause names (`interpreter/nested.rs`, `build_applied_levels`). A body-less
declaration's class object is the level its last clause adds, so its own
code -- its full initializer at that class object, and a block created in
it -- starts its walk at the declaration's own enclosing object and
declaring class, which the class object records (`interpreter/sends.rs`,
`lexical_origin`, decided from the code:
`CompiledCode::is_declaration_code`), while the applied mixin's code there
starts at the level's; its `super` starts at the class object's
superclass, as every code's does (§5.10; `object-model.md`, "Mixin
application").

**A `<:` clause's message runs with the applying class's factory arguments, just
before the initializer of the level it adds**: its arguments are lowered into
the applying declaration's full initializer, right before the
`InitializeAppliedLevel` that runs that level's own part (§6.2;
`object-model.md`, "Mixin application"; `compiler.md`, "The three initializer
bodies"; pinned by `goldens/language/class/mixin/mixin_initializer_order.ns`).
A level a `<:` clause added that a program reaches from outside -- through a
mirror -- runs its mixin's standalone body instead, never the clause's
message.

**The VM mirror is a capability.** Whoever holds it runs every leaf, so only the
shipped core-library modules reach it. Six locks keep it there; never loosen one
and never add a way for a program to obtain the mirror, its class or a
`MirrorSend` binding: (1) only `resolver::resolve_kernel` records
`Binding::MirrorSend`, and only the shipped modules are resolved with it; (2)
the mirror slot of `Kernel` is checked at boot, and that of every shipped
module given the mirror (`PlatformFactory`, `TranscriptFactory`,
`TimeFactory`, `TypedArraysFactory`, `MirrorsFactory`) whenever its instance is recorded --
at boot, by `recordShippedModule:` on first demand, or by the loader -- so
no instance that fails it is ever recorded, and no program reaches a
shipped root (the factory pattern), nor the mirror or the kernel instance
as the argument of a send its receiver may not understand; (3) the encoder
names a shipped object by its declaration and refuses one the rule of what
may be named where does not allow (`image/shipped_names.rs`); (4) no
program can name the mirror's class or `Top`: the VM mirror's class has no
declaration and so no name, and the decoder refuses `Top`, `Activation` and
every other shipped name in a position the rule does not allow; (5) the
encoder refuses a graph reaching the mirror, its class, `Top` or their
metaclasses, and the factory body's allocation
(`instantiate::allocate_for_factory`, which `AllocateInstance` runs for a
send and a host's `instantiate` alike) refuses to instantiate the mirror's class;
(6) `SendMirror` has no wire tag (`core-library.md`, "The VM mirror is the
whole Rust boundary"). The bulk-forward leaf
(`elementsOf:forwardIdentityToElementsOf:ifFail:`) refuses the mirror,
every withheld value and every builtin or shipped instance on either side
(`builtins/reflection.rs`'s `may_be_forwarded`), so a forward can never put
the mirror where a program reaches it; never loosen it either. The
MirrorBuilder refuses every edit to a shipped mixin, in `asBuilder` and
again on the Rust side (`interpreter/edit.rs`'s `prepare_edit`), and
resolves an edit with `resolve`, never `resolve_kernel`, so no edit
records a `MirrorSend` binding.

**A cache records what the lookup decided; it never decides.** An inline-cache
hit is `act` on the resolution `resolve_*` answered for the same key; an entry
holds no handle. Anything the key does not cover is read afresh on every
send — today a primitive's payload check, in `act`, and the class object's
builtin field (`Heap::class_builtin`), which the factory body's
`AllocateInstance` reads on every instantiation; a method's `MethodFastPath`
is read off the entry's method on every hit, by the interpreter's `act`
and machine code's stub alike -- a method whose fast path only the
debugger changes, switching it off while a breakpoint stands in the body,
and invalidating every cache and every optimized copy when it does
(`MixinMethod::switch_fast_path`; `debugger.md`, "A breakpoint in a
method with a fast path"). Add such a read and its per-send re-check goes with
it. A lazy slot's getter has no fast path (`Instruction::ReturnSlotIfFilled`
instead). The reference lowering runs every send uncached and is the oracle
(`dispatch.md`, "Primitive methods", "3b — The caches").

**Machine code is a lowering of the interpreter's instructions, never a
second interpreter.** Four rules keep the baseline JIT
(`crates/ns-runtime/src/jit/`) exact. A template (`baseline.rs`) stores
into the frame only after its last decline or exit, so an exit, which
hands the instruction to the interpreter, never re-applies it. A leaf
(`leaves.rs`) runs no Newspeak code, cannot collect, reads no other
activation and never pushes; only the miss leaf, the initializer leaf
and the forcing leaf compile and install a callee, and the twin leaf
the running body's twin -- the budget leaf only parks or re-arms its body's
budget and answers whether an optimization is due, whose exit
(`ExitKind::BudgetSpent`) the run loop takes to walk up the stack for
the frame to request (`jit::find_recompilee`) and record its request
-- under the optimize stress mode the leaf records the spending
frame's own request -- which the run loop's safepoint compiles and
installs (`jit::optimize_requested`), or points its own body's budget
word and dispatching word, the latter at its code id's block of
dispatching rows (`jit::point_budget_word`); and a leaf answers "re-execute" only
before it changed anything but a send's argument slots, which the mirror
leaf places as the interpreter does and which nothing reads after the
send but the send re-executed. Every body compiles lean, and its
`MakeClosure` transfers the frame into the body's reifying twin, so a
lean body's frame is never reified: whether a frame reifies is decided
by the copy it runs (`MachineFrame::reifies`), the run loop goes on in
the twin with a frame the interpreter reified, and no leaf reifies a
frame running lean code, whose return divorces nothing and whose frame
object would stay married to a popped offset; a debug build asserts it
as the leaf's view is released. A twin is freed, abandoned and replaced
with its lean body, never alone, and the lean body's transfer words are
written afresh wherever the twin moves (`execution.md`, "A frame reifies
only in its body's twin").
The cut (`cut.rs`), which carries out a non-local return in machine
code, is the one routine that reads and pops activations other than
its own: it runs no Newspeak code, never collects, never pushes, reads
only the compiled frames of the current stretch and pops only those
above the home and a home machine code pushed -- each as its return pops
it, a reified one divorced and its count taken off -- while a home that
is the stretch's entry frame it divorces and uncounts as that frame's
return does, and returns through the entry stub, whose run loop pops
it. It changes nothing before it has found the home among them, and
answers the exit -- so that the interpreter's one unwinder carries the
return -- for a body that is no block's, a block that is the entry
frame, a home not running, suspended or below the entry frame, an
unwind marker's activation between, a return address into a stub
before the home, a home that catches no return, and a frame of a
method a method breakpoint names, whose exit only the unwinder reports.
A slot a breakpoint patched lowers as an exit, whatever its instruction,
so that the interpreter's `Breakpoint` arm stops there; patching or
restoring a slot frees the body's machine code and every optimized copy
that inlined it first (`jit::free_for_patch`), and the optimizing tier
neither optimizes nor inlines a patched body (`debugger.md`, "Where the
original lives").
A new instruction is one arm of `jit::baseline::lower` plus the
macro-assembler idioms it lacks, never a second dispatch, and optimized
code (`jit/optimizing/emit.rs`) emits every instruction it does not
lower as a simple node -- register arithmetic that emits no call --
through the very template function the baseline's arm for it calls,
after writing back into the frame every value it holds that the frame
word does not hold yet, so that at every interrupt point the frame is
the interpreter's word for word -- every scope's, an inlined scope's
receiver and current class among its bindings, the current class
computed only before the scope's frame nodes and by the exit's walk
before it writes the scope's header; it leaves no store out, its code is
entered only at its prologue, or just past it at a push, or at the
entry of the poll a frame left it at, once the safepoint is served,
where the entry restores every value the code past the poll reads
from a frame word the frame kept or a constant's (`metadata::poll_restores`,
which the emitter and the replay share) and a poll it cannot restore has no
entry; never otherwise mid-body, and a frame that goes on mid-body goes on in the baseline code (`execution.md`, "The optimizing tier's
foundation", "Inlining"); the run loop
enters machine code through `jit::entered` and `jit::resumed`, a send it
delivers through `jit::called` and `jit::called_closure` (`mod.rs`);
the entry block's lasting words -- what does not move between entries --
are written only when what they name changes, which must mark the block
stale (`JitState::mark_block_stale`; only a debug build's check at every
entry catches a missed mark), the frame's and the code's words at each
entry, and it holds no handle. A cache cell is written only from its
entry -- beside it, or afresh when the JIT's state under it changes --
its entry address a function of the entry -- its guard and its target
-- and the JIT's state,
and a stub does what `Mutator::act_with_base` does for its target and
nothing more. A row of the machine entry table, which the closure stubs
jump through (`heap/machine_entries.rs`), is written only from the
body its code id names, and cleared wherever that stops naming its
machine code: the id freed, the body abandoned or freed by a
compaction or a slide of its feedback base, the zone replaced, the
JIT switched on or off -- a stale row is a jump into another body's
code. A method body's dispatching rows (`heap/optimized_copies.rs`),
which the dispatching stub jumps through, are written only from the
copies of the body whose dispatching word names them
(`JitState::write_dispatching_rows`), each row from the copy its key
names, written afresh at every install and wherever a copy stops
naming its machine code -- retired, invalidated, freed or moved by a
compaction -- and cleared when the zone is replaced; a stale row is a
jump into another copy's code.
The code zone is compacted only at the run loop's safepoint, when no
return address into it exists. A compaction remaps every cell, machine
entry row, dispatching row and record that names an address in it, and a body holds no
address of its own but those relative to `rip`.
No word machine code reads or writes between two leaf calls lives inline
in `Mutator`: frame registers and object fields lie in the activation
stack's and the object space's reservations, the cells, the
application id table, the machine entry table and the optimization
budget rows in reservations of their own, a body's closure and
outer-local sites among its data words in the code zone, moved and
freed with it and written only through the zone's writer
(`JitState::write_body_words`), and each copy's budget word, the
address of its body's budget row, among them, written through the same
writer by `JitState::write_budget_word` -- once the code is installed,
afresh after every full collection, which may free the code id the row
is indexed by (`jit::point_budget_words_after_collection`), or the word
names another body's row, and by the budget leaf when the word named no
row of its body's (`jit::budget_spent`) -- all outside it, and
every other such word lives in the heap's machine words
(`heap/machine_words.rs`, atomics behind an `Arc`, which the debugger's
agent arms from its own thread, every hot access to them `Relaxed`) or
the entry block (`ns_memory::EntryBlock`, `Cell`s behind an `Rc`).
`CodeZone::enter` holds the mutator as a
protected `&mut`, so machine code's access to a word inside it is
undefined behaviour once a leaf has written that word, which no test and
no tool detects (`execution.md`, "The baseline JIT's foundation", "The
machine words"). No file under `jit/` but `macro_assembler.rs` names an
architecture's registers or its encoder (`x64`, `aarch64`) outside a test
module: a template, a send or a stub uses the macro assembler's idioms
and its `Roles`, and a new register need is a new named role, held by
`crates/ns-runtime/tests/jit_architecture_split.rs`. Every address a
call reaches begins with `enter_called`, which on AArch64 stores the
return address in a 16-byte slot on `sp`; a stub's tail jump into one is
`jump_into_called`, and a call into a body's middle -- the entry at a
push, an on-stack replacement, a resumption -- is `call_unentered`, or the
return address is saved twice or not at all and every walk of the native
stack reads the wrong frame, invisibly on x64, where all three are the
plain `call` and `jmp` (`execution.md`, "The native stack carries the
return addresses").

**A chain of messages is walked link by link.** `a b + c …` is a left-nested
`Send` as deep as the chain is long, and every chain length is valid
Newspeak, so no depth guard bounds it: never recurse along a receiver —
walk `Send::receiver_chain`; `Send` drops itself iteratively. The compiler
still recurses (`OPEN-WORK.md`) (`frontend.md`, "Recovery", rule 7).

## Working in this repository

The suites run under `cargo-nextest` (`cargo install cargo-nextest --locked`).
The gates need, beyond that, the rustup targets of `memory`
(`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`) and `release-checks`
(`aarch64-pc-windows-msvc`, `aarch64-apple-darwin`), `cargo-show-asm` for
`codegen` and a JDK and a `python` on `PATH` for `benchmarks`. `linux` and `aarch64` need Docker
(Docker Desktop, or a Docker engine on x86_64); everything else they need,
QEMU included, is the pinned image `xtask/docker/Dockerfile` describes,
which the groups build themselves, so nothing is set up by hand.

### The gates

This table is the one place that says which gate runs when;
`docs/design/README.md`, "Gates", follows it. Each row is a group of
`cargo xtask gates <group>` (`xtask/src/main.rs`), which says what the
group runs; several groups run in one call, one after another, and the
first that fails stops it. Every `cargo xtask gates` call runs
`cargo xtask shipped-code` once, which writes the embedded shipped code
(`crates/ns-runtime/core-library/shipped-code.bin`, gitignored;
`docs/design/core-library.md`, "The embedded shipped code") and refuses,
failing the call, under `NS_NEXT_NO_PRIMITIVE_METHODS=1`: right after
`cargo fmt --all` in the `commit` group, which is the only step that
rewrites sources, and otherwise before the first group. "Every commit" is
every commit that changes more than Markdown (check `git diff --stat`): a
change to Rust, a test, a fixture, a golden, a `.ns` or a `Cargo.toml` runs
its gates however small. The nextest profiles are `.config/nextest.toml`'s;
every one runs on 18 threads.

| Group | Runs | When |
|---|---|---|
| `commit` | `cargo fmt --all`; `cargo clippy --workspace --all-targets`, warning-free; `cargo nextest run --workspace`, the default profile, every golden under both lowerings; `cargo test --workspace --doc`, since nextest runs no doctest; `parse-corpus corpus/modules`, "parsed 211, failed 0"; the plugin's `gradlew --no-daemon check`, whose tests Gradle reruns only when the plugin, the analyzer binary or a `.ns` its lexer comparison reads (`corpus/modules`, `goldens/`, the core library, `benchmarks/`) changed, up to date in a few seconds otherwise, most of them the start of Gradle's JVM, since no daemon outlives a gate run | every commit |
| `analyzer` | the hints' behaviour oracle, profile `hints` | a commit touching `crates/ns-analyzer` |
| `images` | every golden through `build` then `load`, profile `images` | a commit touching the image format (`crates/ns-runtime/src/image/`), the instruction encoding (`crates/ns-runtime/src/bytecode.rs`, `bytecode/`) or `ns-next build`/`load`; the end of a batch that changed the runtime |
| `caches` | every golden with a two-entry global cache, profile `caches` | a commit touching the caches (`crates/ns-runtime/src/interpreter/inline_cache.rs`); the end of a batch that changed the runtime |
| `memory` | `ns-memory`'s clippy for `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` | a commit touching `crates/ns-memory` |
| `benchmarks` | the benchmark programs verified at full size, and their Java and Python counterparts; needs a JDK (`JAVA_HOME` or `PATH`) and a `python` on `PATH`; profile `benchmarks-verify`; it measures nothing and never runs under a stress mode (`execution.md`, "The Are We Fast Yet benchmarks") | when benchmarking; the end of a batch that changed the runtime |
| `release-checks` | the suite built without debug assertions, profile `release-checks` of `Cargo.toml` (release without its fat LTO), which runs the size tests (below); `ns-runtime`'s clippy, warning-free, for `aarch64-pc-windows-msvc`, a target without a JIT, where no `cfg(ns_jit)` code is compiled, and for `aarch64-apple-darwin`, Apple's JIT (`rustup target add` both) | when a change touches a type a size test pins; the end of a batch that changed the runtime |
| `stress` | the suite under the fifteen stress modes and once with `NS_NEXT_NO_JIT=1`, all at once, each into `.temp/stress-<MODE>.log` (`NO_JIT` the last's), then `run-corpus` plain and under each mode (`docs/design/README.md`, "Gates") | the end of a batch that changed the runtime |
| `codegen` | `cargo asm` (`cargo install cargo-show-asm --locked`) over `ns-runtime`'s lib in the `release` profile for `x86_64-pc-windows-msvc`, into `target/codegen`: each hot function of the send, push and return path held to a budget of instructions and of wide stack loads over narrower stores (`xtask/src/codegen.rs`; `execution.md`, "Keeping the hot path in registers") | a commit touching `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs` (under `crates/ns-runtime/src/`); the end of a batch that changed the runtime |
| `linux` | in Docker, in an x86_64 container of the image `ns-next-gates` (`xtask/docker/Dockerfile`, built first, cached by Docker), the checkout mounted read-only and rsynced into a working-copy volume of its own, `ns-next-gates-<hash of the checkout's path>-work` (what a `.gitignore` names and `prior-art` excluded; `target` and cargo's registry volumes of their own, so a second run builds incrementally and two checkouts never share one; a linked worktree's git metadata mounted read-only instead of copied): `cargo xtask shipped-code`, `cargo clippy --workspace --all-targets` warning-free, the default suite on as many threads as the container has CPUs, the doctests, and the benchmark programs with their Java and Python counterparts (profile `benchmarks-verify`, as `benchmarks` runs them; the image has a JDK and Python), all for `x86_64-unknown-linux-gnu` | the end of a batch that changed the runtime |
| `aarch64` | in Docker, the same image and copy, then the shipped code, clippy for `aarch64-unknown-linux-gnu` warning-free, and the suite cross-compiled for it, run through the image's own `qemu-aarch64`, a pinned QEMU release, as the target's runner, which also starts every `ns-next` and `ns-next-analyzer` a test starts (`aarch64-linux-gnu-gcc` the linker, `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER=qemu-aarch64`, `QEMU_LD_PREFIX=/usr/aarch64-linux-gnu`, `RUST_MIN_STACK=8388608`), no doctests (`docs/design/execution.md`, "Tests"); about 8.5 minutes | the end of a batch that changed the runtime |
| `plugin-verification` | `gradlew --no-daemon verifyPlugin buildPlugin`, which downloads several IDEs | the end of a batch that changed `editors/jetbrains/` |

`end-of-batch` is `images`, `caches`, `benchmarks`, `release-checks`, `codegen`,
`stress`, `linux` and `aarch64` together, the two Docker groups last; neither runs
at a commit, nor under a stress mode.
"The runtime" is `crates/ns-runtime`, `ns-syntax`, `ns-tooling`, `ns-cli` or
`ns-memory`; a batch that changed only `crates/ns-analyzer`, the plugin or
Markdown runs no stress mode, no `run-corpus` and no benchmark programs, since
the stress modes test the runtime's behaviour and cannot reach the analyzer
otherwise than the plain suite does. A commit that never saw the gates of the
end of its batch is work in progress, not finished work; a subagent
implementing part of a batch is told this staggering explicitly.

A change to `crates/ns-syntax/src/lexer.rs` is mirrored in
`editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt`
in the same commit. Nothing under `crates/ns-syntax` should change for runtime work.

Never judge performance from a debug build (the `measuring-performance` skill).
Only a build without debug assertions runs the size tests
(`a_handle_and_an_optional_handle_are_one_word`,
`a_resolution_stays_the_size_the_hot_path_was_measured_at`,
`a_signal_fits_in_the_niche_of_its_one_unboxed_error` and their
neighbours): `cargo xtask gates release-checks`, which builds the profile
`release-checks` in well under a minute where `--release`'s fat LTO takes
minutes. A type's size does not depend on LTO, so the profile is never
used to measure.

Every corpus module must parse; there is no list of exceptions. A module that
runs end to end belongs in `corpus/runnable.toml` with the unary selector its
generated application sends it (`send`) and its output. `PlatformFactory.ns`,
`TranscriptFactory.ns` and `TimeFactory.ns` are widened only on demand: an accessor exists because a `runnable.toml`
module or a program under `benchmarks/` needs it, named in the accessor's
comment; the kernel facet (`platform kernel`), the typed arrays facet
(`platform typedArrays`) and the mirrors facet (`platform mirrors`), which
only hand out the classes of `Kernel.ns`, `TypedArraysFactory.ns` and
`MirrorsFactory.ns`, are exempt. `Kernel.ns` and `CollectionsFactory.ns` take Primordial Soup's whole
core API instead, its selectors and behaviour adopted, every body written
here and no other project named in their comments (`core-library.md`,
"The kernel completed to Primordial Soup's core API").

Add no new external dependency without a reason that survives review — the image
format is hand-rolled on purpose.

**Search the whole specification before concluding it is silent.**
`docs/references/newspeak-spec.md` transcribes `newspeak-spec.pdf`; a rule often
sits in a section whose heading looks unrelated. Cite the PDF. The older papers
are background only; where the specification is silent, the canonical
implementation decides (`docs/references/PROVENANCE.md`).

**Design decisions follow the principles in `docs/design/README.md`**: a
deviation from the specification is a defect; no valid construct is carved out
because it is rare; performance decides among semantics-preserving options,
judged by the finished system; a guarantee is made by arranging Newspeak classes
before adding a runtime rule; `unsafe` behind a safe abstraction is acceptable.
Ground each decision in `prior-art/` (the `making-design-decisions` skill;
`prior-art/PROVENANCE.md`). Copying code out of `prior-art/` is a licensing
decision, made so far only for `num-bigint` and `num-integer`
(`arbitrary_integer/`), SOMns and Primordial Soup (`benchmarks/are-we-fast-yet/newspeak/`),
and Are We Fast Yet (`benchmarks/are-we-fast-yet/java/`).

## Conventions

- All code, identifiers, comments, commit messages and developer documentation
  are in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Newspeak sources (`.ns`) outside `corpus/` and `prior-art/` are formatted
  by `ns-next-analyzer fmt` (two spaces, `docs/design/formatting.md`) and
  commented with `///` lines, never `(* … *)` (but in
  `goldens/language/syntax/line_comment.ns`, which pins both forms).
  `crates/ns-analyzer/tests/formatting_gate.rs` holds the layout of every
  `.ns` under `goldens/`, `benchmarks/` and `crates/ns-runtime/core-library/`
  (the directories it walks), and `crates/ns-cli/tests/source_formatting.rs`
  the `///`; `corpus/` stays upstream's verbatim copy. After editing a `.ns`
  file run `cargo run -p ns-analyzer -- fmt <paths>`, naming files or those
  directories: `fmt` formats exactly what it is given, so at the repository
  root it would rewrite `corpus/` and `prior-art/` too.
- **Names are written out.** No identifier of one or two characters and no
  truncated word, in every kind of name and every segment of a compound:
  `mutator`, `index`, `error`, `interpreter` (not `interp`), `program_counter`
  (not `pc`), `arguments`, `argument_count` (not `num_args`), `ObjectReference`
  (not `ObjRef`). Exceptions: type parameters and lifetimes (`T`, `'a`, `'gc`);
  the words `ok`, `id`, `min`, `max`, `temp`, `And`, `Or`; names the workspace
  does not declare (`str`, `len()`, `fmt`, `Rc`, `Err`); and names bound to a
  file or interface (`vm`, `disasm`, `g1`/`g2`). Clippy's `min_ident_chars`
  enforces the short ones. Code samples in plans follow the same rule.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`).
- Design documents in `docs/design/` are living: update the document for the
  aspect rather than adding a dated one. Plans live in
  `docs/superpowers/plans/` as `YYYY-MM-DD-HHMM-<name>.md` and are left as
  written once complete.
- Documentation other than a plan describes the current state — `PROGRESS.md`
  and `OPEN-WORK.md` included. A statement no longer true is deleted or
  rewritten, never labelled historical. A reason still true stays.
- Before a branch is merged, a fresh session must be able to start from the
  documentation alone (the `finishing-a-batch` skill checks it).
- A Newspeak module inline in a test is an `indoc! {"…"}` (or `formatdoc!`),
  indented with spaces with the Rust around it, the closing `"}` on its own
  line — not a one-line literal of `\n` escapes.
- Comments explain the decision — what a check prevents — not the mechanics.
- **Every member of a shipped core-library module is documented** to the
  standard of Javadoc. Every method, slot and class of `Kernel.ns`,
  `Manifest.ns`, `PlatformFactory.ns`, `TranscriptFactory.ns`,
  `TimeFactory.ns`, `CollectionsFactory.ns`, `TypedArraysFactory.ns` and
  `MirrorsFactory.ns` -- public, protected and private -- has a doc
  comment directly above it, a block of two or more `///` lines
  (`docs/design/editor-support.md`, "Doc comments"): a description of
  what it does and what it guarantees about order, identity, copying and
  mutation, as long as that takes, then its tags -- a `@param` for each
  argument, in the pattern's order, saying what it must be; for a method
  one `@answers`, what it answers; a `@signals` for each Error class it
  signals, saying when; and a `@see` for a related member worth naming. A
  class whose header names its factory has one `@factory`, what the
  factory does, and its `@param`s are the factory's. A section that would
  say nothing is left out. It names no other implementation. A comment
  that explains a decision, as the rule above asks, goes after it.
  `crates/ns-runtime/tests/core_library_documentation.rs` holds every
  member to having a doc comment, its `@param`s, its `@answers` and its
  `@factory`. A comment is part of the shipped source, so editing one
  outside `Manifest.ns` changes the fingerprint in
  `crates/ns-runtime/tests/fixtures/golden-image.bin`, which
  `regenerate_the_golden_fixture` (`tests/image.rs`) rewrites in the same
  commit (`core-library.md`, "The embedded shipped code").
- **Every encoding constant of `ns-assembler` is a named `const` documented with its
  official source** -- the Intel SDM volume and section title for x64, the
  support.arm.com page of the instruction (DDI0602) for an AArch64 instruction,
  the section of the AAPCS64 in ARM-software/abi-aa on GitHub for the calling
  convention; a link points at the concrete page or section, never at a
  document -- and no integer literal other than 0 and 1 stands outside a
  `const`; held, for every encoder file under `src/`, by
  `crates/ns-assembler/tests/encoding_sources.rs`.
- **A web link is never wrapped or shortened for line width.** A line
  holding a link may exceed the width, in code comments and documents
  alike: a URL cut across lines or abbreviated points nowhere.
- **Object, frame and activation access goes through
  `crates/ns-runtime/src/heap.rs`.** The handles' inner index is private to the
  `heap` module (`heap.rs` and its submodules, `heap/value.rs` among them, which
  puts the index into a word); add a missing accessor in `heap.rs`. Accessors
  copy out rather than lend out (`memory.md`, "The seam").
