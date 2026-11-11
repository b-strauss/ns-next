# The debugger

> **Status:** planned; nothing of the debugger itself is built. Its design
> rests on nine decisions made with the user on 2026-11-10 ("Planned: the
> decisions"), on the mechanisms they imply ("Planned: the mechanisms"), and
> leaves the questions in "Open questions" to be decided. It builds on
> groundwork that exists: reified activations and the activation stack,
> suspension into frames, the statement→program-counter table, stable
> call-site numbers, retained source, the unwinder's frame targets, the
> JIT's materialization and invalidation, the safepoint poll, and the
> MirrorBuilder's atomic install, each described in the document for its own
> aspect ("What exists").
> **Code:** none yet. The groundwork is in `crates/ns-runtime/src/bytecode.rs`
> (`CompiledCode::statement_program_counter`, `CallSite`),
> `bytecode/compact.rs` (`EncodedBody`), `bytecode/leftovers.rs`, `vm.rs`
> (the run loop, its safepoint, `resume_caller`, `send_destination`),
> `heap/machine_words.rs` (the poll words), `heap.rs`
> (`Heap::synchronize_depth_limit`), `types.rs` (`Continuation`,
> `SourceOrigin`), `interpreter/unwind.rs`, `interpreter/install.rs`,
> `jit/materialize.rs`, `jit/mod.rs` (`abandon`, `invalidate_applications`,
> `CopyDependencies`), `core-library/Kernel.ns` (the exceptions) and
> `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/`
> (the run configurations).

This document is the debugger's: what it has to do, how the VM supports it,
the protocol between `ns-next` and an IDE, and the JetBrains plugin's side.
The requirements it serves -- edit-and-continue, one reflective core over
the interpreter's state, the interpreter's frames as the one truth a JIT
must always be able to reconstruct -- are stated in
[liveness-and-reflection.md](liveness-and-reflection.md), "Requirements"
and "The unifying principle", and bind it. The specification asks that a
programming environment "must always allow developers to retain
activations during debugging so that accurate and complete stack traces can
be maintained" (§3.6, `newspeak-spec.pdf` p. 17), and of
mirrors that they are "capabilities for reflection" which give access to
the current activation (§8.2, p. 47).

## What it has to do

The target is the Java debugger in IntelliJ IDEA, feature for feature, plus
what Newspeak's liveness requires beyond it.

**Breakpoints**

- line breakpoints, with a variant per block body on the line (IntelliJ's
  lambda breakpoints: "line", "block", "all");
- conditions, hit counts, logpoints ("evaluate and log", no stop),
  temporary breakpoints (removed once hit), "disable until another
  breakpoint is hit", and instance and caller filters;
- method breakpoints: stop at a method's entry and at each of its returns;
- exception breakpoints, for caught and for uncaught exceptions, filtered
  by class; `halt` stops;
- a breakpoint set before the code it names exists -- a module not built
  yet, a shipped module built on first demand -- is pending and takes
  effect when the code is built;
- breakpoints in the core library's modules, which the binary embeds
  rather than ships as files.

**Running and stepping**

- launching a program under the debugger, stopping before `main:args:`
  runs if asked (`stopOnEntry`), pause, resume, terminate and disconnect;
- step over, step into, step out, smart step into (choose which send on the
  line to step into), run to cursor, and the time a step took, shown
  inline;
- step filters: by default, step into does not stop in the shipped
  modules' code, so stepping into `coll do: [:each | …]` lands in the
  block, not in `Array>>do:` (open, "Open questions").

**Inspecting**

- the stack of the stopped program, each frame's source position and the
  send it stands at, frames below the top included;
- receiver, arguments, locals, slots and the elements of collections,
  lazily and paged; labels as a Newspeak programmer reads them;
- marking an object and naming it in later evaluations (open, "Open
  questions");
- the value the last call returned, shown after a step over or a step out;
- the memory view: the instances of a class and the objects that refer to
  one (open, "Open questions").

**Changing**

- evaluating an expression in a frame's scope, watches, and setting a
  variable;
- restart frame and force return;
- hot swap of changed methods, and beyond Java's RedefineClasses -- which
  permits only method bodies to change and leaves running frames in the
  old code (`jdk/src/hotspot/share/prims/jvmtiRedefineClasses.cpp:954-1063`,
  `:3873-3934`) -- edit-and-continue of an activation that is running,
  with added locals, as [liveness-and-reflection.md](liveness-and-reflection.md),
  requirement 2, asks;
- for an uncaught `MessageNotUnderstood`, "Create Method": the method is
  created and the message sent again
  ([core-library.md](core-library.md), "Exceptions", the paragraph on
  `defaultAction`).

## What exists

- **Activations** are frames of tagged words on an activation stack of their
  own, walkable without the Rust stack, reified on demand into frame objects
  and suspended into them and resumed lazily ([execution.md](execution.md),
  "Activations", "Suspending and resuming").
- **`statement_program_counter`** maps every statement of a body to its
  instruction range (`bytecode.rs:532`; [compiler.md](compiler.md),
  "Safepoints and call-site identity"); an inlined block's statements are
  part of their enclosing statement's range. A body carries one `source`
  span; there is no table of local names.
- **A send's `CallSite`** is its stable number within its code family
  (`bytecode.rs:99`).
- **A caller's stored `program_counter` names its send.** While a callee
  runs, the caller's `program_counter` is the index of the send that called
  it; `vm::resume_caller` reads that slot again for the register the value
  goes to (`send_destination`, which is `unreachable!` for any slot but a
  send's, a `ResolveFuture`'s, an initializer invocation's
  (`InitializeInstance`, `InitializeSuperclass`, `InitializeAppliedLevel`)
  or a `SendMirror`'s) and goes on at the next index ([execution.md](execution.md),
  "Where `program_counter` is stored").
- **The unwinder** carries any unwind to a `FrameReference` target, with
  `ensure:` and `ifCurtailed:` blocks run on the way
  ([execution.md](execution.md), "Unwinding"); its `Restarted` outcome, for
  `retry`, sets a frame's `program_counter` back to 0 and leaves its
  registers as they are (`interpreter/unwind.rs:34-38`); it is written for
  an `on:do:` activation `retry` asks to run again, so restarting any frame
  widens what it accepts.
- **`Continuation`** has the variants `Deliver`, `NestedClass`, `Unwind`,
  `VmErrorSignalled`, `Reexecute` and `ReturnToSuspended` (`types.rs:407-435`):
  what a frame's return does besides delivering its value.
- **The JIT** materializes every compiled frame of its stretch into the
  interpreter's frames at an exit (`jit/materialize.rs`); a frame running
  machine code is the interpreter's word for word at every interrupt
  point, with one exception: a poll's exit in optimized code is light. It
  writes the inlined scopes' headers but defers making the blocks the copy
  eliminated (`materialize::defer_the_poll_s_blocks`), and once the
  safepoint is served the frame goes back into its optimized code at the
  poll (`vm::after_safepoint`, `jit::resumed_at_poll`), restoring what the
  code past the poll reads from frame words or from the code's constants
  (`metadata::poll_restores`) and never looking at the frame's compiled
  mark; the debug-only `jit::deoptimize_stopped_at_poll` makes the deferred
  blocks and sends the frame on in baseline code instead
  ([execution.md](execution.md), "Back into optimized code at a poll").
  Freeing a body's machine code is safe for frames stopped in it,
  which go on interpreted (`jit/mod.rs`, `abandon`). An install frees
  every optimized copy that relied on an application it rebuilds
  (`invalidate_applications`, keyed by dispatch id); each copy records the
  code ids it inlined (`CopyDependencies::code_ids`), today so that a full
  collection keeps those ids alive.
- **The safepoint poll**: the interpreter polls at every push, every
  caller resumed and every backward jump, one compare of the request word;
  machine code polls at each loop's head, and at a compiled push through
  the prologue's compare of the depth with `DepthLimit`, which a real
  request (a bit of `LOWERS_DEPTH_LIMIT`: the VM's, the debugger's pause
  or commands), never a stress mode's hold (`STRESS_HOLD`), sets to 0
  (`Heap::synchronize_depth_limit`; [memory.md](memory.md),
  "Where a collection can happen"; [execution.md](execution.md), "Polls
  and the depth check"). A compiled `ret` into a compiled caller does not
  poll. Leaving a stretch for a safepoint stores the running activation's
  `program_counter` (`vm.rs`). `Interpreter::safepoint` answers whether the
  suspend stress mode rebuilt the stack, so that the loop reads its state
  again, and is always `false` in a release build
  (`interpreter/safepoint.rs`).
- **The words the poll reads** are two of the heap's machine words,
  `Rc<[Cell<u64>; WORD_COUNT]>` (`heap/machine_words.rs`), which machine
  code addresses from one base (`BlockWord::MachineWords`).
- **Every declaration keeps its text**, the core library's source is in the
  binary, and an image carries source unless built with `--strip-source`.
  An image does not record which lowering compiled its code
  ([compiler.md](compiler.md), "The reference lowering").
- **The MirrorBuilder's install** replaces a program's class -- its header,
  its methods on either side, its lazy slots -- atomically; it refuses a
  shipped class, and cannot yet change a superclass or the nested classes
  ([liveness-and-reflection.md](liveness-and-reflection.md), "A
  MirrorBuilder and an atomic install").
- **A signal searches its handler before anything unwinds** ("nothing is
  unwound until it decides", `Kernel.ns`, `signal`), and an unhandled
  exception reaches `defaultAction`, which ends the run through the VM
  mirror's `terminate…` leaves; `defaultAction` is where the debugger will
  plug in, and a test-only `Interpreter::resume_unhandled_exceptions`
  (`#[doc(hidden)]`) stands in for it today
  ([core-library.md](core-library.md), "Exceptions").
- **`SourceOrigin`** has `File` and `Embedded` only (`types.rs`).
- **A tuple literal** is lowered to one `MakeArray`, so it makes no
  `fromArray:`, `new:` or `at:put:` send a step could stop in; §5.1.7 lets
  a debugger treat it as one composition of its elements' values
  (`newspeak-spec.pdf` p. 26), never skipping the element expressions,
  which are ordinary statements' parts here.
- **The JetBrains plugin** runs a project or a module through its run
  configurations and talks to `ns-next-analyzer lsp` through the platform's
  LSP API ([editor-support.md](editor-support.md)).

## Planned: the decisions

Decided with the user on 2026-11-10, one at a time, each after the
specification, the prior art (`prior-art/jdk`, `v8`, `opensmalltalk-vm`,
`primordialsoup`, `newspeak`, `somns`) and the repository's invariants.
Each states the choice and the reasons the alternatives were rejected; the
mechanisms each implies are in the next section.

### 1. One debugging API in Rust, two clients over it

Every mechanism -- stopping and going on, the breakpoint instruction, the
stepping state, reading and writing frames, the metadata, deoptimization --
will sit behind one debugging API of `ns-runtime`. It has two clients: the
remote agent in `ns-cli`, which speaks the protocol to an IDE and is built
first, and `ActivationMirror` in `MirrorsFactory.ns`, through new mirror
leaves, which gives a program the access to activations §8.2 asks of
mirrors. Evaluation and edit-and-continue run Newspeak code through the
existing resolver, compiler and install.

HotSpot is cut the same way: JVMTI is the one API, and the JDWP agent
(`jdk/src/jdk.jdwp.agent/share/native/libjdwp/`) one of its clients.
Primordial Soup's debugger is instead Newspeak over activation primitives
(`primordialsoup/vm/primitives.cc:213-229`) and steps by simulating bytecodes
in a Newspeak interpreter (`primordialsoup/newspeak/Mirrors.ns:2437-2458`,
`:1190-1209`); `stepInto`, `stepOver`, `stepThrough` and `stepOut` differ
only in the predicate the simulation runs until. Rejected for ns-next: the
simulator would be a second interpreter of the instructions, the leaves and
the unwind markers, slower by orders of magnitude and drifting from the
real one with every new instruction; and the debugger would share the
debuggee's heap and thread, which needs actors before a program can be
stopped while Newspeak code of the debugger runs. A debugger written in
Newspeak can still be built later, on the same mirrors.

### 2. A breakpoint patches the body in place

A breakpoint will replace its slot of `EncodedBody::slots` by an
instruction `Breakpoint`, and keep the original beside the slots, on the
body ("Where the original lives" below). The interpreter stops at it and,
going on, executes the original. The slots become interior-mutable
(`Cell<CompactInstruction>`), written only through one narrow API; the run
loop's fetch copies a slot out as it does today. Setting a breakpoint
frees the body's machine code and every optimized copy that inlined it;
the baseline lowers a patched index as an exit, so every other body keeps
running in machine code.

- `Breakpoint` has no wire tag: the image encoder and `cargo xtask
  shipped-code` read the decoded view, which answers the original.
- Instruction indices do not move, so neither the statement table, nor the
  leftover analysis, nor any frame's `program_counter` is touched.

HotSpot patches the same way (`BreakpointInfo::set`/`clear`,
`jdk/src/hotspot/share/oops/method.cpp:2138-2162`, which deoptimizes all
dependents; the template at `cpu/x86/templateTable_x86.cpp:4030-4048`), and
V8 patches a debug copy of the bytecode array
(`v8/src/debug/debug.cc:507-525`) after deoptimizing the function
(`PrepareFunctionForDebugExecution`). A debug copy is rejected here
because a body is held by its mixin's method table, by its creator's
literal when it is a block, and by every running frame, all of which would
have to be repointed -- a second exchange mechanism beside the install; a
per-body flag checked at statement boundaries is rejected because it costs
the run loop a check or a second copy of the loop, and reaches a running
activation only if the loop rereads the flag at every instruction. Only
patching in place stops an activation already running, a loop the program
is in included.

### 3. Stepping by one-shot breakpoints and the safepoint

- **Step over** sets one-shot breakpoints at every statement start of the
  frame's body (V8's `FloodWithOneShot`, `v8/src/debug/debug.cc:1364`) and
  one at the stepping frame's **return point**: the index after the
  instruction its caller stands at -- a send, a `ResolveFuture`, an
  initializer invocation or a `SendMirror` -- never that slot itself, which
  `resume_caller` reads again. A frame left by an unwind -- an exception, a non-local
  return -- is reported by the unwinder, which reads a word of the
  debugger's as it pops; that costs only the cold unwind path.
- **Step out** is the return point's one-shot breakpoint and the
  unwinder's report.
- **Step into** is a step over plus a safepoint request held until the next
  push, which the debugger's safepoint stops at, at the new body's first
  statement start. A send answered by a primitive or a leaf pushes
  nothing, and the next one-shot breakpoint stops.
- **Smart step into** is a step into whose safepoint stops only at a push
  from the chosen `CallSite`.
- **Run to cursor** is a one-shot breakpoint at the target.

No check enters the run loop. What a step over or step out calls runs at
full speed, in machine code, but for the bodies the step patched, which
lose their machine code for the step, and every optimized copy that
inlined one of them, which is freed -- for a shipped body such as
`Array>>do:` or `whileTrue:`, possibly many copies, recompiled as their
budgets run out again. A step into's held request is taken at every poll
until the push ("Step into's held request" below). HotSpot instead runs a
stepping thread interpreted only (`enter_interp_only_mode`,
`jdk/src/hotspot/share/prims/jvmtiEventController.cpp:367-400`) with a hook
at every bytecode (`InterpreterRuntime::at_safepoint`), which its template
interpreter affords by swapping dispatch tables; ns-next's `match` loop has
none to swap, so the hook would cost the loop, and a step over a costly
call would run it interpreted.

### 4. Stop locations are statement starts; every send is recorded

The metadata of a body will list:

- every **statement start** -- the body's own, every block's and every
  inlined block's -- with its source span. A line breakpoint, a step over
  and a step into stop only there. A line breakpoint sits at the first
  statement beginning on its line, or the next line that has one, and each
  block body beginning on the line is a variant of it.
- every **send**: its instruction's index, its `CallSite` and the span of
  its receiver and selector, for smart step into's targets and for
  highlighting the send a frame stands at, a frame below the top included
  (Primordial Soup's `ActivationMirror>>sourceRange`, through `Simulator
  bciBefore:in:`, `primordialsoup/newspeak/Mirrors.ns:207-216`).
  The index after an instruction a caller can stand at -- a send, a
  `ResolveFuture`, an initializer invocation, a `SendMirror` -- is a
  **return point**, the one other
  place a stop happens: a step out, and a step over that leaves its frame,
  stop there, mid-statement in the caller, as Java's step out stops at the
  bytecode after the call.

The agent steps by line for IntelliJ, over statement starts until the line
or the frame changes, as the JDWP agent steps by its line table
(`stepControl_handleStep`, `jdk/src/jdk.jdwp.agent/share/native/libjdwp/stepControl.c:523-697`,
the line comparison at `:640-669`, `findLineNumber` at `:130`); a client
may ask for `granularity: statement`. A source span per instruction,
Primordial Soup's `bciSourceMapping`
(`primordialsoup/newspeak/Newspeak2PrimordialSoupCompilation.ns:564-596`),
is rejected: most of ns-next's instructions are register moves with no
place of their own in the source.

### 5. The metadata is compiled again on demand and verified

Neither table is stored. The first time the debugger needs a body's --
a stop in it, or a breakpoint the IDE sets -- it compiles the body's source
again, the lowerer collecting the stop locations and the **variable table**
on the side: each named variable's place (a register, a slot of the
indirection vector, a copied value) and the instruction range it is live
in. The result is compared with the running body; only an equal body gets
the tables, cached by the body ("Compiling the metadata again" below). A
body whose source is gone (an image built with `--strip-source`) or whose
recompilation differs (code an older binary compiled) gets none, and its
frames show without names and lines, as Java's do for a class compiled
without `-g`.

This rests on the invariant that compiling a resolved module is total and
deterministic. Primordial Soup does the same, recompiling a method for its
`DebugInfo` when asked (`primordialsoup/newspeak/Mirrors.ns:2711-2722`);
HotSpot reads tables the class file carries
(`parse_linenumber_table`,
`jdk/src/hotspot/share/classfile/classFileParser.cpp:1684-1719`, and
`parse_localvariable_table` after it, read by
`Method::line_number_from_bci` and JVMTI's `GetLineNumberTable` and
`GetLocalVariableTable`). Storing them would cost every body memory whether
anyone debugs or not, need wire tags of their own, and grow the embedded
shipped code; storing them only under `--debug`, `javac -g`'s way, would
leave the embedded core library and images without them and need this
decision's mechanism anyway.

### 6. The protocol is DAP over a localhost socket

The agent speaks the Debug Adapter Protocol, JSON in `Content-Length`
frames, over a TCP socket bound to 127.0.0.1, since the program owns its
standard streams; `ns-next run --debug …` opens it, as `-agentlib:jdwp`
does for a JVM. The framing is written by hand on `ns-cli`'s `serde_json`;
the analyzer's server dependencies stay out of `ns-next`, and `ns-runtime`
sees only the debugging API's types.

DAP covers breakpoints with conditions, hit conditions and log messages
(`setBreakpoints`), the breakable places and their variants
(`breakpointLocations`), method breakpoints (`setFunctionBreakpoints`),
exception filters (`setExceptionBreakpoints`), `pause`, the steps with
their `granularity`, smart step into (`stepInTargets`), `restartFrame`,
`evaluate` in a frame, `setVariable`, paged `variables`, the source of a
module with no file (`source` with a `sourceReference`), `terminate`,
`disconnect`, and the `stopped`, `output` and `exited` events. Hot swap,
edit-and-continue and force return are requests of ns-next's own, as
Microsoft's `java-debug` adapter extends DAP for hot code replace (not
under `prior-art/`). An own protocol
-- JDWP's way, or SOMns's Kómpos, JSON over a WebSocket (`FrontendConnector`,
`somns/src/tools/debugger/FrontendConnector.java:60-63`, its WebSocket
fields at `:84-86`) -- is rejected: it would have no second client, and what
is Newspeak's own, actor breakpoints later among it, can be added to DAP as
Kómpos added breakpoints for messages, promises and actors
(`somns/src/tools/debugger/entities/BreakpointType.java`).

### 7. Pause arms the poll words from the agent's thread

The machine words become `AtomicU64`s that the agent's thread reaches too
("The poll words as atomics" below); every hot access of the VM's is a
`Relaxed` load or store, the same instruction as today's on x64 and
AArch64, so neither the interpreter's poll nor machine code's changes. The
agent sets a pause bit of its own in the request word and lowers
`DepthLimit` to 0, so that the next poll anywhere -- interpreted or
compiled, a push or a loop's head -- takes the safepoint. The agent's two writes and the VM's clearing of a
debugger bit, which resynchronizes the limit, hold one lock per
interpreter (`debugger::Arming`), so that no clearing falls between the
two writes and leaves the limit at 0 with no bit set; the lock also says
which attachment is current, and a remote whose debugger was detached or
replaced arms nothing. Ctrl-C and the actors' scheduler need the same
word.

Cog's heartbeat thread writes `stackLimit`
(`forceInterruptCheckFromHeartbeat`,
`opensmalltalk-vm/src/spur64.cog/cointerp.c:16072`; `forceInterruptCheck`
at `:68263`), and the stack check every frame build makes is the poll;
V8's `StackGuard::RequestInterrupt` lowers the stack limit from any thread
under a lock (`v8/src/execution/stack-guard.cc:169-183`, `ExecutionAccess`);
HotSpot arms each thread's poll word
(`jdk/src/hotspot/share/runtime/safepointMechanism.inline.hpp:101-115`):
every one arms the word the poll reads anyway. A VM polling a channel or a
flag of its own is rejected for the check it adds to the poll and to
machine code, and for the pause it would take late.

### 8. An expression is evaluated as a block in the stop's scope

An expression -- `evaluate`, a breakpoint's condition, a log message, a
watch, `setVariable` as `name := expression` -- is resolved and compiled as
if it were a block standing at the frame's stored `program_counter` -- a
statement start, or a return point mid-statement -- its outer frame the
stopped frame, reified for it. It reads and writes the frame's
variables by reference, through the instructions a closure reads its
creator's registers with (`LoadOuterLocal`, `StoreOuterLocal`, counted in
`outer_reach`), and finds each variable by the variable table: an
assignment reaches the frame, a parameter or immutable local is refused by
the resolver as anywhere else, an assignment to a mutable local a block
captured by copy is refused ("Assigning a captured variable" below), and a
variable not live at the stop reads `nil`, never its register's leftover.
It runs on the stopped thread above the frame as ordinary program code,
without the VM mirror; breakpoints it meets are skipped, as IntelliJ skips
them by default, and an error it raises is its result, the program's
frames untouched ("Evaluation" below). Its source takes a `SourceOrigin`
of its own ([OPEN-WORK.md](../OPEN-WORK.md)).

Primordial Soup compiles the expression with a debug mapper that rewrites
every access to a frame variable into an access through the activation,
and runs the result in a simulated thread
(`primordialsoup/newspeak/Mirrors.ns:49-95`); that is a second resolution of
scopes beside the resolver, and would need `ActivationMirror` before an IDE
could evaluate. Evaluating read-only, on copies of the values, is rejected:
`count := 0` would appear to work and change nothing. IntelliJ itself
assigns a Java local through JDI's `StackFrame.setValue`, and refuses a
local a lambda captured -- the case refused here too.

### 9. A stop waits at the run loop's safepoint

A breakpoint, a step's stop or a pause leaves its stretch for the run
loop's safepoint, as a collection request does, and there calls the
debugging API, which blocks on a channel and serves the agent's commands --
reading the stack and values, setting breakpoints, evaluating -- answering
on another; no handle is live there, and the whole stack is on the
activation stack. Evaluating pushes the expression's activation and
returns into the loop, its return carried back into the stop by a
`Continuation` variant of its own: the trampoline every send uses, never a
re-entry of the loop from Rust ([execution.md](execution.md), "No re-entry
from Rust"). The debugger therefore needs no yield path out of `vm::run`;
that remains the image's need, for a snapshot taken mid-execution
([image-format.md](image-format.md), "Planned: suspend and resume").

HotSpot's thread waits inside the runtime call that posts the JVMTI event,
V8's embedder runs a message loop while paused (`runMessageLoopOnPause`,
`v8/include/v8-inspector.h:262`), and Squeak's and Primordial Soup's
debugged process is suspended while the debugger runs beside it. Leaving
`vm::run` at every stop and every evaluation, and moving the loop's state
across the safepoint into `World`, is rejected.

## Planned: the mechanisms

What building the decisions changes, found by checking them against the
code and the invariants in `CLAUDE.md` in three reviews; each is part of the
design, not an option, unless it names an open question ("Open
questions").

### Where the original lives

The run loop is not the only reader of a slot: `send_destination`
(`vm.rs`), two leaves in `jit/leaves.rs`, and the decoded view
(`CompiledCode::instruction`, `instructions`) read it, and the decoded view
feeds the leftover analysis, `jit::baseline::lower`, the optimizing tier's
snapshot and build, the image encoder, `shipped-code` and `disasm`. So the
original of a patched slot is kept on the body, beside `EncodedBody::slots`,
and every reader but two sees the original without reaching the debugger:
the decoded view answers it, and `send_destination` reads it. Only the run
loop's fetch sees `Breakpoint`, and `jit::baseline::lower` asks
`CompiledCode::is_patched(index)` and lowers a patched index as an exit.
The optimizing tier refuses to optimize or inline a body with a patched
slot. `Breakpoint` is therefore a compact form only, never an
`Instruction`: `bytecode.rs`'s exhaustive matches over `Instruction`
(`registers_read`, `register_written`, `outer_reach_of`, …) never see it,
and the leftover analysis runs over the originals.

Other readers match a slot without decoding it: `cached_send` (the miss
leaf, `inline_cache::decision_at`, and `jit::sends::call_slots`, which the
JIT's install, the materialization's `callee_of_call` and the cut use),
`is_send` (the exit statistics), `instantiate::factory_registers`, which
matches a factory body's first slots at every factory send,
`created_blocks` (`bytecode::mark_declaration_code`, the image decoder),
`for_each_emitted_mut`, and the derivations of `outer_reach`,
`addresses_own_slots` and `is_wide` -- with the run loop, `send_destination`
and the two leaves, every reader of `slots` there is. Each runs before a
body can be patched -- at build or decode time -- or only at an index
machine code reaches by a call, which a patched index never is: the
baseline lowers it as an exit, and a patch frees the body's code first.
Two run on a patched body at run time and see `Breakpoint`:
`factory_registers`, which then sees no factory body, so the run loop runs
it as an ordinary body; and the exit statistics' `is_send`, which runs at
the exit's own index (`MachineExit::Reexecute`) and so does not count the
patched index's exit as a send's -- a statistic only. Each of them is to
be checked again when the patching is built, and a new reader of `slots`
has to decide which view it wants.

### A breakpoint in a method with a fast path

A method with a `MethodFastPath` never runs its body (`mixin.rs`): a quick
method (`^true`, `^false`, `^nil`, `^self`, `^` a parameter), a method
that only sends `value` or `cull:` to a block parameter, and a mirror
forwarder whose leaf does not decline. The interpreter's `act` and machine
code's stubs read the fast path off the method on every hit, and the
optimizing tier inlines a quick method without looking at its body. So a
line breakpoint or a method breakpoint in, for example, a program's
`isEmpty = ( ^true )` would never stop. Patching such a method therefore
also replaces the method in its mixin's method table by one without the
fast path, the same body otherwise, and then invalidates the inline
caches (`Interpreter::invalidate_caches`) and frees every optimized copy
that inlined the method, as an install does; removing the last breakpoint
puts the method with its fast path back the same way. A check on every
hit whether the method is patched is rejected: a cache records what the
lookup decided and re-reads only what its key does not cover (`CLAUDE.md`,
"A cache records what the lookup decided; it never decides"), so the
check would cost every send of every fast-path method.

### Stopping at and going past a breakpoint

The `Breakpoint` arm of the run loop is `#[cold]`, out of line, and calls
into `interpreter/` (`vm.rs` holds no dispatch logic), which decides: stop,
evaluate a condition, count a hit, log, or skip. Going on, the arm
replaces the fetched slot by the original and re-enters the loop's `match`
with it -- the `match` sits inside a dispatch loop that runs once per
instruction unless the `Breakpoint` arm continues it with the original,
which costs nothing until the arm is reached, since breaking the loop's
outer labelled blocks from inside it is legal; a check before the `match`
would cost every instruction a compare -- so every kind of instruction, sends included, runs through its
ordinary arm and the original's behaviour is the interpreter's own;
`run_wide`'s way, the effect functions called out of line, is closed to a
send (`unreachable!("internal: a send is never encoded through Wide")`),
and statement starts and return points are often sends.

A stop that resumes sets a "past (activation, index) once" token, so the
loop does not stop at the same breakpoint again before executing it. An
activation's offset is reused after its pop, so the token is consumed or
dropped at the next `Breakpoint` arm the loop executes, and dropped when
its activation pops -- a restart, a forced return, a termination or an
unwind may leave it unexecuted; a restart, a forced return and
edit-and-continue drop it explicitly, since a restart does not pop its
activation and would otherwise skip the breakpoint at its first statement.
Breakpoints skipped during an evaluation neither consume nor drop it. A
`Continuation::Reexecute` runs a caller's instruction again at the same
index once a future it met is resolved (`vm::reexecute`); when that
instruction is a patched one, the arm sets the token again, so the
breakpoint neither stops, counts nor logs twice. The codegen gate's budget of
`vm::run` and `resume_caller` would show any growth the `Cell` slots or the
new arm cost the hot path.

### Breakpoints by body, and reclaimed code ids

A full collection reclaims code ids nothing names (`heap/tables.rs`,
`heap/reclaim.rs`), so a breakpoint keyed by code id could restore its
original into another body that later gets the id. The breakpoint table
holds the body itself (`Rc<CompiledCode>`) with its index, and a full
collection treats every patched body's code id as named, as it does
`CopyDependencies::code_ids`; a body without an id yet is given one. The
invalidation a patch needs is new: keyed by code id over every copy's
`CopyDependencies::code_ids`, it frees every copy that inlined the patched
body, beside the body's own machine code.

### A breakpoint names a source position

The IDE sets a breakpoint at a file and line. The agent resolves it
against the bodies compiled from that file -- methods, blocks (a block's
body is a literal of its home, so a block never yet created is patched in
place all the same), slot initializers, superclass clauses and initializer
expressions, each a body of its own -- through their metadata, compiled
again for the purpose. A breakpoint whose module is not built yet -- a
program module not loaded, a shipped module built on first demand through
`recordShippedModule:` -- is pending and is resolved when the module's
code is built. An install replaces bodies: each breakpoint in an edited
class is set again by its source position on the new bodies, and frames
still running the old bodies keep theirs.

An inlined control-flow block -- an `ifTrue:`'s, an `and:`'s, a
`whileTrue:`'s -- is lowered twice: inline in its home, and as the
fallback closure a receiver that is not a Boolean runs
([execution.md](execution.md), "Fallback blocks"). A line breakpoint, a
flood and a method breakpoint patch both copies, or a non-Boolean receiver
runs past the breakpoint.

The plugin needs the breakable places before any session runs, since
IntelliJ asks `canPutAt` and the variants while breakpoints are set before
a launch; `breakpointLocations` answers only within a session. So the
places are proposed to come from `ns-next-analyzer`, through a request of
the language server, which can compile a module, and to be verified by
`breakpointLocations` when the session starts ("Open questions").

### Step over in recursion and blocks

A one-shot breakpoint carries the frame it is for, by its position on the
activation stack: a `FrameReference` would reify the frame and send it
through its body's twin and a divorce on return, while a position is
reused only after a pop, which the return point's breakpoint or the
unwinder reports first, ending the step. It is ignored in any other activation of the body -- a recursive call
deeper on the stack, or a block of the same home body running in a callee
-- but taken in a fallback closure's activation whose creator is the
stepping frame, so that a step over a non-Boolean receiver's `ifTrue:`
stops in its fallback block as in the inline copy. V8 checks
the frame count the same way ("We have to ignore recursive calls",
`v8/src/debug/debug.cc:822-855`), and the JDWP agent compares `fromDepth`
(`stepControl.c:523-697`). The flood covers the stepping body's own
statement starts and those of the inlined blocks in it; a block the body
creates is a body of its own and not flooded, so a step over a `do:` whose
argument block contains a breakpoint stops only at that breakpoint, as
IntelliJ's does.

### Step into's held request

The held request is taken at every poll until the next push, not only at
pushes: a backward jump in the stepping frame, or a caller resumed, also
reaches the safepoint, which goes on without stopping when nothing was
pushed. A loop with no send, or a statement whose sends are all answered by
primitives, so takes the safepoint at every iteration until a push or a
one-shot breakpoint ends the step, and machine code's loop-head poll exits
at each. The request is a real one (the debugger's bit counts as real), so
that a compiled prologue refuses the push. The safepoint recognizes a push by
a frame on top other than the stepping frame and above it -- not by a
`program_counter` of 0, which a backward jump to the stepping body's first
instruction, or a frame `retry` restarted, also shows -- and stops at the
new body's first statement start, which is index 1, not 0, for a body --
a method's or a block's -- whose first instruction makes its indirection
vector (`MakeVector`, emitted before the local initializers and outside
every statement's range, `compiler/lower.rs`).

### The poll words as atomics

The whole machine-word area becomes `Arc<[AtomicU64; WORD_COUNT]>`, since
machine code addresses every word from one base and the two words cannot
move to an area of their own; the argument of `heap/machine_words.rs` that
machine code's accesses do not alias the protected `&mut` carries over to
atomics. The request word, today one of the values 0, 1 and `STRESS_HOLD`
tested by `== 1` (`Heap::real_safepoint_requested`), becomes a set of bits
-- the VM's real request, the stress hold, the debugger's pause and step
bits, a "commands pending" bit -- with the real and the stress tests as
masks. Every write of the request word becomes a read-modify-write:
the VM's own sets too -- `write_safepoint_request`, `request_safepoint`,
`hold_safepoint_request`, whole-word stores today -- become `fetch_or`,
or they would erase the agent's bits and lose a pause. The agent sets its
bits with `fetch_or` and then stores 0 into
`DepthLimit`, both `SeqCst`, so that no store of the VM's can fall between
its two writes unseen; it never clears a bit. The VM clears the bits it
served -- its own, and the agent's pause and commands bits once it served
them -- with `fetch_and`, a locked read-modify-write on x64 and an
`ldclr` or exclusive pair on AArch64 -- the cold path only. `DepthLimit`
races with the VM's own writes (`write_safepoint_request` and
`set_depth_limit`, through which `set_depth_reserve` writes): each of these
cold writers issues
a `SeqCst` fence between storing `DepthLimit` and reading the request word
again, and writes 0 if a real or debugger bit is set, so that a pause
arriving between the two is not lost; without the fence a store→load
reordering, which x64 allows, would lose it, and a fully compiled
computation that only recurses, whose prologues compare `DepthLimit` alone,
would never see the pause. The hot reads stay `Relaxed`. The ordering argument is held by review and
by a model test of the agent and the two writers in the style of `loom`,
a dev-dependency to be justified when it is built; the `aarch64` gate runs
the suite on AArch64's encoding of these accesses, but QEMU's user mode
does not reproduce AArch64's weak ordering, so it cannot confirm the
argument.

### Commands while the program runs

`setBreakpoints`, `pause`, `terminate` and `disconnect` arrive while the
program runs. The agent queues the command and sets the "commands pending"
bit; the next safepoint serves the queue without stopping, unless the
command is `pause`. The agent queues a command before it sets the bit,
and the VM clears the bit before it drains the queue, so that a command
queued while the queue is drained keeps its bit set for the next
safepoint; the other order would lose it, a `pause` in a fully compiled
loop included. A leaf that runs long (a large `LargeInteger`
multiplication, a large copy) delays the next poll, and with it the pause,
until it returns; so does a leaf blocked waiting for input, reading the
standard input among them, which no pause and no `terminate` served at a
safepoint reaches. The agent's thread then ends the process itself
("Open questions").

### Reading the stack at a stop

A stop is reached through an exit from machine code or a safepoint. An
exit has materialized every compiled frame of its stretch into the
interpreter's frames, the inlined scopes of optimized code with their
headers written and their eliminated blocks made (`jit/materialize.rs`);
frames below, in earlier stretches, were materialized when their code left
for the interpreter's send. A stop reached through a poll of optimized code
(`StoppedAtPoll`) is the exception ("What exists"): its eliminated blocks
are deferred, and it would go back into its optimized code at the poll,
restoring constants from the code rather than the frame. So the debugger
deoptimizes such a stop before the agent reads or writes the stack, as the
debug-only `jit::deoptimize_stopped_at_poll` does today during the
safepoint; `deoptimize_at_poll` itself is in every build, where
`resumed_at_poll` calls it after the safepoint for a frame that cannot go
back in, and only the wrapper that does it on demand during the safepoint
is to be made available in every build: the deferred blocks are made and every
scope goes on in baseline code. Only then is every frame of the stack the
interpreter's word for word, a variable holding an eliminated block shows
its closure, and a write reaches what the code reads. A frame that is only
read goes back into its machine code when something returns into it.
Frames of every kind appear on the stack: methods, blocks, initializers,
superclass clauses; a frame shows its body's name (a block as
"`[] in Foo bar`", from its home link), its source position, and the send
it stands at. A frame's registers are shown through the variable table at
its stored `program_counter`, and a scratch register only when the
leftover analysis (`bytecode::leftovers`, `tainted_at`) says it is not
tainted there: a frame may hold words a callee or a popped frame left,
and a variable outside its live range is shown as not yet assigned.

### Values the IDE holds

An object the protocol names by an id is held as a `Root` in a table of
the stop, released when the program goes on unless the IDE pins it, as
JDWP's object ids and their `DisableCollection` do. A value's label comes
from `Heap::describe`, never `{:?}`. A builtin's elements (`Array`,
`ByteArray`, `String`) are read by the heap's accessors and paged; a
Newspeak collection (`List`, `Map`, `Set` of `CollectionsFactory.ns`)
shows its slots until evaluation exists, then a renderer that runs
Newspeak code (`size`, `do:`, `printString`) under a limit of time and
sends, skipping breakpoints as every evaluation does.

### Writing a frame

Reading a compiled frame needs nothing; a frame whose variable is set, or
that is restarted or forced to return, goes on in the interpreter -- a stop
at an optimized poll having been deoptimized first (above): its
compiled mark is cleared (`Heap::clear_compiled`, as the run loop does
where it gives a frame to the interpreter), as
[liveness-and-reflection.md](liveness-and-reflection.md), "The unifying
principle", requires.

### Evaluation

- **Where it lies.** The expression's activation is pushed above every
  live frame's whole register window (`Mutator::push_above`), whichever
  frame it evaluates in: IntelliJ evaluates in any selected frame, and a
  frame below the top has its callees above its window; at a statement
  start inside an inlined block, scratch registers of the enclosing
  expression may still be live in the stopped frame too.
- **How it returns.** Its return must not go through `resume_caller`: the
  stopped frame's slot is not a send, and `send_destination` would reach
  its `unreachable!`. The new `Continuation` variant records the result,
  requests a safepoint, and so re-enters the stop's command loop, which
  finds the evaluation's frame gone. The run loop has to learn, in every
  build, that the stack changed at the safepoint -- which
  `Interpreter::safepoint` answers today only for the suspend stress mode,
  `false` in a release build -- when the debugger pushes the evaluation's
  frame and when it returns. The variant is named in
  [image-format.md](image-format.md), "Planned: suspend and resume", for
  completeness; no image is written while an evaluation runs, so it is
  never encoded.
- **The frame it reads.** Reifying the stopped frame clears its compiled
  mark first, so that it goes on interpreted -- a frame the debugger may
  write needs no machine code; the run loop could enter a reified frame in
  its body's reifying twin (`jit::entry_of_frame`), so this is a choice,
  not a necessity; a stop at an optimized poll is
  deoptimized before ("Reading the stack at a stop"). The debugger makes
  the expression's closure itself, as a `MakeClosure` in the frame would,
  the stopped frame its creator, so that `LoadOuterLocal` and
  `StoreOuterLocal` at depth 1 reach the frame's registers. A block the
  expression creates can escape (`[count]` stored away), so the
  frame is marked as `MakeClosure` marks a creator whose block has an
  `outer_reach` above 0, or it would drop its registers when it returns and
  the block would read `nil` at exit status 0.
- **Its errors.** The handler search walks below the expression into the
  program's own `on:do:` frames, and a program's handler would catch the
  expression's error and unwind the program's frames. So the expression
  runs inside a handler for every `Exception`, at its base, that answers
  the exception as the evaluation's result; `retry` and `pass` cannot
  cross it. An `on:do:` does not stop a non-local return, so `^` in an
  evaluated expression is refused by its compiler, and the evaluation's
  base frame is an unwind barrier the unwinder stops at: a program block
  the expression calls whose `^` would return to a home below the
  evaluation -- `b value` for a `b` holding `[^1]` -- is stopped there,
  and the evaluation answers the error a return to a home it cannot reach
  signals (`CannotReturn`), the program's frames untouched. Machine
  code's cut already exits for a home below its stretch's entry frame
  (`jit/cut.rs`), so the interpreter's unwinder is the one place that
  changes. A stop at the depth limit leaves no room to
  evaluate: the evaluation takes a reserve of its own, or is refused there
  ("Open questions").
- **Hot conditions.** A breakpoint's condition and a logpoint's message are
  compiled once per breakpoint and frame layout and cached; still, every
  hit leaves the stretch and runs an evaluation, so a conditional
  breakpoint in a hot loop slows that loop, and the body, which exits at
  every hit, is abandoned by the JIT's exit decay, correctly. A condition
  that raises an error stops at the breakpoint and reports the error, as
  IntelliJ does; a condition evaluated at the depth limit follows the
  depth limit's open question.
- **Aborting.** An evaluation or a renderer past its limit, or one the user
  cancels, is stopped by the agent's pause bit; the stop unwinds the
  evaluation to its base frame, running its `ensure:` blocks, and answers
  that it was aborted.

### Assigning a captured variable

Closure conversion copies a captured mutable local into the closure
unless something can write it once a block holds it
(`compiler/capture.rs`, `needs_vector`): a local a block writes, one
written conditionally or in a loop enclosing the capture, goes into the
indirection vector instead. Such a copy is read-only by
construction ([execution.md](execution.md), "Closure conversion"). An
assignment through evaluation -- in the home frame or in the block's --
would write one copy and leave the others stale, and the reference
lowering, which does not copy, would behave differently. So the variable
table records, for each mutable local, whether a block of its family
captured it by copy, and an assignment to such a variable is refused with
a message naming the capture, as IntelliJ refuses a local a lambda
captured; a variable in the indirection vector and a register no block
captured are assigned. Whether it could instead be moved into a
by-reference layout through edit-and-continue is open ("Open
questions").

### Evaluation never reaches the VM mirror

The VM mirror is a capability (`CLAUDE.md`, "The VM mirror is a
capability"). An expression evaluated in a shipped module's frame -- a
frame of `Kernel.ns` a step into reached -- would resolve `vmMirror` as
the frame's slot and could hand the mirror to the program, as its result
stored into a program object. So evaluation in a frame whose body belongs
to a shipped module is refused; the IDE shows such a frame's variables,
read by the agent and held as `Root`s, which never enter the program. A
value the IDE shows enters the program only through an expression the user
typed in a program frame, and an object reached only through a shipped
activation can be neither marked nor named in one. `ActivationMirror`
withholds every value of a shipped activation, as the mirrors withhold
what they reach through a shipped level.

### Exceptions

A signal searches its handler before anything unwinds, so an uncaught
exception stops with its whole stack, where the search, having found no
handler, is about to send `defaultAction` (`invokeNextHandler`,
`Kernel.ns`), before any subclass's override of it runs. The frame shown
first is the first frame below the exception's own machinery -- below
`signal`, the instance side's and the class side's `signal:`, and
`Object>>doesNotUnderstand:` for a `MessageNotUnderstood` -- which is the
raising frame itself only for a VM error, whose `signal` is pushed right
above it; with step filters ("Open questions") that is the first frame of
program code. Going on lets `defaultAction` run, which ends the program
for every exception whose class does not override it, as its
`terminate…` leaf does today, resumable ones included; a `Halt` and a
declined `MessageNotUnderstood` are handled as below. Whether a handler
takes an exception is decided by Newspeak code (`handles:`), so "caught"
is known only once the search found one: a caught-exception breakpoint
stops where the search invokes the handler it found
(`invokeHandlerAt:disabling:`), before the handler runs. A class filter is
decided in Rust by walking the exception's class chain, running no code.
`halt` signals `Halt`, whose breakpoint is on by default under the
debugger: it stops, and going on resumes the `Halt` with `nil`. An
uncaught `MessageNotUnderstood` offers "Create Method": the method is
created public, since the send again is a public send, and the exception
is resumed, sending the message again
([core-library.md](core-library.md), "Exceptions"). Declined, the
failing send answers `nil`: the debugger forces
`Object>>doesNotUnderstand:`'s frame to return `nil` -- the one forced
return in a shipped frame the refusal below allows. Resuming the
exception instead, as `resume_unhandled_exceptions` does today, would
send the message again (`doesNotUnderstand:` sends it again once the
exception reached its default handler, `Kernel.ns`), signal the same
`MessageNotUnderstood` again, and stop again, without end.

A handler block runs on top of the stack before anything unwinds, so it is
never flooded by a step over. As Java's step over stops in the `catch`
that takes an exception thrown below, a step over whose exception a
handler at or below the stepping frame takes stops at the handler block's
first statement: the search's invocation of the handler
(`invokeHandlerAt:disabling:`) is reported to the stepping state, as an
unwind is.

### Restart frame and force return

- **Restart** unwinds to the frame's `FrameReference` and sets its
  `program_counter` back to 0, the `Restarted` outcome `retry` uses
  (`interpreter/unwind.rs:34-38`), which keeps
  the registers as they are: a restart also nils the named temporaries
  between the copied values and `local_count`, while the scratch registers
  are tainted at index 0 anyway. A block's copied values stay
  (Primordial Soup's restart, `primordialsoup/newspeak/Mirrors.ns:2325-2353`).
  The frame restarts at `program_counter` 0, not at its first statement:
  under the optimized lowering a body with vector variables makes its
  indirection vector at index 0, outside every statement's range, so a
  restart makes a fresh vector. Blocks made in the first run keep the
  vector and the copies they had and do not see the reset variables, while
  under the reference lowering, whose blocks read the frame's registers,
  they do -- a difference between the lowerings to be decided ("Open
  questions"). Restarting an initializer, a superclass clause or a frame in
  `InitializeSlot`'s work would run slot initializers twice, so it is
  refused ("Open questions").
- **Force return** unwinds to the frame and answers a value to its sender,
  defined for a frame whose return is `Continuation::Deliver`; for
  `NestedClass` (which expects a class), `VmErrorSignalled`, `Reexecute`
  and `Unwind` it is refused ("Open questions"); a `ReturnToSuspended`
  wraps another continuation, so force return looks through it to the
  continuation it wraps and decides by that one. It is refused for an
  initializer's frame, whose caller stands at an initializer invocation
  -- the slots it has not initialized would stay uninitialized -- and for
  an unwind marker's activation (`on:do:`, `ensure:`, `ifCurtailed:`),
  whose ensure block it would skip.
- **Shipped frames.** Setting a variable, restarting and forcing a return
  are refused in a shipped module's frame, as evaluation is: forcing
  `signal` or an `ensure:` marker to return would break the kernel's own
  invariants. The one exception is the forced `nil` from
  `Object>>doesNotUnderstand:` when "Create Method" is declined
  ("Exceptions"), which the debugger issues itself.

### Hot swap and edit-and-continue

- **Hot swap** is the MirrorBuilder's install of the changed classes of a
  saved module: the agent compares the module's new text with each
  declaration's text, stages the changed declarations through builders and
  installs them atomically. What the install cannot do -- a shipped class,
  a superclass change, nested classes added, replaced or removed
  ([OPEN-WORK.md](../OPEN-WORK.md), "Reflection beyond road step 5b2a") --
  is refused as a whole and reported, as IntelliJ reports a schema change
  it cannot apply. Breakpoints are set again on the new bodies by source
  position.
- **Edit-and-continue** recompiles the edited method -- new instructions,
  new register count, new statement table -- and moves its activations
  onto the new code. The top activation goes on at a statement boundary:
  its current statement restarts, or its first. A pending activation,
  stopped mid-statement in a send, is restarted at its current statement
  or its first; every frame above it is unwound first, its `ensure:` blocks
  run. Statements of the old and the new body are matched by source, not
  by index; values move by name through both bodies' variable tables, a
  variable that moved between a register and the indirection vector
  included; added locals start `nil`. A variable a live closure of the
  activation shares through the indirection vector keeps the same vector
  object, and one a live closure copied cannot move into a vector; an edit
  that would need either is applied by unwinding the activation and
  restarting it at its first statement, or refused. A running activation is an entry in
  a stack where an entry in the middle cannot grow, so it is divorced into
  a newly allocated, larger frame object, every reference to the old frame
  rewritten through the bulk forward behind `become:`
  (`Heap::forward_identity`) -- which has no `may_be_forwarded` lock of
  its own and rewrites every object, the stack's words, the side tables of
  suspended activations and continuations -- pending unwinds among them --
  and the host roots, the agent's `Root`s among them, while
  `world::forward_identity` forwards the step roots, `Mutator.depth_reserve`
  among them; the debugger forwards only handles it holds outside these,
  and the object forwarded to must never have been hashed. Its callers are
  the mirror leaf and the install; the debugger becomes a third (the
  sentence in `Heap::forward_identity`'s doc that names one caller is
  stale already) -- the new frame allocated old and born
  remembered, and execution continues from the new frame as from any
  suspended one, resumed onto the stack when something returns into it
  ([execution.md](execution.md), "Suspending and resuming";
  [object-model.md](object-model.md), "The object layout"). That is the
  same move as forcing a frame out of optimized code, one level down.
  Activations of the old body's blocks keep running the old block code.
  Cog forces the contexts running a changed method back to bytecode
  program counters the same way (`primitiveVoidVMStateForMethod`,
  `widowOrForceToBytecodePC`, `opensmalltalk-vm/src/spur64.cog/cointerp.c:24552`,
  `:21910`), and Primordial Soup restarts a frame in the method looked up
  afresh (`restart:withLookup:` and `lookupNewMethod:`,
  `primordialsoup/newspeak/Mirrors.ns:2325-2337`, `:2307-2315`).

### Compiling the metadata again

- **Which lowering.** The lowering is chosen once per interpreter, so for
  a program read from source it is known. An image does not record which
  lowering compiled its code; the agent tries the optimized lowering, then
  the reference lowering, and takes the one whose body is equal.
- **Both lowerers collect.** The reference lowering is an independent
  module, not a mode ([compiler.md](compiler.md), "The reference
  lowering"), so both lowerers collect stop locations and variables.
- **What "equal" means.** Equal decoded instructions and side tables,
  block literals compared as bodies rather than by identity, symbols by
  name; a patched slot by its original.
- **What is compiled.** The whole module is parsed and resolved from its
  current text, as `interpreter/edit.rs` prepares an edit, and only the
  class subtree the body belongs to is compiled.
- **Shipped bodies.** A shipped module's body is compiled again from the
  shipped resolution the boot made, cached per thread
  (`interpreter::kernel::resolved_kernel`,
  `core_library::resolved_shipped_module`), so that its `SendMirror` sites
  are the same and the bodies can be equal; no new caller of
  `resolver::resolve_kernel` is needed. The first lock of the VM mirror
  permits a shipped module resolved with it (`CLAUDE.md`, "The VM mirror
  is a capability"); still, the result is only compared and its tables
  kept, never installed, run or handed to anything but the metadata
  cache.

### Method breakpoints

A method breakpoint patches the body's first statement start (for a
body whose first instruction makes its indirection vector, index 1) and
every return instruction of the body; a non-local return or an unwind out
of the method is reported by the unwinder, as for a step. The method is
named by its selector and its class declaration, and set again on the new
body after an install. A method with a fast path loses it while the
breakpoint is set ("A breakpoint in a method with a fast path").

### Smart step into a block

IntelliJ offers a lambda on the line as a target. Here a block argument's
body is pushed from inside its callee -- `do:`'s loop, not the chosen
`CallSite` -- so a block target is a one-shot breakpoint at the block
body's first statement start, carried for every activation of that block
created by the stepping frame, rather than a push from a call site.

### The return value of the last call

IntelliJ shows the value the last call returned after a step over or a
step out. It is the value in the destination register of the instruction
before a return point -- written by `resume_caller` for a callee that
returned, or by the send's own arm for a send a primitive answered. A step
out stops at the return point and reads it there; a step over stops at the
next statement start, past the return points of the stepping frame's
sends, so during a step over the stepping state records the value at each
of its sends' returns, the last one reported with the stop.

### Launching, the start and the end

`ns-next run --debug` binds the socket, prints its port on the standard
error stream before the program writes anything, and waits for the
IDE's `configurationDone` before `main:args:` runs, so breakpoints are set
first, as JDWP's `suspend=y` does; `stopOnEntry` stops at the first
statement of `main:args:`. The agent accepts one connection. `terminate`
ends the program as its own `terminate…` would; `disconnect` removes every
breakpoint and lets the program run on. A program that ends -- while
running, while stopped, or in the middle of a step -- sends `exited` with
its exit status. The program's standard streams stay its own: when the
IDE launched the process it shows them itself, and the agent sends
`output` events only for logpoints and its own messages, so no line is
shown twice.

### Threads

The VM is single-threaded: the protocol shows one thread. A pull future's
forcing runs on the same stack and shows as frames of it. Each actor will
be a thread of its own ([liveness-and-reflection.md](liveness-and-reflection.md),
"Concurrency: actors, not threads").

### The time a step took

The agent measures the wall time from the resume to the stop, minus the
time it spent serving commands meanwhile, and sends it with the `stopped`
event, which the plugin shows inline as IntelliJ does.

### Testing and gates

- **The pause stress mode** (`NS_NEXT_STRESS_PAUSE`, debug builds, one
  of the `stress` gate's modes; `Interpreter::set_stress_pause` for a
  test) stops the run at the safepoints its schedule picks -- paced by
  the traced size as the collect and suspend modes are (`StressSchedule`,
  `interpreter/safepoint.rs`) -- exactly as a pause does: first at the
  safepoint, a frame its optimized code's poll left deoptimized
  (`jit::deoptimize_stopped_at_poll`). No debugger is attached: the stop
  takes its commands from a `CommandSource`, the attached debugger's
  channels or the mode's reader (`debugger::StressReader`), which asks
  for the stack trace, the variables of every frame and the children
  (from the first, eight of them) of every value those answered, then
  continues; a refusal is a defect and panics. Every golden and the
  corpus run under it, so that reading a stopped stack is shown never to
  change a program's outcome nor to read a register the leftover
  analysis calls dead, which a debug build refuses.
- **A breakpoint stress mode** patches every statement start of every body
  a program runs, goes on at each, and holds every golden's outcome under
  both lowerings, with the JIT and with `NS_NEXT_NO_JIT=1`; a **step stress
  mode** steps into everything; both join the `stress` gate.
- **Debugger goldens**: a `.ns` with a script of commands and the stops,
  stacks and values expected, run by a scripted DAP client in `ns-cli`'s
  tests.
- **The `codegen` gate** after the `Cell` slots and the `Breakpoint` arm;
  a measurement (the `measuring-performance` skill) that `--debug` with no
  breakpoint runs at plain speed; the `linux` and `aarch64` gates for the
  atomics.
- **The plugin's tests** drive its `XDebugProcess` against the agent.

## Open questions

Raised by the reviews of these decisions; each is to be decided with the
user, one at a time, before the stage that needs it. The proposal is a
proposal.

- **Step filters.** Whether step into skips the shipped modules' code by
  default, with a setting to step into it, and how a step behaves at its
  edge: a step over at the last statement of a `do:` block would land at a
  return point inside `Array>>do:`, and the next element's block
  activation is no push from the stepping frame. Proposal: shipped code is
  filtered by default; a stop that would fall in filtered code goes on as
  a step into, which IntelliJ does for library frames, so the step lands
  in the block's next activation or back in the caller; a filtered step
  into keeps its request through the pushes of shipped code until a push
  of program code. As IntelliJ skips `java.*` and V8 skips blackboxed code
  (`IsBlackboxed` in `FloodWithOneShot`, `v8/src/debug/debug.cc:1364-1368`).
- **Watchpoints on slots.** Whether a slot's reads and writes can be
  breakpoints. Own-slot accesses are inline instructions, and machine code
  stores with an inline barrier, so a watchpoint needs every body that
  accesses the slot patched, or its layout level marked. Proposal: patch
  the own-slot instructions of the slot's level, found through the
  metadata.
- **Access control in evaluation.** Whether `evaluate` obeys private and
  protected as the frame's method would. Proposal: as the frame's method
  would, since the expression is resolved as a block in its scope; IntelliJ
  ignores access in Java. The question bears on the VM mirror's capability:
  an evaluation that ignored access in a program frame could send private
  messages to kernel objects the program holds, which the kernel's access
  modifiers keep from it today.
- **Marking objects.** Whether an object can be marked and named in later
  evaluations. Proposal: yes, within the rule that a value reached only
  through a shipped activation cannot be marked.
- **The socket.** Whether the agent requires a token the IDE passes, since
  any local process could connect and run code through evaluation.
  Proposal: a token generated by the IDE, passed on the command line or the
  environment, checked at `initialize`.
- **Who listens.** `ns-next` listens and prints its port (proposed), or the
  IDE listens and passes its port.
- **Debugging an image.** Whether `ns-next load --debug` exists; an image
  built with `--strip-source` shows no names or lines.
- **Evaluation at the depth limit.** A reserve of its own, or refused.
- **Restart and force return beyond the plain case.** Whether a restart of
  an initializer or a superclass clause, a forced return for the other
  `Continuation`s, of an initializer's frame or of an unwind marker's
  activation, are defined rather than refused.
- **Restart and the indirection vector.** A restart at `program_counter`
  0 makes a fresh vector, so blocks made in the first run keep the old
  vector under the optimized lowering, while under the reference lowering
  they see the reset variables. Proposal: restart reuses the frame's
  vector, its elements nilled, restarting at index 1 past `MakeVector`, so
  blocks of the first run see the reset of the vector variables; a value a
  first-run block copied it keeps, where the reference lowering's blocks
  read the frame's register, which the restart nils -- a difference
  between the lowerings that remains, for a variable nothing writes once a
  block holds it. (A vector variable's own register is always `nil`, so
  nilling the named temporaries alone does not reset it.)
- **Assigning a captured variable.** Refused, as proposed above, or the
  frame moved into a layout where the variable lives in a vector, through
  edit-and-continue, so that the assignment reaches every copy. Proposal:
  refused, as IntelliJ refuses a captured Java local.
- **Stepping out of a frame that returns elsewhere.** Step out is defined
  for a frame whose return delivers into its caller's instruction. Not yet
  for a superclass clause (`NestedClass`, whose return creates the class),
  an `ensure:` or `ifCurtailed:` block the unwinder runs (`Unwind`, whose
  return carries the unwind on), a future's `resolve` (`Reexecute`), and a
  caller standing at an instruction that raised (`vm::raise` stores the
  `program_counter` of the raising instruction, which may be no send; a
  send path raises too, and that caller goes on through
  `VmErrorSignalled`). Proposal: each
  stops at the next statement start the program reaches, the step going
  on as a step into until one is reached.
- **Breakable places before a session.** Whether `ns-next-analyzer`
  answers the breakable places and their variants to the plugin, through a
  request of the language server, before any session runs (proposed), or
  the plugin allows a breakpoint on every line and the session moves it.
- **Terminating a program that reaches no safepoint.** A program blocked
  in a leaf waiting for input, or in a long leaf, serves no `terminate`.
  Proposal: the agent's thread ends the process after a short grace
  period, as IntelliJ's stop button kills a JVM that does not answer.
- **Further IntelliJ features.** The memory view (the instances of a
  class, and the objects referring to one, through a walk of the heap at a
  stop), folding the shipped modules' frames in the stack, and attaching
  to an `ns-next` already running. Proposal: the memory view and folding
  in stage 5, attaching after stage 6.

## Planned: the JetBrains plugin

The plugin will implement IntelliJ's debugger API over a DAP client of its
own:

- an `XDebugProcess` started by a `ProgramRunner` for the Debug executor
  from the existing run configurations, which run `ns-next run --debug`;
- an `XLineBreakpointType` whose `canPutAt` and variants come from
  `breakpointLocations`, an `XBreakpointHandler`, a method breakpoint type,
  and a breakpoint type for exceptions;
- `XSuspendContext`, `XExecutionStack`, `XStackFrame` and `XValue` over
  `stackTrace`, `scopes` and paged `variables`;
- an `XDebuggerEvaluator`, an `XSmartStepIntoHandler` over `stepInTargets`,
  run to cursor through `runToPosition`, restart frame, force return and a
  hot-swap action on the agent's own requests, and inline values and step
  times;
- the core library's modules opened from `source` requests.

## Planned: the order of building it

1. The debugging API at the safepoint: stopping, the command channel and
   the commands served while running, the atomic poll words and pause, the
   deoptimization of a stop at a poll of optimized code in every build,
   the stack and values.
2. The metadata: both lowerers' stop locations and variable tables,
   compiled again on demand and verified.
3. Breakpoints (pending ones included), stepping, run to cursor, method
   breakpoints and exception breakpoints; the breakpoint and step stress
   modes.
4. The DAP agent in `ns-cli` and the plugin's `XDebugProcess`: from here
   the debugger is usable in the IDE.
5. Evaluation, conditional breakpoints and logpoints, watches and setting
   variables, smart step into, restart frame and force return.
6. Hot swap, then edit-and-continue of running activations;
   `ActivationMirror` for programs.
