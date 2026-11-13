# The debugger

> **Status:** **Partial:** stage 1 is built -- the debugging API at the
> run loop's safepoint ("The debugging API at the safepoint"): a host
> attaches a debugger and hands its remote to another thread, which pauses
> the program wherever it runs, interpreted or in machine code, optimized
> code included, queues commands the next safepoint serves, and, while the
> program is stopped, reads its stack, each frame's receiver, arguments and
> live registers, and the fields and elements of the values those answer,
> then continues or terminates the run; the request word the poll reads is
> a set of bits in atomic machine words, which the agent arms from its own
> thread; and the pause stress mode stops every program at the safepoints
> its schedule picks and reads its whole stack. Stage 2 is built too --
> the metadata ("Compiling the metadata again"): both lowerers collect a
> body's statement starts, its sends with their receiver and selector
> spans and its named variables when the debugger compiles the body's
> declaration again from its module's current text, kept per body only
> when the body compiled again is alike; a stopped frame shows its name,
> the statement and the send it stands at, and its named variables over
> its whole lexical chain, and the oracle holds every body of every
> golden and of the shipped modules to compiling again alike. Stage 3 is
> under way: a line breakpoint, set by its module and an offset into its
> text, pending until its module is built and set again on the bodies an
> install replaces, patches its statement's slots in place and stops the
> interpreter there ("Stopping at and going past a breakpoint", "A
> breakpoint names a source position"); a patch frees the body's machine
> code and every optimized copy that inlined it, the baseline lowers a
> patched slot as an exit and the optimizing tier leaves a patched body
> alone, and a method's fast path is off while a breakpoint stands in its
> body ("Where the original lives", "A breakpoint in a method with a fast
> path"); step over, step into and step out stop at the stepping frame's
> next statement start, in a frame it pushes and at its caller's return
> point, by one-shot breakpoints for the stepping frame and a request bit
> a step into holds until the next push, and report the value of the last
> call ("Step over in recursion and blocks", "Step into's held request",
> "The return value of the last call"); a step whose frame an unwind pops,
> or whose frame returns otherwise than to its caller's send, goes on as a
> step into from the frame the program goes on in (decision 17); and no
> step lands in a frame the step filters filter, by default the shipped
> modules' -- a stop it would make there goes on as a step into from the
> first unfiltered frame below, and a step into's request outlasts the
> pushes of filtered bodies (decision 16); run to cursor stops at a
> position resolved as a line breakpoint is, in any activation, once
> (decision 3); and a method breakpoint stops at its method's entry and
> before each of its returns, and reports an exit an unwind makes where
> the program goes on ("Method breakpoints").
> Everything else is planned: the rest of stage 3 -- exception
> breakpoints -- the DAP agent and the plugin, evaluation, hot swap and edit-and-continue
> ("Planned: the order of building it"). The
> design rests on fifteen decisions made with the user, nine on
> 2026-11-10 and stage 2's six on 2026-11-12 ("The decisions"), on the mechanisms they imply ("Planned: the mechanisms"),
> and leaves the questions in "Open questions" to be decided. It builds on
> groundwork that exists: reified activations and the activation stack,
> suspension into frames, the statement→program-counter table, stable
> call-site numbers, retained source, the unwinder's frame targets, the
> JIT's materialization and invalidation, the safepoint poll, and the
> MirrorBuilder's atomic install, each described in the document for its own
> aspect ("What exists").
> **Code:** stages 1 and 2 are `crates/ns-runtime/src/debugger/` (`mod.rs`: the
> VM's side, `Interpreter::attach_debugger`, `serve_debugger`, the stop,
> the arming lock `Arming`, the pause stress mode's `StressReader`;
> `breakpoints.rs`: the breakpoint table, line breakpoints and the
> one-shots; `stepping.rs`: the step in progress, `Stepping`;
> `remote.rs`: `DebugRemote`, `Command`, `Event` and the descriptions that
> cross to the agent's thread; `stack.rs`: the stack trace, a frame's
> body found by its declaration (`origin_of`) and its variables;
> `values.rs`: the values a stop holds, `StopValues`; `metadata/`: the
> tables (`mod.rs`: `BodyMetadata`, `MetadataCollector`), a declaration's
> level compiled again and paired (`again.rs`), the cache (`cache.rs`)
> and the oracle, `Interpreter::check_debug_metadata` (`check.rs`)),
> the collection in both lowerers (`compiler/lower.rs`,
> `compiler/reference.rs`, the `collector` parameter of the entry points
> in `compiler/mod.rs`), `Mutator::mixin_level`
> (`interpreter/instantiate.rs`), `bytecode::compiles_alike`, the cache
> cleared at an install (`interpreter/install.rs`), the selector spans
> (`ns_syntax::ast::MessageSend::selector_spans`), the
> request word's bits in `heap.rs` (`REAL_REQUEST`, `STRESS_HOLD`,
> `DEBUGGER_PAUSE`, `DEBUGGER_COMMANDS`, `DEBUGGER_STEP`, `LOWERS_DEPTH_LIMIT`,
> `Heap::synchronize_depth_limit`, `Heap::clear_debugger_bits`),
> `heap/machine_words.rs` (the machine words as atomics behind an `Arc`),
> `interpreter/safepoint.rs` (`Interpreter::safepoint`, which serves the
> debugger first, `SafepointOutcome`, the pause stress mode's schedule),
> `vm.rs` (`take_safepoint`, `terminate_at_safepoint`), `jit/mod.rs`
> (`deoptimize_stopped_at_poll`, in every JIT build), and the tests in
> `crates/ns-runtime/tests/debugger.rs`, `tests/stress.rs` (the pause
> stress mode), `debugger/metadata/tests.rs` and the oracle's runners
> `crates/ns-cli/tests/debug_metadata.rs` and `golden_round_trips.rs`
> ("Testing and gates"). The groundwork is in `crates/ns-runtime/src/bytecode.rs`
> (`CompiledCode::statement_program_counter`, `CallSite`),
> `bytecode/compact.rs` (`EncodedBody`), `bytecode/leftovers.rs`, `vm.rs`
> (the run loop, its safepoint, `resume_caller`, `send_destination`),
> `types.rs` (`Continuation`, `SourceOrigin`), `interpreter/unwind.rs`,
> `interpreter/install.rs`, `jit/materialize.rs`, `jit/mod.rs` (`abandon`,
> `invalidate_applications`, `CopyDependencies`), `core-library/Kernel.ns`
> (the exceptions) and
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
- step filters: by default, no step stops in the shipped modules' code,
  so stepping into `coll do: [:each | …]` lands in the block, not in
  `Array>>do:`, and the program's own classes can be added to the filter
  (decision 16).

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

The debugger's own first stage is described in the next section, "The
debugging API at the safepoint". What it and the later stages build on:

- **Activations** are frames of tagged words on an activation stack of their
  own, walkable without the Rust stack, reified on demand into frame objects
  and suspended into them and resumed lazily ([execution.md](execution.md),
  "Activations", "Suspending and resuming").
- **`statement_program_counter`** maps every statement of a body to its
  instruction range (`bytecode.rs:532`; [compiler.md](compiler.md),
  "Safepoints and call-site identity"); an inlined block's statements are
  part of their enclosing statement's range. A body carries one `source`
  span and stores no table of local names: the debugger's tables are
  collected when it compiles the body again ("Compiling the metadata
  again").
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
  mark; `jit::deoptimize_stopped_at_poll`, in every JIT build, makes the
  deferred blocks and sends the frame on in baseline code instead, which
  the debugger's stop and termination and the suspend stress mode call
  during the safepoint ([execution.md](execution.md), "Back into optimized
  code at a poll"; "The stop" below).
  Freeing a body's machine code is safe for frames stopped in it,
  which go on interpreted (`jit/mod.rs`, `abandon`). An install frees
  every optimized copy that relied on an application it rebuilds
  (`invalidate_applications`, keyed by dispatch id); each copy records the
  code ids it inlined (`CopyDependencies::code_ids`), today so that a full
  collection keeps those ids alive.
- **The safepoint poll**: the interpreter polls at every push, every
  caller resumed and every backward jump, one compare of the whole request
  word with zero; machine code polls at each loop's head the same way, and
  at a compiled push through the prologue's compare of the depth with
  `DepthLimit`, which a request that lowers it (a bit of
  `LOWERS_DEPTH_LIMIT`: the VM's own, the debugger's pause or commands),
  never a stress mode's hold (`STRESS_HOLD`), sets to 0
  (`Heap::synchronize_depth_limit`; [memory.md](memory.md),
  "Where a collection can happen"; [execution.md](execution.md), "Polls
  and the depth check"; "The poll words as atomics" below). A compiled
  `ret` into a compiled caller does not poll. Leaving a stretch for a
  safepoint stores the running activation's `program_counter` (`vm.rs`).
  `Interpreter::safepoint` serves an attached debugger first and answers
  a `SafepointOutcome` (`interpreter/safepoint.rs`): `Unchanged`;
  `Rebuilt` when the suspend stress mode rebuilt the stack, so that the
  loop reads its state again, never in a release build; and `Terminate`
  when the debugger terminated the run, in every build.
- **The words the poll reads** are two of the heap's machine words,
  `Arc<[AtomicU64; WORD_COUNT]>` (`heap/machine_words.rs`), which machine
  code addresses from one base (`BlockWord::MachineWords`) and a clone of
  which the debugger's remote holds on its own thread.
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

## The debugging API at the safepoint

Stages 1 and 2 of the debugger ("Planned: the order of building it") are
built: the VM's side of decisions 7 and 9 and the part of decision 1 that
pausing, reading and ending a run need, and the metadata of decisions 4,
5 and 10 to 15, which give a stopped frame its name, its place and its
named variables ("Reading the stack at a stop", "Compiling the metadata
again"). It lives in `crates/ns-runtime/src/debugger/`
and is reached through `Interpreter::attach_debugger`; its only clients
today are the test suite (`crates/ns-runtime/tests/debugger.rs`) and the
pause stress mode, since the DAP agent in `ns-cli` is stage 4's. A
program stops for a pause, at a line breakpoint the interpreter
reaches ("Stopping at and going past a breakpoint") and where a step
arrives ("Step over in recursion and blocks"); no evaluation or write of
a frame exists yet, and a stop only reads.

### The attachment and the remote

`Interpreter::attach_debugger` (`debugger/mod.rs`) attaches a debugger to
the interpreter and answers its remote, a `DebugRemote`
(`debugger/remote.rs`), the agent's end, which may move to another
thread. The two ends talk through two `std::sync::mpsc` channels, the
commands to the VM and the events back; the interpreter holds the VM's
end (`Interpreter::debugger`, a `debugger::Debugger`: the command
receiver, the event sender and the id the next stop's first value takes).
The remote holds besides a clone of the heap's machine words
(`Heap::machine_words_handle`), through which it arms the poll, and the
interpreter's arming lock with the number of the attachment that handed it
out ("The arming lock" below). Nothing that crosses to the agent's thread
holds a `Value`, a `Root`, a handle or a `Symbol`, each meaningful only on
the VM's thread: a command and an event are text and numbers, and a value
is named by a `ValueId` that the stop which handed it out resolves
("Values the IDE holds" below).

The remote has three operations:

- `pause()` asks the program to stop at its next safepoint, interpreted or
  in machine code, at a push or a loop's head. A pause asked while no run
  executes stops the next run at its first safepoint.
- `send(command)` queues a command and then sets the commands bit, so that
  the next safepoint serves it while the program runs, or the stop serves
  it while the program is stopped. A command queued while no run
  executes, or still queued when a run ends, is dropped unanswered, but
  a configuration command -- `SetBreakpoints`, `SetStepFilters` -- which
  the start or the end of a run serves ("Commands while the program runs"). A remote
  whose VM is gone queues it nowhere and arms nothing.
- `next_event(timeout)` answers the VM's next event, or `None` once the
  timeout passed or the VM is gone.

The remote is served from another thread than the one running the
interpreter: a stop blocks the running thread until a `Continue` or a
`Terminate` arrives, so a host that pauses and then runs the program on
one thread waits in the stop forever.

Attaching again replaces the debugger: the earlier one's channels go with
it, the commands still queued in them included, its pause and commands
bits are cleared under the hold of the arming lock that makes the new
attachment current, so that the new remote meets no stop it never asked
for (`a_pause_before_a_replacement_does_not_stop_the_run`), every
breakpoint it set is removed, and its remote, still held, arms nothing
from then on. Dropping the remote detaches the debugger ("Detaching"
below).

### Commands and events

The commands (`debugger::Command`):

- `Continue` -- the stopped program goes on; answered by `Continued`.
- `StepOver`, `StepInto`, `StepOut` -- the stopped program goes on to the
  top frame's next statement start, or the first statement start of a
  frame it pushes, or its caller's return point (decision 3, "Step over
  in recursion and blocks", "Step into's held request"); answered by
  `Continued`, then by the step's `Stopped`, unless another stop comes
  first, which ends the step and reports its own reason.
- `RunToPosition { module, offset }` -- the stopped program goes on to
  the statement the offset resolves to, as a line breakpoint's does, in
  any activation (decision 3); answered by `Continued`, then by
  `Stopped` with `StopReason::Step`, unless another stop comes first,
  which ends the run to cursor as it ends a step; refused, the program
  still stopped, with "no module of that name is built" or "no statement
  starts at or after that position".
- `Terminate` -- the run ends as an uncaught termination does
  ("Continuing and terminating" below); served whether the program is
  stopped or running, and answered by no event: the host's send answers
  the error.
- `StackTrace` -- the stopped program's activations, innermost first;
  answered by `Event::StackTrace`, a `FrameDescription` each.
- `Variables { frame }` -- the variables of the activation at index
  `frame` of the stack trace; answered by `Event::Variables`, or refused
  with "no frame at that index".
- `Children { value, start, count }` -- `count` children of a value the
  stop handed out, from the `start`th; answered by `Event::Variables`, or
  refused with "no value of this stop has that id".
- `SetBreakpoints { module, offsets }` -- replaces every line breakpoint
  of the module `module` names, its root class's name as
  `FrameLocation::module` names it, by one at each byte offset into the
  module's current text ("A breakpoint names a source position");
  answered by `Event::Breakpoints`. A configuration command: served
  wherever commands are taken, at a stop, at a safepoint while the
  program runs, and at the start and the end of a run.
- `SetStepFilters { shipped, classes }` -- replaces the step filters
  (decision 16): `shipped` filters the shipped modules' bodies, and each
  `ClassName { module, path }` of `classes` the bodies of the class
  declaration it names and of every declaration nested in it, `module`
  the module's root class's name and `path` the names of the
  declarations from that root down, the root's own excluded, so that
  an empty path names the whole module. Until the first, the shipped
  modules are filtered and no class is, and every attachment begins
  with that default. Answered by `Event::StepFiltersSet`; a
  configuration command, served as `SetBreakpoints` is.
- `SetMethodBreakpoints(Vec<MethodBreakpoint>)` -- replaces every method
  breakpoint ("Method breakpoints"), each naming a method by its
  `class`, a `ClassName` as the step filters name a declaration, its
  side (`class_side`) and its `selector`; answered by
  `Event::MethodBreakpoints`, a configuration command served as
  `SetBreakpoints` is.

The events (`debugger::Event`): `Stopped { reason, returned }`, the
program stopped and serves commands until a `Continue` or a `Terminate`
-- `StopReason::Pause`, `StopReason::Breakpoint { ids }`, the
breakpoints standing at the statement it stopped at,
`StopReason::Step`, a step's or the run to cursor's arrival, or
`StopReason::MethodEntry { id, others }` and `StopReason::MethodExit {
id, others }`, a method breakpoint's ("Method breakpoints"); `returned`, at a step's stop, the
value the stepping frame's last call answered, described as the stop's
first value, and `None` at every other stop ("The return value of the
last call"); `StackTrace`; `Variables`, the
answer to both `Variables` and `Children`; `Refused { command, reason }`,
the command `Command::name` names not served; `Continued`; and
`Breakpoints { module, breakpoints }`, the module's line breakpoints in
the order of their offsets, each a `BreakpointState` -- its
`BreakpointId`, its offset and the span of the statement it stops at, or
`None` while it is pending -- the answer to `SetBreakpoints`, and sent
unasked when a pending one resolves or an install moves one;
`StepFiltersSet`, the answer to `SetStepFilters`; and
`MethodBreakpoints`, each method breakpoint's `MethodBreakpointState` --
its id and whether its method is declared (`verified`) -- in the order
they were set, the answer to `SetMethodBreakpoints`, and sent unasked
when a module's build or an install changes whether one is verified.
`Continue`, the three steps, `RunToPosition`, `StackTrace`, `Variables` and `Children` are
served only at a stop; one that arrives while the program runs is refused
with "the program is running", and one queued between runs is dropped. Every command a stop or a safepoint serves but
`Terminate` is answered by one event, in the order the commands came. An
agent therefore waits for `Stopped` before it sends a command only a stop
serves: one sent right after `pause()` is refused or served depending on
whether a safepoint drains the queue before the stop or the stop serves
it.

### The poll words as atomics

The heap's machine words are `Arc<[AtomicU64; WORD_COUNT]>`
(`heap/machine_words.rs`; [execution.md](execution.md), "The machine
words"), so that a clone of them crosses to the agent's thread; the words
the poll reads cannot move to an area of their own, since machine code
reaches every word from the one base. The argument that machine code's
accesses do not alias the protected `&mut` the mutator is held by during
an entry carries over: Rust reads and writes the words only through
shared references to atomics, and machine code through the address
derived from the `Arc`'s own pointer, which no retag covers. Every hot
access of the VM's is a `Relaxed` load or store, on x64 and AArch64 the
plain instruction a `Cell`'s was, so neither the interpreter's poll nor
machine code's changed.

The request word (`MachineWord::SafepointRequested`) is a set of bits
(`heap.rs`):

- `REAL_REQUEST` (bit 0) -- the VM's own request: a collection's, a
  scavenge's, a full code zone's (`Heap::request_safepoint`);
- `STRESS_HOLD` (bit 1) -- a stress mode's hold
  (`Heap::set_safepoint_requested`, and `Heap::hold_safepoint_request`
  for the deopt stress mode);
- `DEBUGGER_PAUSE` (bit 2) -- the agent's pause;
- `DEBUGGER_COMMANDS` (bit 3) -- the agent queued a command;
- `DEBUGGER_STEP` (bit 4) -- a step into's request, held until the next
  push ("Step into's held request"): the VM's own, set
  (`Heap::hold_step_request`) and cleared on its thread alone, under the
  arming lock as the debugger's other bits are cleared.

Every write of the word, the VM's and the agent's, is a `SeqCst`
read-modify-write -- a `fetch_or` to set a bit, a `fetch_and` to clear one,
a locked instruction on x64, an atomic or exclusive-pair sequence on
AArch64, on the cold path only -- so that no writer erases another's bit.
The poll compares the whole word with zero: every bit takes the safepoint.
`Heap::real_safepoint_requested` masks it with `LOWERS_DEPTH_LIMIT`
(`REAL_REQUEST | DEBUGGER_BITS`), every bit but the stress hold's, and
`Heap::debugger_bits` with the debugger's three (`DEBUGGER_BITS`). The VM
clears its own bit and the hold once it served them
(`Heap::clear_safepoint_request`), never the debugger's; only
`Heap::clear_debugger_bits` clears those, once the VM served them, or
once the push a step into waited for came or the step ended. The agent
never clears a bit.

A compiled callee's prologue compares the depth count with the
`DepthLimit` word alone, so a request that must stop compiled recursion
lowers that word to 0. After every write of the request word, and of the
limit (`Heap::set_depth_limit`, through which `Mutator::set_depth_reserve`
writes), `Heap::synchronize_depth_limit` writes the word: 0 while a bit of
`LOWERS_DEPTH_LIMIT` is set, the limit `set_depth_limit` last set
otherwise. The debugger's bits therefore lower it as the VM's own request
does, and the stress hold does not, so that compiled frames still nest
under a stress mode ([execution.md](execution.md), "Polls and the depth
check").

The agent arms the poll with two writes (`DebugRemote::arm`): its bit's
`fetch_or` into the request word, then a store of 0 into `DepthLimit`,
both `SeqCst`. `DepthLimit` races with the VM's own writes of it, each of
which stores the limit with a `Relaxed` store after reading the request
word to decide it. That is the store-buffering pattern: without an
ordering, the VM's read could miss the agent's bit while the VM's store of
the full limit lands after the agent's 0, and the pause would be lost to
every compiled push -- a computation that only recurses in machine code,
whose prologues compare `DepthLimit` alone, would never stop. So
`synchronize_depth_limit` stores the limit, issues a `SeqCst` fence
(`MachineWords::fence`), reads the request word again and stores 0 if a
lowering bit is set now: with the fence, either that second read sees the
agent's bit, or the agent's store of 0 comes after the VM's store. A
threaded test races the agent's two writes against the VM's writes of the
limit over 10,000 rounds (`no_pause_is_lost_to_a_racing_depth_limit_write`,
`heap.rs`); a model test of the ordering does not exist ("Testing and
gates" below).

### The arming lock

The remote's two writes and the VM's clearing of a debugger bit, which
resynchronizes the limit, hold one lock per interpreter
(`debugger::Arming`, an `Arc<Mutex<Attachments>>` the interpreter holds
and every remote it ever handed out shares): the agent across its
`fetch_or` and its store of 0, the VM across `Heap::clear_debugger_bits`,
in `Interpreter::clear_debugger_bits` and `detach_debugger`, and across
its setting and clearing of the step bit, which `debugger::stepping`
does from inside an entry through the clone of the lock the breakpoint
table holds while a debugger is attached (`Breakpoints::arming`). Without it,
the VM could serve the bit and clear it between the agent's two writes;
the agent's late store of 0 would then land after the VM's
resynchronization, leaving the request word at 0 and `DepthLimit` at 0:
no poll takes a safepoint that would resynchronize it, and every compiled
push is refused at the word and counted as an exit until something else
writes the request word or the limit. One lock for every attachment,
since a remote of an earlier attachment holding a lock of its own would
reopen that race against the VM's clearing under another. V8 arms its
interrupts the same way, under a lock (`StackGuard::RequestInterrupt`
under `ExecutionAccess`, `v8/src/execution/stack-guard.cc:169-183`). Only
the cold paths take the lock, never a poll.

What the lock guards says which attachment is current (`Attachments`: the
current attachment's number, `None` while no debugger is attached, and the
last number taken). `attach_debugger` takes the next number and makes it
current; `detach_debugger` makes none current under the same hold that
clears the debugger's bits; and a remote arms only while its own number is
current (`Attachments::is_current`). So a remote whose debugger was
detached, or replaced by a later attachment, arms nothing: its bit would
stop the run for another remote's debugger, or for none
(`a_replaced_remote_s_pause_does_not_stop_the_run`).

### Serving at the safepoint

`Interpreter::safepoint` (`interpreter/safepoint.rs`) calls
`Interpreter::serve_debugger` first, before the pause stress mode, the
optimizing tier's install, the other stress modes, the code zone's
compaction and a pending collection: a safepoint whose service raised
another request returns before it clears the request, and a pause served
after that point would wait for a later safepoint
(`a_pause_while_a_collection_is_requested_is_served`). `serve_debugger`
reads the debugger's bits and takes the stop a `Breakpoint` arm left
for ("Stopping at and going past a breakpoint"); with neither the
safepoint goes on. Bits set with no debugger attached are cleared, since
nobody is left to stop for. The commands are served first, then the
breakpoint's stop, which serves a pause asked meanwhile too, then the
pause, then a step into's held request (`stepping::held_step`): a frame
pushed above the stepping one stops there ("Step into's held request").
Every stop ends a step in progress (`stepping::end`), whatever made it.

### Commands while the program runs

With the commands bit set, the queue is drained
(`serve_queued_commands`): the bit is cleared first, under the arming
lock, then every queued command is taken without waiting (`try_recv`). A
`Terminate` ends the run, and the commands queued after it are dropped
but for a configuration command; a `SetBreakpoints` is served -- the
breakpoints patched or restored, answered by `Event::Breakpoints` -- and
so is a `SetStepFilters`, and the program runs on; any other command is refused, "the program is
running". The agent queues
a command before it sets the bit, and the VM clears the bit before it
drains the queue, so that a
command queued while the queue is drained keeps its bit set for the next
safepoint; the other order would lose it, a `Terminate` sent to a fully
compiled loop included.

A command that acts on a run is served by the run it arrives during.
`run_from_host` clears the commands bit, under the arming lock, and then
takes every queued command, before a run starts and once it ended
(`Interpreter::serve_between_runs`), in the order the drain keeps and
for its reason: a command the drain misses keeps its bit and is served,
or dropped, at the next safepoint. A configuration command is served
there, so that breakpoints an agent sets before the program runs, or
between two runs, are set when the next run starts; every other command
is dropped unanswered. Without the drop, a command left queued -- a
`Terminate` sent after its run ended, or queued after the one that ended
it -- would otherwise end the next run at its first safepoint, or be
taken by its first stop, a run nobody terminated
(`a_terminate_after_its_run_ended_does_not_end_the_next`,
`a_terminate_queued_after_the_one_that_ended_the_run_ends_no_other`). A
pause is no command: one asked between runs stays, and stops the next run
at its first safepoint.

Once the queue is drained, `serve_debugger` reads the pause bit again;
if it is set, it is cleared, under the arming lock, and the program stops
("The stop" below).

A leaf that runs long -- a large `LargeInteger` multiplication, a large
copy -- delays the next poll, and with it the pause, until it returns; so
does a leaf blocked waiting for input, reading the standard input among
them, which no pause and no `Terminate` served at a safepoint reaches
("Open questions", "Terminating a program that reaches no safepoint").

The DAP request `setBreakpoints` arrives while the program runs, as
`pause` and `terminate` do, and becomes `SetBreakpoints`, which the next
safepoint serves from the queue without stopping
(`a_breakpoint_set_while_running_takes_effect`). Planned: `disconnect`
becomes a command served the same way, the debugger detached with every
breakpoint removed, as a remote's departure detaches it today
("Detaching"), and the program runs on.

### The stop

A stop (`Interpreter::stop`, `debugger/mod.rs`) first makes the frame its
optimized code's poll left go on in baseline code
(`jit::deoptimize_stopped_at_poll`, in every JIT build, counted in
`JitStatistics::poll_deoptimizations`): the light exit of a poll in
optimized code wrote the inlined scopes' headers but deferred the blocks
the copy eliminated, and would go back into the optimized code at the
poll, restoring values from the code's constants rather than the frame
("What exists"). Made now, the deferred blocks exist and every frame of
the stack is the interpreter's word for word before the agent reads it
(`a_stop_at_an_optimized_poll_is_deoptimized`). Every other compiled frame
of the stretch was materialized by the exit that left it for the
safepoint, and frames of earlier stretches by the exits that left them.

The stop then sends `Stopped { reason, returned }` -- a pause's, a
breakpoint's with the ids standing at its slot, or a step's with the
value of the last call, the first value of the stop's table ("Values the
IDE holds") -- and blocks on the
command channel (`recv`), on the VM's thread, between two stretches: no
handle is live, and nothing pushes or pops until the program goes on. The
stack is the activations on the activation stack and, below them, the
senders a suspension left in their frame objects ("Reading the stack at a
stop"). It serves `StackTrace`, `Variables`, `Children` ("Reading the
stack at a stop", "Values the IDE holds"), `SetBreakpoints` and
`SetStepFilters` until a
`Continue`, a step or a `Terminate`; a step begins there
(`stepping::begin`), is answered by `Continued` and goes on as a
`Continue` does. A command the stop serves was queued with its
bit set, which stays set: once the program goes on, the next safepoint
clears it and finds the queue empty. A pause asked while stopped likewise
stays set, and stops the program again at the next safepoint.

### Continuing and terminating

`Continue` answers `Continued` and returns into the safepoint, which goes
on with the pause stress mode, the other stress modes and a pending
collection, and clears the VM's request; the run loop goes on from the
frame the stretch left, a frame the stop deoptimized in baseline code.

`Terminate` -- at a stop, or drained while the program runs -- clears a
pause still pending, which the end of the run makes moot, ends a step in
progress, whose request would lower the depth limit for the `ensure:`
blocks the unwind runs and whose one-shots would stop in them, makes the frame
an optimized poll left go on in baseline code, so that the unwind pops
interpreter frames, and makes `Interpreter::safepoint` answer
`SafepointOutcome::Terminate` at once, the rest of the safepoint undone: a
request still standing is served at the next poll. The run loop
(`vm::take_safepoint`) calls `vm::terminate_at_safepoint`, which raises the
termination in the top activation (`debugger::termination`: a
`Signal::Terminate` whose error has the selector `terminate`, the top
activation's receiver and the message "the debugger terminated the
program") and hands it to the one unwinder (`interpreter/unwind.rs`) with
the run's floor, as the termination a leaf raises is handed: every pending
`ensure:` and `ifCurtailed:` block runs on the way, each started
interpreted, and once the floor is popped the host's send answers the
error (`HostSignal::Error`; `terminate_runs_the_pending_ensure_blocks`,
`a_terminate_queued_while_running_ends_the_run`).

### Detaching

Dropping the remote detaches the debugger. The VM notices it where it next
talks to the remote -- a command channel found disconnected while draining
or while stopped, or an event that cannot be sent -- and
`detach_debugger` makes no attachment current and clears both debugger
bits under one hold of the arming lock, removes every breakpoint, each
slot given its original back, ends a step in progress and puts the step
filters back to their default, then drops the debugger. A stopped
program goes on as after a `Continue`, and a running one is never stopped
again (`a_dropped_remote_lets_the_program_finish`,
`a_remote_dropped_while_stopped_lets_the_program_finish`).

### Reading the stack at a stop

**Partial:** a stop answers the run's frames and, for each, its name,
its receiver and, from its body's metadata ("Compiling the metadata
again"), the statement and the send it stands at in its module's current
text and its named variables over its whole lexical chain (decisions 10,
12 and 13); a frame whose body has no metadata shows its arguments and its
registers below the first scratch register, by number. Writing a frame is
stage 5's ("Writing a frame").

The run's frames (`stack::frames`) are walked from the top activation
through each sender down to the run's floor
(`Heap::activation_position`): the activations below belong to the host's
sends that started this run, which the stop does not show. Not every
frame of the run is on the activation stack: a suspension
(`Heap::suspend_activations`, which the suspend stress mode makes at the
safepoints its schedule picks; [execution.md](execution.md),
"Activations") moves every activation of the run into its frame object
and resumes the top one, whose stack frame then returns to the suspended
frame (`Heap::activation_suspended_sender`) rather than to the one below.
From the activation whose sender is suspended the walk goes on through the
senders the suspended frames name (`Heap::frame_sender`) until one names
none. That chain stays within the run: only the run's own safepoint
suspends, only its activations at its floor or above, and the lowest of
them names no sender, since it returns below the floor. A suspended frame
is held for the stop as a `Root` of its frame object
(`stack::StackFrame::Suspended`) and read from it: its kind, its code, the
program counter it stores (`Heap::frame_stored_program_counter`), its
receiver and its registers (`Heap::frame_local`;
`the_stack_reaches_suspended_senders`). The walk is made once per stop,
at the first command that reads the stack, since nothing pushes or pops
while the program is stopped, so reading every frame's variables costs
the stack's depth, not its square.

A `FrameDescription` (`stack::stack_trace`) gives, innermost first:

- `kind` -- what the activation runs (`FrameKind`, from its
  `ActivationKind`): `Method { selector }`, `Block`, `Initializer`,
  `SuperclassClause`, `Factory` or `TopLevel`;
- `name` -- what it runs as a reader names it, after the declaration its
  body belongs to: `Foo>>bar`, `Foo class>>bar`, `[] in Foo>>bar`,
  `Foo initializer`, `Foo superclass clause`, `Foo class>>new:` for a
  factory, `top level`; after its current class where no declaration
  holds the body;
- `program_counter` -- the program counter its frame stores: for a frame
  below the top the index of the send, or other instruction, it called
  from, which its callee's return reads again; for the top frame where the
  stretch left it;
- `source` -- the span of its body's source (`CompiledCode::source`), the
  whole body;
- `location` -- with the body's metadata, where it stands
  (`FrameLocation`): the name of its module's root class, the innermost
  statement whose instructions hold its program counter -- an inlined
  block's statement inside its loop or conditional, the body's `source`
  for the return it ends with -- or a factory's class header pattern
  (decision 12), and for a frame below the top the receiver and selector
  spans of the send it waits on; every span in the module's current text
  (decision 11). `None` without metadata (decision 5): a module without
  text, an image's without its source among them, or a body its text no
  longer compiles to, as a superseded version an edit changed;
- `receiver` -- its receiver, described as every value is.

A frame's body is found by its declaration (`stack::origin_of`, a
`BodyOrigin`): a method's in its current class's mixin, an initializer's
in the mixin carrying the body (`Heap::initializer_mixin`), a superclass
clause's in the nested declaration whose clause it is, a factory's in its
class's declaration, and a block's in the member of its current class's
level -- or of a nested declaration's clause -- whose body creates it,
walked transitively through `created_blocks`. The mixin is the version
whose member holds the frame's body (`stack::running_version`): the
version its class names, or else the newest version of the same id an
install superseded that the heap still holds (`Heap::superseded_mixins`,
which keeps every version a running body was compiled in). The class
alone does not tell: an install re-applies every class object of the
declaration it edits over the new version
(`Heap::class_install_application`) and rebuilds every body of it and of
its nested declarations, so a frame still running a body of the old
version, edited or not, names the current version through its class. The
activation's entered application (`frame_header::ENTERED_LEVEL`) would
name the old version, but a push records it only for a body that
addresses its own slots. The metadata cache (`metadata::Cache`) compiles
the level again on the first ask and keeps each body's tables, or none.
The metadata is looked up before any handle is read off the frame.

A frame's variables (`stack::variables`) are `self`, then, with the
body's metadata, each variable of its table once -- the innermost of a
name, the body's own coming first -- each read where it lives:

- in a register of the frame itself, a parameter, a temporary or a value
  it was handed;
- in a register of the frame `hops` closures' captured frames out
  (`Heap::activation_captured`, `Heap::frame_captured`): a block's outer
  variable it did not capture, read from the nearest frame of its chain,
  innermost first, whose layout holds a copy of it -- an enclosing block
  that was handed it -- or else from the frame that declares it, as a
  block reading by reference resolves it; read while that frame's
  activation runs, is
  suspended, or returned keeping its registers
  (`Heap::frame_registers_kept`: a divorce of a frame marked by
  `retain_registers` leaves `true` in its married word) -- or when the
  closure that hopped to it reads its creator by reference, whose creation
  marked it, which covers a frame an image decoded; otherwise the variable
  lives nowhere any more and has no value, shown as not available
  (decision 10);
- in a slot of an indirection vector, read through the register that
  holds it, its own frame's or one `hops` out; a vector variable read
  before its vector is made, a register still `nil`, reads `nil`, as
  every temporary does before it is written (decision 13).

A variable entry carries no instruction range: an inlined block declares
no parameter and no temporary, so every variable a body names is in scope
over the whole body, `nil` from its start.

A frame without metadata keeps its name, which comes from its
declaration, not from the text: a frame of an image without source shows
the mixin's name the image carries, no location and its variables by
number. A frame's variables without metadata are stage 1's: `argument 1`
up to `argument N`, the
argument registers, and `register K` for each register past the frame's
header registers (`HEADER_REGISTERS`) up to the body's `local_count`, the
first scratch register. A scratch register is never shown (decision 13).
A register of the frame's own the leftover analysis finds tainted before
the frame's stored program counter (`CompiledCode::tainted_before`;
`bytecode::leftovers`) is not read -- a named one is not available, a
numbered one left out -- as the collector's pre-pass nils it
(`Heap::trace_stack`) and a suspension nils it in the frame object: such
a word may be one a callee or a popped frame left, a stale handle, or a
debug build's poison, whose read a debug build refuses; another frame's
register is read only below its body's `local_count`, and while that frame
runs only where the analysis at its own program counter calls it live.
The analysis taints no register below `local_count` today, since a push
fills them all; the check keeps the reader to it should that change.

Reading changes nothing: no compiled mark is cleared, and every frame goes
on as it would have, but the one the stop deoptimized, in baseline code.

### Compiling the metadata again

The metadata of decisions 4, 5 and 10 to 12 is built
(`crates/ns-runtime/src/debugger/metadata/`). A body's tables
(`BodyMetadata`) are its statement starts (`StatementEntry`: the
instructions one statement compiled to and its span), its sends
(`SendEntry`: the index, the `CallSite`, the receiver's span and the
selector tokens' spans), its named variables (`VariableEntry`: the name,
the lexical depth in frame scopes, and the place, a `VariablePlace` --
`Register`, `Outer { hops, register }` or `Vector { vector, slot }`, the
vector held in a register of the frame or of one `hops` out), the body's
own first, parameters before temporaries, then each enclosing scope's
outward, and, for a factory, the header's factory pattern
(`factory_pattern`). Every span is in the module's current text
(decision 11).

- **Both lowerers collect.** The reference lowering is an independent
  module, not a mode ([compiler.md](compiler.md), "The reference
  lowering"), so both lowerers collect. Every entry point of
  `compiler/mod.rs` takes a `collector: Option<&MetadataCollector>`;
  each lowerer records a body's tables as it finishes the body, keyed by
  the body's identity (`EncodedBody::identity`), so that a family's
  blocks, which the compilation hands back only inside their creators'
  `MakeClosure`s, are found by their code; the kernel's unwind-marker
  bodies, cloned once compiled, move their entry (`rekey`). Without a
  collector each lowering emits exactly the code it emits with one. A
  prologue entry's statement is its clause's span, a `<:` clause check's
  the clauses' spans merged, a superclass initialization no clause
  writes the declaration's name, and a slot's initializer its slot
  declaration; a `SendMirror` has a send entry without a call site, a
  `ResolveFuture` and an initializer invocation none. A body past the
  compiler's limits, which raises `Literal::BodyTooLarge` first, records
  no tables: it is not the source's body. It drops the blocks it compiled
  before it found itself too large, and their tables with them
  (`MetadataCollector::forget_family`): the collector holds the tables
  of no body but those the compilation still holds, since a body that
  records none -- one past a limit -- allocated where a dropped one was
  would otherwise be answered the dropped one's.
- **What is compiled: the declaration's level.** A frame's body is named
  by a `BodyOrigin` (`stack::origin_of`, "Reading the stack at a stop"):
  the version of the mixin whose member holds the frame's body --
  possibly a version an edit superseded, which keeps its id -- and the
  member: a method or a lazy slot's getter, an initializer body, the
  superclass clause or the factory.
  `again::compile_level` finds the declaration's path from its module's
  root by that version's id (`Heap::known_mixin_id`,
  `Mutator::path_from_module_root`; a class side's through its instance
  side's), parses and resolves the module's current text
  (`Heap::module_source`) with `resolver::resolve`, as
  `interpreter/edit.rs` prepares an edit, finds the declaration
  (`edit::find_declaration`) and compiles its level alone -- its methods,
  its initializer bodies, its clause and its class side, never its nested
  declarations -- through `Mutator::mixin_level`, the function that builds
  a level when a module loads, given the collector and no nested
  declarations, so that the level compiles again exactly as it was
  built. It registers nothing -- no mixin id, no code id -- and runs
  nothing. A module without text, or a text that does not parse, does not
  resolve or holds no such declaration, answers no metadata, never a
  panic: an image's text is untrusted input.
- **Pairing.** Each running body of the level -- the frame's side's, and
  for an instance side its class side's too, a declared one or the one
  synthesized for a declaration that declares none, which holds the
  factory alone -- is paired with the body compiled again in the same
  place: a method by its selector, the three initializer bodies and the
  clause by their fields. The running version's bodies are paired, not
  the current version's, so that a superseded version still running
  takes the metadata of the current text when the two compile alike, and
  none when the edit changed it (decision 11). A family whose root
  compiles alike pairs its blocks with the blocks compiled again in
  creation order (`created_blocks`), walked by a worklist rather than a
  recursion, since an image's nesting is not bounded by the text; a
  family whose root does not compile alike gets no tables at all, since a
  block's tables name its creators' registers, which only an equal root
  vouches for.
- **What "equal" means.** `bytecode::compiles_alike`: equal kind, counts,
  `local_count`, `family_size`, `outer_reach`, own-slot addressing,
  `statement_program_counter` and instructions
  (`Instruction::is_identical_to`), a `MakeClosure`'s block compared by
  the same function; `source`, the unwind marker -- an image's body
  carries none, and where it is is decided by the instructions compared
  -- and the caches are ignored. `CompiledCode` is destructured there, so
  that a field added to it must be decided. Planned with breakpoints
  (stage 3): a patched slot compared by its original ("Where the
  original lives").
- **A factory's body** (decision 12) takes the metadata `factory_again`
  makes from the class header: the header's parameter names in the
  registers a factory takes its arguments in, no statement start and no
  send, and as its place the header's factory pattern, or the class's
  name for a header that writes none. Any body that compiles alike with
  `compiler::factory_body` of the header's arity takes it, a factory body
  no mixin holds -- a host's `instantiate` builds its own -- included.
- **Which lowering.** The cache compiles a level under the interpreter's
  lowering first, then under the other one only when the first left a
  body of the level unpaired, and keeps every pairing either found,
  never replacing one by the other's or by none. An image does not
  record which lowering compiled its code, so its bodies may pair under
  either (decision 5). A program read from source takes the other
  lowering's pairing too: a pairing still requires `compiles_alike`, so
  the tables are right whichever lowering made them, and the reference
  lowering's sends carry the uncached call site, so that a body with a
  send compiles alike only under the lowering that made it. A level with
  a body neither lowering pairs -- a superseded version an edit changed,
  a body past the limits -- is compiled twice.
- **The cache** (`metadata::Cache`, `Mutator::debug_metadata`, on the
  mutator so that the pause stress mode, which runs with no debugger
  attached, reads through it too) keeps each body's metadata, or `None`
  for a body that pairs with none, keyed by the body's address, for every
  body of the level compiled, so that the other frames of the level
  compile nothing. It holds each body weakly: a strong hold would keep
  every body the debugger ever looked at alive, and with it its inline
  caches' family (`InlineCaches::forget_families` frees a family only
  once the family alone holds its body); a `Weak` still keeps the body's
  allocation, so that no other body takes its address while the entry
  lives, and the entries of dead bodies are dropped at the next miss. It
  holds no mixin, so that a superseded version is dropped once nothing
  runs it. Every install clears it (`interpreter/install.rs`): an
  install changes the module's text, which every span counts in, and may
  replace what the bodies pair with.
- **Shipped bodies.** A shipped module's body -- its module's root
  decides, since a class side synthesized for a shipped declaration is
  not marked shipped -- is compiled again from the shipped resolution
  the boot made, cached per thread (`interpreter::kernel::resolved_kernel`,
  `core_library::resolved_shipped_module`), and the text the binary
  embeds, so that its `SendMirror` sites are the same and the bodies can
  be equal; there is no new caller of `resolver::resolve_kernel`, and a
  program's text is resolved with `resolve`, so that no text compiled
  again records a mirror send. The first lock of the VM mirror permits a
  shipped module resolved with it (`CLAUDE.md`, "The VM mirror is a
  capability"); still, the result is only compared and its tables kept,
  never installed, run or handed to anything but the metadata cache.

### Values the IDE holds

**Partial:** a stop holds the values it hands out and answers an
instance's fields and an `Array`'s elements, paged. Holding a value past
its stop, the elements of a `ByteArray` and a `String`, and a Newspeak
collection's renderer are planned (below).

Every value a stop describes (`values::StopValues::describe`) is held as a
`Root` (`Heap::stash`) in the stop's table and named by a `ValueId`, its
index there offset by the table's first id. The table goes with its stop,
releasing every `Root`, when the program goes on or ends. The ids
continue across the stops of one attachment (`Debugger::next_value_id`,
`StopValues::next_first`), so that an id of an earlier stop, whose table
is gone, names nothing in a later one rather than another value, and is
refused (`a_value_id_does_not_outlive_its_stop`); they wrap only after
2^32 values handed out. A `ValueDescription` carries the id, the label
`Heap::describe` gives -- never `{:?}` -- the name of the value's class
(`Heap::class_display_name`) and how many children it has.

`Children` answers, from `start` and at most `count` of them: for an
instance, its fields, named `field 0` up (`Heap::instance_field`;
`an_instance_s_children_are_its_fields`); for an
`Array`, its elements, named `[1]` up as `at:` counts
(`Heap::array_element`; `an_array_s_children_are_its_elements_paged`).
Every other value has none: an immediate, a `ByteArray`, a `String`, and
a reified mixin, whose one field is an id that tells a reader nothing.
Each child is described, and held, as every value is. The heap's accessors
alone describe a value: no Newspeak code runs while the program is
stopped.

Planned: an object the IDE pins outlives its stop, as JDWP's object ids
and their `DisableCollection` do; a `ByteArray`'s and a `String`'s
elements are read by the heap's accessors and paged; a Newspeak collection
(`List`, `Map`, `Set` of `CollectionsFactory.ns`) shows its slots until
evaluation exists, then a renderer that runs Newspeak code (`size`, `do:`,
`printString`) under a limit of time and sends, skipping breakpoints as
every evaluation does.

### The pause stress mode

The pause stress mode (`NS_NEXT_STRESS_PAUSE`, debug builds, one of the
`stress` gate's thirteen modes; `Interpreter::set_stress_pause` for a
test, `Interpreter::stress_pauses` the count of its stops) stops the run
at the safepoints its schedule picks -- paced by the traced size as the
collect and suspend modes are (`StressSchedule`,
`interpreter/safepoint.rs`) -- exactly as a pause does: right after the
debugger is served, before anything else at the safepoint changes the
stack or the heap, a frame its optimized code's poll left deoptimized
first. No debugger need be attached: the stop takes its commands from a
`CommandSource`, the attached debugger's channels or the mode's reader
(`debugger::StressReader`), which asks for the stack trace, the variables
of every frame and the first eight children of every value a variable has,
then continues; a refusal is the stop misreading its own stack, and
panics. The stack trace and the variables are read through the metadata
as an agent's are, so every frame's body is found by its declaration and
compiled again on the first stop in it, and its name, its location and
its named variables, the outer chain of a block included, are read at
real stops; a variable not available has no value to ask children of.
While the mode is on it holds the stress bit
(`hold_safepoints_for_stress`), so that every poll comes to the
safepoint. Every golden and the corpus run under it, so reading a stopped
stack is shown never to change a program's outcome nor to read a register
the leftover analysis calls dead, which a debug build refuses -- a
variable table that named a dead register included;
`the_pause_stress_mode_keeps_outcomes` (`tests/stress.rs`) holds a program
to its outcome under both lowerings with the mode on and off.

### Testing and gates

**Partial:** stage 1's and stage 2's tests and the pause stress mode
exist; the model test of the poll words' ordering, and every later
stage's tests, do not.

- **The debugging API's tests** (`crates/ns-runtime/tests/debugger.rs`):
  a pause before the run and from another thread, continue and
  terminate, the `ensure:` blocks a termination runs, a `Terminate` queued
  while running, a remote dropped before the run and while stopped, a
  pause while a collection is requested, a stop-only command refused while
  running, a replaced remote's pause and a pause asked before the
  replacement, a `Terminate` sent after its run ended and one queued after
  the one that ended its run, the stack's frames, its suspended senders
  under the suspend stress mode and their variables, and a frame's
  arguments, an instance's fields and an `Array`'s children paged, a value
  id that does not outlive its stop, a frame index past the stack
  refused, a pause that stops compiled recursion at a poll of baseline
  machine code, and a stop at a poll of optimized code deoptimized; where
  the JIT's counters are exact, the last two assert the machine code they
  name. `heap.rs`'s unit tests
  hold the bits: a debugger pause lowers the depth limit, serving the VM's
  request keeps the debugger's bits, a stress hold lowers nothing, and
  clearing the debugger's bits gives the limit back.
- **The metadata at a stop** (`tests/debugger.rs`, each under both
  lowerings but the image without source's): a frame named and located,
  the send it waits on included; a frame's named variables; a block's
  view of its method's variables; an outer variable of a creator that
  returned without keeping its registers, not available; a block two
  deep reading its method's variable two captured frames out, while the
  method runs and after it returned; a returned creator's frame an
  image decoded, read through the closure that reads it by reference; a
  factory's frame named by its header's parameters; a superseded body its edit changed, shown
  by number and without a location, its block's frame still named for
  its method; a superseded body an install into its declaration left
  alone, and its block's, named, located in the current text and showing
  their named variables; an image without source, its frames
  named and their variables numbered; and a vector variable's current
  value in every frame of its chain.
- **The collection and the pairing** (`debugger/metadata/tests.rs`):
  statement starts, an inlined block's included, and their spans; a
  send's call site, receiver and selector, an implicit receiver's, a
  setter's, a cascade's, an integer fallback's and a conditional
  fallback's; a block's whole lexical chain, a shadowed name, and a
  variable an enclosing block was handed; an initializer's prologue,
  slots and lazy getter; both lowerings naming the same variables, sends
  by span and initializer statements, a block two deep's included;
  either lowering compiling without a
  collector emitting exactly the code it emits with one, so that no
  golden, no shipped-code fingerprint and no image changes; every body of
  a level compiling again alike, an unwind marker's body included; a
  body past a limit leaving no tables of the blocks it dropped, and no
  such body of a level getting tables;
  `compiles_alike` ignoring spans but not instructions; an install
  moving the spans of a body it did not edit; a reference-lowered
  image's body paired under the reference lowering; and a crafted image
  text without the declaration giving no metadata.
- **The oracle** (decision 14), `Interpreter::check_debug_metadata`
  (`debugger/metadata/check.rs`, `#[doc(hidden)]`): every body of every
  loaded declaration compiled again, required alike, and its tables, read
  through the cache as a stopped frame reads them, held to their
  invariants -- every statement range within the body, every
  `statement_program_counter` range starting at a statement start, every
  send entry a send of exactly its call site and every send named by
  one, every variable's registers below the reading body's `local_count`
  and every hop within its lexical depth, every span within the module's
  text. Only a body past a compiler limit, which raises
  `Literal::BodyTooLarge` first and records no tables, may have none.
  `crates/ns-cli/tests/debug_metadata.rs` runs it over every golden under
  each lowering, from source and without running it, in the default
  suite (`debug_metadata::<lowering>::<path>`), a golden whose modules do
  not resolve passing unchecked, and `debug_metadata::floor` requires
  at least 500 goldens, at least four of them projects, to resolve, so
  that a drift in how a golden's modules are read cannot stop the check
  silently;
  `every_shipped_body_compiles_again_alike` (`tests/debugger.rs`) runs
  it once over the shipped modules, the lazy ones built first; and
  `golden_round_trips.rs` runs it over every golden's artifact `build`
  wrote under each lowering and `load` decoded, in the profile `images`,
  where the artifact's text pairs under the interpreter's lowering or the
  other. Its own tests (`check.rs`, `a_program_s_bodies_are_checked`
  and `a_broken_table_fails_the_check`) check a program's bodies and fail
  a broken table, and `debugger/metadata/tests.rs`'s
  `a_body_that_compiles_again_otherwise_fails_the_check` fails an
  image's body whose text, patched, compiles otherwise.
- **The breakpoints' tests** (`tests/debugger.rs`, each under both
  lowerings with the JIT off): a line breakpoint resolved to its
  statement and stopping there, the stopped frame located at it, the
  run's outcome unchanged; a breakpoint in a loop stopping at every pass;
  the breakpoint a stop stands at removed, its original run once; an
  inlined block's breakpoint stopping in its home and in its fallback
  closure; a breakpoint set while the program runs; a pending breakpoint
  resolved when its module loads, and one in `TranscriptFactory`
  resolved when the program first asks for it; an install setting the
  breakpoint again on the new body; and a patched body keeping its code
  id across a full collection. With the JIT on, under both lowerings: a
  breakpoint in a loop that runs in machine code freeing the code and
  stopping at every pass, each pass running its statement once, the
  exit at the patch taken; a breakpoint in a callee an optimized copy
  inlined freeing the copy; a breakpoint in `True>>ifTrue:`, a method
  with a fast path, stopping a send that machine code's cell answered
  without activating the method before, and removed at the stop putting
  the fast path back; and a body compiled again once its breakpoint is
  removed. Where the JIT's counters are exact, under the optimized
  lowering, they assert the machine code these build. The helper
  `debugged` queues an agent's configuration before the run and serves
  the run from another thread; `events_during` collects a run's events
  and continues any stop, for a run that must not stop.
- **The steps' tests** (`tests/debugger.rs`, each under both lowerings
  with the JIT on, the bodies run twice before so that they have machine
  code, each starting from a line breakpoint): a step over stopping at
  the frame's next statement; a step into stopping at the callee's first
  statement, and at the first statement of a callee whose first
  instruction makes its indirection vector; a step into a send a
  primitive answers stopping at the next statement; a step out stopping
  at the caller's return point mid-statement, reporting the callee's
  answer; a step over reporting the value of the last call; a step over
  a recursive send staying in its activation, then a step out of a
  deeper one stopping in its caller; a step over a non-Boolean receiver's
  `ifTrue:` stopping in the fallback block and going on in its creator;
  a step over at a method's last statement stopping at the caller's
  return point; steps over and steps into a loop stopping at each of
  its statements once per pass; a step into taken in a fallback block
  going on in its creator, past the receiver's `ifTrue:`; a `Terminate`
  while a step into is pending ending the step, the `ensure:` block the
  unwind runs making no stop; and a step over begun at a pause at a push
  stopping at the callee's next statement, not where it stands. Of a
  frame that leaves otherwise (decision 17): a step out of an `ensure:`
  block a non-local return's unwind runs, and a step over a non-local
  return out of a block a `do:` runs, each stopping at the home's
  caller's return point with the home's answer; a step out of a
  future's computation stopping in the frame that met the future at its
  next statement once the instruction ran again, past the forcing's
  shipped frames -- a send, whose answer is reported, and a value's slot
  initializer, which no return point follows -- and with the shipped
  filter off stopping in each of the forcing's frames on the way; a
  step out of a superclass clause stopping at the accessor's return
  point; and a step out of a clause whose answer is refused as no class
  stopping past the handler, not in the `signal`: past the shipped
  `on:do:` too, at the caller's return point of the method that sent
  it, and with the shipped filter off at the `on:do:`'s return point.
  Of the step filters (decision 16): a step into a `do:` over an array
  landing in its block; a step over at the block's last statement
  landing in its next activation, and after the last element at the
  statement after the `do:`; with the shipped filter off, a step into
  the `do:` stopping in `Kernel`; a filtered program class stepped
  through to the caller's block it runs, and the same class unfiltered
  stepped into; a breakpoint in `Array>>do:` stopping, and a step over
  begun there stopping at its next statement; and a step into a send of
  `ifTrue:` with a literal block making the same stops under both
  lowerings; a step out of `Array>>do:`, stopped at a breakpoint there,
  landing at its return point in the unfiltered caller, and one whose
  caller `inject:into:` is filtered too landing at the next statement of
  the first unfiltered frame, reporting `inject:into:`'s answer; and a
  step into a send whose callee pushes a chain of filtered frames
  landing in the program's block at its end. Beside them, a breakpoint at an
  instruction that meets an unresolved future stopping once, its slot
  initialized once: only the optimized lowering puts the instruction
  that meets it first in its statement, since the reference lowering
  begins every slot initializer's statement with a `Move`. A step over a
  statement that raises, stopping in the handler block, is written and
  ignored until the handler search reports to the stepping state. Each
  also passes under every stress mode, the suspend
  stress mode's suspensions included.
- **Run to cursor's and the method breakpoints' tests**
  (`tests/debugger.rs`, each under both lowerings, with the JIT on but
  for the pending method breakpoint and the shared stop): run to cursor
  stopping at a later statement as a step's stop, once; run to cursor
  refused, with its reason, for a position past every statement and a
  module not built; a method breakpoint stopping at its method's entry
  and at each return it takes, the method's own and one in an inlined
  block, and nowhere else; an exit by the unwind of a non-local return
  through the method, and the exit of a home its own block returns
  from, each stopping at the next place the program reaches; a method
  breakpoint on `True>>ifTrue:`, a method with a fast path, stopping at
  its entry, removed putting the fast path back; a pending method
  breakpoint verified when its module loads, then stopping at entry and
  exit; a stop that is a method's entry and a line breakpoint's
  reporting the entry with the line breakpoint among its others; and a
  method breakpoint on a lazy slot's getter an optimized copy inlined
  freeing the copy, every read of the filled slot stopping at the
  getter's entry and exit; a method breakpoint on a class-side method
  stopping at its entry and at each return, the one in its inlined block
  included; an install setting a method breakpoint again on the new
  body; and, under the JIT stress mode, a method whose loop's on-stack
  replacement runs it in machine code reporting the exit its own block's
  non-local return makes. A step into whose push stops at a statement a
  line breakpoint stands at reports the breakpoint. Beside them, under
  the JIT: a copy compiled
  while a callee or a block it would inline is patched calling the
  callee and making the block's closure, so that every pass stops once
  the breakpoint stands at a statement a pass runs; sending the same
  line breakpoints, or the same method breakpoints, again freeing no
  machine code; and a line breakpoint set into `True>>ifTrue:` while a
  step's one-shot already patched its body still switching the fast
  path off, the callee's loop sending `ifTrue:` with a block until the
  breakpoint stops it, so that the stop does not depend on when the
  breakpoint is served.
- **The pause stress mode** (above).
- **The ordering of the poll words** is held by review, by the threaded
  race test `no_pause_is_lost_to_a_racing_depth_limit_write` (`heap.rs`),
  which runs the agent's two writes against the VM's writes of the limit
  for 10,000 rounds on whatever processor runs the suite, and by the
  `aarch64` gate, which runs the suite on AArch64's encoding of these
  accesses. Neither proves it: a race test only samples interleavings,
  and QEMU's user mode does not reproduce AArch64's weak ordering. A
  model test of the agent and the two writers in the style of `loom`,
  which would explore every interleaving the memory model allows, does
  not exist; it needs `loom` as a dev-dependency, to be justified when it
  is built ([OPEN-WORK.md](../OPEN-WORK.md), "A model test of the poll
  words' ordering").
- Planned: **a breakpoint stress mode** patches every statement start of
  every body a program runs, goes on at each, and holds every golden's
  outcome under both lowerings, with the JIT and with `NS_NEXT_NO_JIT=1`;
  a **step stress mode** steps into everything; both join the `stress`
  gate.
- Planned: **debugger goldens**, a `.ns` with a script of commands and the
  stops, stacks and values expected, run by a scripted DAP client in
  `ns-cli`'s tests.
- **The `codegen` gate** holds the run loop with the `Cell` slots and
  the `Breakpoint` arm to its budgets: the arm shares the out-of-line arm
  of the simultaneous slot clauses, since an arm of its own grew
  `vm::run` past its budget ("Stopping at and going past a breakpoint").
  Planned: a measurement (the `measuring-performance` skill) that
  `--debug` with no breakpoint runs at plain speed. The `linux` and
  `aarch64` gates already run the atomics on both architectures.
- Planned: **the plugin's tests** drive its `XDebugProcess` against the
  agent.

### What stage 1 moved

Stage 1 puts an atomic `Relaxed` load at every safepoint poll and moves
the run loop's safepoint handling into a cold `take_safepoint`. It was
measured against `master` in one sitting on 2026-11-11, by the protocol
of "What floats in machine code moved" in
[execution.md](execution.md): three pinned builds a side with the zone
paddings, three rotated rounds, every process held to the eight
performance cores.

The geometric mean over the twenty-three sources is -0.61%; by build
pair it is -0.39%, +0.96% and -2.37%, by round -0.18%, -0.50% and
-1.14%, and the two sides' builds interleave. The seconds-long programs
(CD, Havlak, Mandelbrot, NBody, DeltaBlue) are within 0.5%. Three
programs lie outside 3% at eight iterations and none is separated
slower: Towers (+4.9%, two single slow rounds on the branch; +1.2% at
forty iterations), RichardsTon80 (-4.0%, separated faster; -0.9% at
forty) and `bench_recursion` (-10.1%, the bimodal medians that both sides
show; -0.5% at forty). Their sub-millisecond medians scatter, and the
forty-iteration run, which confirms none of the movements, shows them to
be noise. The one separation toward slower is Permute at forty
iterations, +1.4% on a 75 microsecond program (+0.8%, +0.9% and +2.6%
by build; +2.0% and not separated at eight); it stays unexplained, and
a sweep of zone paddings could look at it.

Not covered: the cold `take_safepoint` path with safepoints actually
requested, since no program ran with a debugger attached or the pause
stress mode, so only the poll on the ordinary path is timed; Linux,
AArch64 and Windows on ARM; start-up, memory and `ns-next load`; and the
poll's cost inside machine code beyond what these programs exercise.

### What stage 2 moved

Nothing on the hot path, so stage 2 was not measured. The lowerers take
an optional collector, and without one emit exactly the code they emitted
before (`compiling_without_a_collector_emits_the_same_code` and its
reference twin): every golden's outcome, the shipped code's fingerprint
and `tests/fixtures/golden-image.bin` are unchanged, and compiling
outside a debugger's stop collects nothing. The divorce that copies a
frame's registers into its frame object, already the cold divorce,
writes `true` rather than nil into the married word; machine code's
inline divorce, which takes only a frame that keeps none, is unchanged.
Everything else stage 2 adds runs only at a stop.

## The decisions

Decisions 1 to 9 were decided with the user on 2026-11-10, decisions 10
to 15, stage 2's, on 2026-11-12, and decisions 16 and 17, stage 3's, on
2026-11-12, one at a time, each after the
specification, the prior art (`prior-art/jdk`, `v8`, `opensmalltalk-vm`,
`primordialsoup`, `newspeak`, `somns`) and the repository's invariants.
Each states the choice and the reasons the alternatives were rejected; the
mechanisms each implies are in "Planned: the mechanisms". Decisions 7
and 10 to 15 are built, and of decisions 1 and 9 what pausing, reading
the stack, ending a run and setting breakpoints need: the debugging
API's first part, and the stop waiting at the safepoint ("The debugging
API at the safepoint"); of decision 2 the patching and the interpreter's
stop at a line breakpoint; of decision 4 the tables, and of decision 5
the compilation again, the comparison and the cache ("Compiling the
metadata again"), which a stop and a breakpoint's resolution read.
Everything else they decide is planned, and is written below as the
design to build.

### 1. One debugging API in Rust, two clients over it

**Partial:** the API's first part exists in `crates/ns-runtime/src/debugger/`
-- attaching, pausing, the commands served while running, the stop,
reading the stack and its values, continuing and terminating ("The
debugging API at the safepoint") -- and its only clients are the tests and
the pause stress mode; the rest of the API and both clients are planned.

Every mechanism -- stopping and going on, the breakpoint instruction, the
stepping state, reading and writing frames, the metadata, deoptimization --
sits, or will sit, behind one debugging API of `ns-runtime`. It will have two clients: the
remote agent in `ns-cli`, which speaks the protocol to an IDE and is built
first, and `ActivationMirror` in `MirrorsFactory.ns`, through new mirror
leaves, which gives a program the access to activations §8.2 asks of
mirrors. Evaluation and edit-and-continue will run Newspeak code through
the existing resolver, compiler and install.

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

**Built.**

A breakpoint replaces its slot of `EncodedBody::slots` by an
instruction `Breakpoint`, and keeps the original beside the slots, on the
body ("Where the original lives" below). The interpreter stops at it and,
going on, executes the original. The slots are interior-mutable
(`Cell<CompactInstruction>`), written only through one narrow API
(`CompiledCode::patch`, `restore`); the run loop's fetch copies a slot
out as it did before. Setting or removing a breakpoint frees the body's
machine code and every optimized copy that inlined it
(`jit::free_for_patch`); the baseline lowers a patched index as an
exit, so every other body keeps running in machine code.

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

**Partial:** step over, step into and step out are built
(`debugger/stepping.rs`; "Step over in recursion and blocks", "Step
into's held request", "The return value of the last call"), the frame
left by a return to its caller's return point, and the reports of a
frame left otherwise, by an unwind or a continuation other than
`Deliver` (decision 17), the step filters (decision 16), and run to
cursor; the handler search's report (decision 17, "Exceptions") and
smart step into are planned.

- **Step over** sets one-shot breakpoints at every statement start of the
  frame's body (V8's `FloodWithOneShot`, `v8/src/debug/debug.cc:1364`) and
  one at the stepping frame's **return point**: the index after the
  instruction its caller stands at -- a send, a `ResolveFuture`, an
  initializer invocation or a `SendMirror` -- never that slot itself, which
  `resume_caller` reads again. A frame left by an unwind -- an exception, a non-local
  return, a `retry` -- is reported by the unwinder as it pops it
  (`Mutator::pop_for_unwind`), and a frame whose continuation is other
  than `Deliver` by the run loop's arm for it (`breakpoint::frame_left`);
  both are cold paths, and the return's own path is unchanged. The step
  then goes on as a step into from the frame the program goes on in
  (decision 17).
- **Step out** is the return point's one-shot breakpoint and the
  reports of a frame left otherwise.
- **Step into** is a step over plus a safepoint request held until the next
  push, which the debugger's safepoint stops at, at the new body's first
  statement start. A send answered by a primitive or a leaf pushes
  nothing, and the next one-shot breakpoint stops.
- **Smart step into** is a step into whose safepoint stops only at a push
  from the chosen `CallSite`.
- **Run to cursor** (`Command::RunToPosition`) is a one-shot breakpoint
  at the target, resolved as a line breakpoint is
  (`breakpoints::run_to`) and taken in any activation, since the cursor
  names a place, not a frame: a kind of the breakpoint table's
  (`PatchKind::RunTo`) that the next stop removes with a step's
  one-shots, whatever made it, its own included. Its stop is reported as
  a step's (`StopReason::Step`), as IntelliJ and the DAP report it; a
  breakpoint the debugger set at the target makes the stop the
  breakpoint's. The frame the command is given in goes past the slot it
  stands at once, as a step's frame does, so that a target where it
  stands stops at the next pass.

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

**Partial:** the tables are built (`debugger::metadata::BodyMetadata`,
"Compiling the metadata again"), a stopped frame's location reads them,
and line breakpoints and steps stop at their statement starts and
return points; smart step into's targets and IntelliJ's stepping by
line, the agent's, are planned.

The metadata of a body lists:

- every **statement start** -- the body's own, every block's and every
  inlined block's, and every initializer, slot and prologue entry
  `statement_program_counter` gives a range -- with its source span. A
  line breakpoint, a step over and a step into will stop only there. A
  line breakpoint sits at the first
  statement beginning on its line, or the next line that has one, and each
  block body beginning on the line is a variant of it.
- every **send**, a `SendMirror` among them: its instruction's index, its
  `CallSite` (none for a `SendMirror`) and the span of its receiver
  (none for an implicit receiver and a setter assignment) and of its
  selector's tokens, for smart step into's targets and for
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

Built ("Compiling the metadata again"; a breakpoint the IDE sets is
stage 3's). Neither table is stored. The first time the debugger needs a
body's -- a stop in it, or a breakpoint the IDE sets -- it compiles the
body's source again, the lowerer collecting the stop locations and the
**variable table** on the side: each named variable's place (a register,
a register of an enclosing frame, a slot of an indirection vector), over
the whole body, since an inlined block declares no variable of its own
(decision 13). The result is compared with the running body; only an
equal body gets the tables, cached by the body. A body whose source is
gone (an image built with `--strip-source`) or whose recompilation
differs (code an older binary compiled, a superseded version an edit
changed) gets none, and its frames show without variable names and
source positions, as Java's do for a class compiled without `-g`; a
frame keeps its name, which its declaration gives.

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

Built ("The poll words as atomics", "The arming lock"). The machine words
are `AtomicU64`s that the agent's thread reaches too; every hot access of
the VM's is a `Relaxed` load or store, the instruction a `Cell`'s was on
x64 and AArch64, so neither the interpreter's poll nor machine code's
changed. The agent sets a pause bit of its own in the request word and
lowers `DepthLimit` to 0, so that the next poll anywhere -- interpreted or
compiled, a push or a loop's head -- takes the safepoint. The agent's two
writes and the VM's clearing of a debugger bit, which resynchronizes the
limit, hold one lock per interpreter (`debugger::Arming`), so that no
clearing falls between the two writes and leaves the limit at 0 with no
bit set; the lock also says which attachment is current, and a remote
whose debugger was detached or replaced arms nothing. Ctrl-C and the
actors' scheduler, both planned, will arm the same word.

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

**Partial:** a pause's stop is built ("Serving at the safepoint", "The
stop"), serving the stack and values; a breakpoint's and a step's stop,
setting breakpoints and evaluating are planned.

A breakpoint, a step's stop or a pause leaves its stretch for the run
loop's safepoint, as a collection request does, and there calls the
debugging API, which blocks on a channel and serves the agent's commands --
reading the stack and values, setting breakpoints, evaluating -- answering
on another; no handle is live there, and nothing pushes or pops while it
waits. Evaluating will push the expression's activation and
return into the loop, its return carried back into the stop by a
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

### 10. A block's frame shows its whole lexical chain

Decided with the user on 2026-11-12, for stage 2, as decisions 11 to 14
are. A block's frame shows every variable in scope at its stop: its own
parameters and temporaries and those of every enclosing block and of its
method, the innermost of a name shadowing the others. Each is read where
it lives for that frame: a copied value from the block's own register, a
vector variable through the vector the frame holds, and a variable the
block did not capture from the register of the creating activation,
reached through the closure's `captured_frame` chain, while that
activation still runs or kept its registers; a variable that lives
nowhere any more -- its activation returned without a divorce that kept
its registers, and nothing captured it -- is shown as not available, as
V8 shows a variable optimized out. Under the reference lowering every
outer variable is read by reference, so the two lowerings show the same
names, differing only in which are available after the creator returned.

Primordial Soup does the same: its `DebugInfo` lists every variable of
the family with its `contextDepth`, and `localsDo:` shows each of a depth
at most the frame's, read through `enclosingActivation`, a returned
context's as `nil`
(`primordialsoup/newspeak/Newspeak2PrimordialSoupCompilation.ns:647-698`).
Showing only what the block's frame holds, as JDWP shows a Java lambda's
captured values and V8 a closure's context-allocated variables, is
rejected: it would show closure conversion, an implementation detail,
differently under each lowering, and evaluation (decision 8) needs the
whole chain anyway.

### 11. A body is found by its declaration and equal by its code

A body compiled again is found by its declaration's path -- the names
from the module's root class, the side, the selector or the kind of
initializer, and for a block its place in its family -- and is equal to
the running body when its instructions and side tables are (decision 5),
whatever its `source` span. Every span of the metadata is in the
module's current text (`Heap::module_source`), which the compilation
reads. So a body an edit elsewhere in its module moved keeps its
metadata, at the place its text now stands; a superseded version still
running gets the metadata of its current version when the two compile
equal, its text then unchanged in the current module, and none when the
edit changed it. Primordial Soup likewise compiles a method's current
source again (`compileMethodSource: rawMethod source`,
`primordialsoup/newspeak/Mirrors.ns:2719`).

Requiring the span to be equal too is rejected: every body after an
edited declaration would lose its names and lines, which under hot swap
(stage 6) is the common case. Answering spans in the coordinates of the
text the body's version was read in is rejected, as rebasing spans was
for the image ([object-model.md](object-model.md), "Source retention and
provenance"): the agent would have to map each frame's text onto the one
the IDE shows.

### 12. A factory's frame takes its metadata from the class header

A factory's body is synthesized from its arity (`compiler::factory_body`)
and has no source, yet stands on every stack between the sender of `C
x: a` and `C`'s initializer. Its frame takes its metadata from the class
header: its arguments are named as the factory's parameters in the
header, it stands at the header's factory message pattern, and it has no
statement start, so no step stops in it. Its body is equal by
construction, `factory_body` of the same arity. Primordial Soup's
`computeDebugInfo:` takes a factory's from the class header the same way
(`primordialsoup/newspeak/Mirrors.ns:2756-2761`). Showing it without
names or position is rejected, since it would be the one frame of an
ordinary program without a place; hiding it contradicts the
specification's "accurate and complete stack traces" (§3.6,
`newspeak-spec.pdf` p. 17) and would need rules of its own for step out,
restart and force return.

### 13. A frame shows named variables only

A frame shows `self` and its named variables; a scratch register is never
shown as a variable, as JDWP does not show the JVM's operand stack, V8 no
intermediate value and Primordial Soup no synthetic local
(`isSynthetic`, `Newspeak2PrimordialSoupCompilation.ns:693-698`). What a
scratch register holds at a stop differs between the lowerings -- the
reference lowering never reuses one, the optimized one resets them at
every statement -- and the return value of the last call reads its
send's destination register itself ("The return value of the last
call"). Showing every scratch register the leftover analysis does not
taint, and doing so on request, are rejected. A temporary is `nil` from
its scope's start, so a variable is shown over its scope's instruction
range and not outside it; there is no "not yet assigned". A scope that
is not a frame's -- an inlined block's -- declares no variable, so every
variable a frame shows is in scope over its whole body, and the table
records no range.

### 14. The metadata is checked on three levels

- Targeted tests at a stop, in `crates/ns-runtime/tests/debugger.rs`,
  each under both lowerings but the image without source's: named
  variables; a block's outer chain, copied, in a vector, by reference,
  two frames out, after its creator returned, and from a creator's
  frame an image decoded; a factory's frame; a superseded version an install into its declaration
  left alone, and one the edit changed; and an image without its source.
  Unit tests in `debugger/metadata/tests.rs`: a body after an edit of
  another declaration (`an_install_moves_the_spans_of_an_unedited_body`)
  and an image whose code the reference lowering compiled
  (`a_reference_lowered_image_body_pairs_with_the_reference_lowering`).
- An oracle over every body: a test compiles every body of every golden
  again under the lowering it runs under, both of them, and requires it
  equal, and holds the tables to their invariants -- every statement
  start within the body and at the start of every
  `statement_program_counter` range, every send entry naming a send of
  exactly its `CallSite`, every variable's register below `local_count`
  and its range within the body, every span within the module's text;
  the shipped modules' bodies are checked once by a test of their own,
  not per golden; the `images` gate runs it through `build` and `load`,
  which checks the trial of the optimized lowering and then the
  reference lowering.
- The pause stress mode's reader asks for every frame's name, position
  and named variables through the tables, its outer chain included, so
  that every golden and the corpus read named variables at real stops,
  and a debug build refuses a table that names a dead register.

A stress mode of its own that asks for every pushed body's metadata is
rejected: it covers only the bodies that run, which the oracle covers
without a stress run.

### 15. A send's selector has spans in the syntax tree

The syntax tree keeps the span of every selector token: a `MessageSend`
carries `selector_spans`, the one token of a unary or binary selector or
each keyword of a keyword selector, which the parser fills as it reads
them, so that a send's entry in the metadata names its receiver's span
and its selector's (decision 4) without either lowering knowing the
grammar. The tree drops only layout ([frontend.md](frontend.md), "AST"),
and a keyword's place is not layout. A unary send to an implicit
receiver, which the tree keeps as an `Identifier`, and a setter
assignment take the identifier's span. Lexing the message's text again
when the metadata is collected is rejected, as a second copy of which
tokens make the selector; a span of the whole message instead of the
selector's is rejected, since smart step into marks the selector.

### 16. Shipped code is filtered from steps, and program classes can be

**Built** (`debugger/stepping.rs`: `is_filtered`, `went_on_past_filtered`,
`continue_as_step_into`, `held_step`; `Command::SetStepFilters`). A
frame is filtered when its body is a member of a declaration, or a block
of one, whose module's root is shipped while the shipped filter is on,
or whose path from its module's root begins with that of a `ClassName`
the client sent (`again::declaration_path`); the declaration is the one
the frame's metadata is looked up by (`stack::origin`), a class side's
its instance side's, and a frame of no declaration, the top level, is
never filtered. A stop the step's own one-shot makes, where no line
breakpoint stands, is decided at the safepoint (`serve_debugger`): in a
filtered frame the step was not begun in, nor in a block lowered inline
in that one, it is not made, and the step goes on as a step into from
the first unfiltered frame below (`went_on_past_filtered`), the frame
past its slot once; every other way a step goes on as a step into -- a
frame an unwind or a continuation other than `Deliver` left (decision
17) -- likewise starts from the first unfiltered frame at or below the
one the program goes on in, through suspended senders. A step into's
held request counts as its push an unfiltered frame whose sender is the
stepping frame, a block the step flooded for it, or a filtered frame
whose sender is one in turn, and so on down; a filtered frame on top
keeps the request. The filters live in the breakpoint table and go back
to their default with its other contents at an attachment and a detach.

A step never lands in a filtered frame: by default the shipped modules'
bodies -- a body is shipped when its module's root is, as for its
metadata ("Compiling the metadata again") -- and, beside them, the
bodies of the program's class declarations a list the client sends
names, IntelliJ's "Do not step into the classes"; the filter of the
shipped modules can be switched off. A stop a step would make in a
filtered frame -- a return point inside `Array>>do:` after a step over
at a block's last statement, the first statement of a shipped method a
step into pushes -- is not made: the step goes on as a step into, so
that it stops at the next statement start of an unfiltered body, the
block's next activation or the caller of `do:` once it returns, which
the step floods with one-shot breakpoints, the first unfiltered frame
below the stepping one. A filtered step into keeps its held request
through the pushes of filtered bodies until an unfiltered one is
pushed. The filter decides only where a step lands: a breakpoint the
user set in a filtered body stops, and a step that starts in a filtered
frame -- stopped there by a breakpoint, or with the filter off -- steps
in it as in any other. The exception's first frame shown is likewise the
first unfiltered one ("Exceptions").

V8 does the same with blackboxing (`v8/src/debug/debug.cc`): a step at a
return becomes a step out in step-into mode (`:1601-1612`), the step out
floods the first frame below that is not blackboxed (`:1735-1741`), and
every function entered that is not blackboxed is flooded
(`PrepareStepIn`, `:1456-1470`); the JDWP agent skips a filtered callee
of a step into by frame-pop and method-entry events
(`jdk/src/jdk.jdwp.agent/share/native/libjdwp/stepControl.c:588-611`,
`:476`). The specification allows it without requiring it: a debugger
"need not step through" the sends a tuple literal is defined by (§5.1.7).
Stepping without a filter, as Newspeak's simulator does (`stepInto:` is
one bytecode, `primordialsoup/newspeak/Mirrors.ns:2411-2415`) and SOMns,
which hides only primitives through Truffle's internal sources, is
rejected: every step into `do:`, `inject:into:` or a tuple literal would
stop in the kernel's frames, where evaluation, setting a variable, a
restart and a forced return are refused anyway. Newspeak's `stepThrough:`,
which stops at the next activation whose home is the stepping frame's
home (`Mirrors.ns:2285-2293`, `:2431-2436`), solves only the block's end,
not a step into `do:`, and is not needed beside the filter. The filter
costs a filtered step into its held request through every push of a
filtered body, each run interpreted, since the request makes compiled
prologues refuse the push; V8's hook on every call and JDWP's
method-entry events cost the same, and a step over or step out costs
nothing.

### 17. A step out stops at the next place the program reaches

**Partial:** built for every way a frame leaves but the handler search's
report: the unwinder's pops and the arms of
`Continuation::Unwind`, `NestedClass` (a step pushed in the clause's
place, or an error), `VmErrorSignalled`, `Reexecute` and
`ReturnToSuspended` report to the stepping state
(`Mutator::pop_for_unwind`, `interpreter::breakpoint::frame_left`,
`stepping::frame_left`), and the step goes on past the shipped frames
-- `ensure:`'s, `on:do:`'s, `resolve`'s -- by decision 16's filter; the
handler search's report ("Exceptions") is planned. The unwinder's pops
and arrivals also report the exit of a method a method breakpoint names,
which stops by the same kind of step ("Method breakpoints").

A step out, and a step over at a body's last statement, stops at the
stepping frame's return point when its caller goes on there --
`Continuation::Deliver`, and a superclass clause's
`Continuation::NestedClass` whose class `after_clause` hands to the
accessor's send. Every other way a frame returns goes on as a step into
(with decision 16's filter) until the next statement start of the
program, the frame the program goes on in flooded with one-shot
breakpoints: an `ensure:` or `ifCurtailed:` block the unwinder ran
(`Continuation::Unwind`) stops in the next such block or in the frame
the unwind lands in; a future's `resolve` (`Continuation::Reexecute`)
stops in its caller after the instruction run again, or in what that
instruction pushes; a caller standing at an instruction that raised
stops in the handler block that takes the error, which the search
reports to the stepping state ("Exceptions"), or the program ends with
the error (`Continuation::VmErrorSignalled`); and a frame that returns
to a suspended sender (`Continuation::ReturnToSuspended`) stops in the
sender, resumed. The pop is reported to the stepping state only by the
unwinder as it pops (`Mutator::pop_for_unwind`) and in
`apply_continuation`'s arms other than `Deliver`'s, which call one cold
function (`interpreter::breakpoint::frame_left`), so the return's hot
path is unchanged: `finish` keeps no handle of the popped frame for
them. The report finds the popped frame as the one whose offset lies
above the stack's top (`Heap::activation_lies_above_the_top`), since
nothing is pushed between the pop and the report, and every name the
step knew it by becomes `Holder::Left`, which no activation is
(`stepping::popped`) -- for a frame returning to a suspended sender
before the sender is resumed, which may take its offset, all a release
build compares (`breakpoint::returning_to_suspended`). When the step's
frame, or the caller its return point is in, has left, the step goes on
as a step into from the frame on top (`continue_as_step_into`); any
other frame the step named above it is only forgotten. An error the
popped frame's continuation raises -- a superclass clause's answer
refused as no class -- makes no frame the step goes on in yet: stepping
into the `signal` pushed for it would stop in the exception's
machinery, so the step waits, its frame `Holder::Left`, for the
unwind its handler or its default action starts, whose first pop goes
on, and, once built, the handler search's report. A step out of a
superclass clause whose class is handed to the accessor's send, the
`NestedClass` arm's value, reports nothing and stops at the accessor's
return point.

All three systems define a step out by where the program goes next,
not by how the frame returned: Newspeak's `stepOut:` stops as soon as
the sender is on top or no longer on the sender chain
(`primordialsoup/newspeak/Mirrors.ns:2276-2283`, `:2417-2424`), the
JDWP agent single-steps after the frame pop "until we reach the next
safe place to stop" (`stepControl.c:340-347`), and V8 floods the frame
below and stays in step-into mode (`v8/src/debug/debug.cc:1601-1612`),
flooding the handler's frames on a throw (`PrepareStepOnThrow`,
`:1488-1555`). Refusing a step out of such a frame is rejected, since the
end of an `ensure:` block is a common place to step from; treating it as
a resume loses the place the step was taken from.

## Planned: the mechanisms

What building the decisions changes, found by checking them against the
code and the invariants in `CLAUDE.md` in three reviews; each is part of the
design, not an option, unless it names an open question ("Open
questions"). The mechanisms stages 1 and 2 built -- the poll words as
atomics, the commands served while the program runs, reading the stack at
a stop, compiling the metadata again and the values a stop holds -- are
described in "The debugging API at the safepoint", with what of them is
still planned; a mechanism below that stage 3 has begun to build says
what of it is built.

### Where the original lives

The run loop is not the only reader of a slot: `send_destination`
(`vm.rs`), two leaves in `jit/leaves.rs`, and the decoded view
(`CompiledCode::instruction`, `instructions`) read it, and the decoded view
feeds the leftover analysis, `jit::baseline::lower`, the optimizing tier's
snapshot and build, the image encoder, `shipped-code`, `disasm` and the
debugger's `compiles_alike`. So the
original of a patched slot is kept on the body, beside `EncodedBody::slots`,
and every reader but two sees the original without reaching the debugger:
the decoded view answers it (`EncodedBody::original`), and
`send_destination` reads it. Only the run loop's fetch sees `Breakpoint`.
`jit::baseline::compile` asks `CompiledCode::is_patched(index)` before
it hands an instruction to `lower`, and lowers a patched index as an
exit (`Lowering::Exit`), which hands the frame to the interpreter at the
slot, whose `Breakpoint` arm stops there; the exit counts in
`JitStatistics::breakpoint_exits`, and against the body's exit budget as
any exit at an instruction does, and a body whose patch it reaches
before any transfer exits at once and is not installed. The optimizing
tier neither optimizes nor inlines a body with a patched slot: `optimize`
puts a patched body's request off, re-arming its budget rather than
recording a refusal, so that it is optimized once its breakpoints are
gone; the snapshot tree's walk answers `CalleeFacts::Other` for a patched
method callee, which the copy then calls, and neither inlines nor
eliminates a patched block, whose closure is made and runs the block's
own code. `Breakpoint` is a compact
form only, never an
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
patched index's exit as a send's -- a statistic only, which
`JitStatistics::breakpoint_exits` counts instead. `is_patched`, which the
baseline's compile and that statistic ask, reads the slot for the
`Breakpoint` itself. A new reader of `slots` has to decide which view it
wants.

### A breakpoint in a method with a fast path

A method with a `MethodFastPath` never runs its body (`mixin.rs`): a quick
method (`^true`, `^false`, `^nil`, `^self`, `^` a parameter), a method
that only sends `value` or `cull:` to a block parameter, a builtin class
accessor, and a mirror forwarder whose leaf does not decline. Only the
shipped modules' methods have one: `compiler::primitive_method::recognize`
gives the quick methods and the class accessors only to the kernel's
resolution, and a mirror forwarder needs the `MirrorSend` binding only
`resolve_kernel` records. The interpreter's `act` and machine code's
stubs read the fast path off the method on every hit, and the optimizing
tier inlines a quick method without looking at its body. So a line
breakpoint or a method breakpoint the user sets in, for example,
`True>>ifTrue:` -- which stops, since the step filter decides only where a
step lands (decision 16) -- would never stop.

The fast path is therefore switched off on the method for as long as a
breakpoint is set in its body: the field `MixinMethod::fast_path` is a
`Cell<Option<MethodFastPath>>`, read through `MixinMethod::fast_path()`
and written only by the debugger through `switch_fast_path` -- cleared
by the first patch that names the method -- a line breakpoint's, a
method breakpoint's or the run to cursor's; a step's one-shot names
none, so that a body a step patched first loses its fast path all the
same when a breakpoint is set into it -- and put back from
`recognized_fast_path` by the restore of the body's last patch --
after which the table's `settle`
(`debugger/breakpoints.rs`) vacates the inline caches and their machine
cells (`InlineCaches::invalidate`) and frees every optimized copy, since
a copy that inlined a quick method records no code id of it
(`jit::invalidate_all_optimized`); setting a breakpoint is rare. The
table finds the method by its body's address among the methods of the
module it resolves against, and keeps it with the patch. Reading the
fast path costs every hit what reading the field cost, one load, and no
check is added. What the program is -- the shipped code's encoder and
the shipped world -- reads `recognized_fast_path`, which a breakpoint
never changes. Replacing the method in its mixin's method table, Squeak's way of
recompiling a method with a `break`, is rejected: a shipped mixin is
never replaced (`Heap::replace_mixin` refuses one), and everything that
finds a shipped mixin by its declaration or its address -- the shipped
world's tables, the image encoder, `Heap::class_is_shipped`, the
metadata cache -- would have to follow a replacement. A check on every
hit whether the method is patched is rejected too: a cache records what
the lookup decided and re-reads only what its key does not cover
(`CLAUDE.md`, "A cache records what the lookup decided; it never
decides"), so the check would cost every send of every fast-path method.

### Stopping at and going past a breakpoint

**Partial:** the stop and the going past are built for line breakpoints;
conditions, hit counts and logpoints, and what a restart, a forced
return, edit-and-continue and an evaluation do with the token, are
planned.

A `Breakpoint` slot shares the run loop's out-of-line arm of the
simultaneous slot clauses (`vm::simultaneous_clause_instruction`): an
arm of its own, one more target of the `match`'s jump table, grew
`vm::run` past the `codegen` gate's budget, where the shared one costs
nothing. That call hands the decision to `interpreter/` (`vm.rs` holds
no dispatch logic): `interpreter::breakpoint::reached`, `#[cold]`, which
answers `BreakpointDecision::Execute` with the slot's original or
`BreakpointDecision::Stop`; planned, it also evaluates a condition,
counts a hit or logs. Going on, the arm replaces the fetched slot by the
original and re-enters the loop's `match` with it -- the `match` sits
inside a dispatch loop that runs once per instruction unless the arm
continues it with the original, which costs nothing until the arm is
reached, since breaking the loop's outer labelled blocks from inside it
is legal; a check before the `match` would cost every instruction a
compare -- so every kind of instruction, sends included, runs through
its ordinary arm and the original's behaviour is the interpreter's own;
`run_wide`'s way, the effect functions called out of line, is closed to a
send (`unreachable!("internal: a send is never encoded through Wide")`),
and statement starts and return points are often sends. Stopping, it
records the stop on the `Mutator` (`Breakpoints::request_stop`) and the
arm leaves the stretch as `Leave::Safepoint`, the frame standing at the
slot: the stop is the safepoint's (`serve_debugger`, "Serving at the
safepoint"), never the arm's.

A stop that resumes sets a "past (activation, body, index) once" token,
so the loop does not stop at the same breakpoint again before executing
it; it is set only while a breakpoint still patches the slot, since one
removed at the stop left the original there, which no arm reads. An
activation's offset is reused after its pop, so the token is consumed or
dropped at the next `Breakpoint` arm the loop executes -- after a
`Continue`, the same frame fetching the same slot again, so no pop needs
to look at it -- and dropped when a run ends, since a termination leaves
it unexecuted, and when its slot is restored. The suspend stress mode,
which resumes the stopped frame at another offset, moves it there.
Planned: a restart, a forced return and edit-and-continue drop it
explicitly, since a restart does not pop its activation and would
otherwise skip the breakpoint at its first statement, and breakpoints
skipped during an evaluation neither consume nor drop it. A
`Continuation::Reexecute` runs a caller's instruction again at the same
index once a future it met is resolved (`vm::reexecute`); when that
instruction is a patched one, `interpreter::breakpoint::reexecuting` sets
the token again, so the breakpoint neither stops, counts nor logs twice.
The `codegen` gate's budgets of `vm::run`, `resume_caller` and `finish`
hold what the `Cell` slots and the arm cost the hot path.

### Breakpoints by body, and reclaimed code ids

A full collection reclaims code ids nothing names (`heap/tables.rs`,
`heap/reclaim.rs`), so a breakpoint keyed by code id could restore its
original into another body that later gets the id. The breakpoint table
(`debugger::breakpoints::Breakpoints`, on the `Mutator`) holds the body
itself (`Rc<CompiledCode>`), keyed by its address and the index, with
every breakpoint standing there; restoring the last of an index restores
the slot. A full collection treats every patched body's code id as named
(`Breakpoints::patched_code_ids`, beside `jit::inlined_code_ids` in
`Mutator::reclaim_ids`), as it does `CopyDependencies::code_ids`; a body
without an id yet is given one when it is patched. Every body a slot of
which the table patched or restored has its machine code freed once the
command is served (`jit::free_for_patch`, called by `settle`): its
baseline code, its twin and its optimized copies, as an abandonment frees
them, and, keyed by its code id over every copy's
`CopyDependencies::code_ids`, every copy that inlined it, as an
invalidation frees them -- code compiled before the change runs the slot
as it was, past a breakpoint set since or into an exit at one removed
since. An inlined lazy slot's getter names no code id
(`AlwaysInlined::LazyGetter`), so a patch of one frees every copy: a
method breakpoint's entry and exit stand at the getter's first
instruction, which a copy's inlined read of the filled slot stands for. The
body then counts its entries afresh, whatever its state was -- compiled,
abandoned, or exiting at once at its patch -- unless it was refused for a
reason no patch changes, so that a breakpoint never leaves it interpreted
for good: it is compiled again with its patches as they are now, and,
once the last is removed, without exits.

### A breakpoint names a source position

The IDE sets a breakpoint at a file and line; the agent, stage 4's, turns
the line into a byte offset into the module's current text and sends
`SetBreakpoints` with the module's name and the offsets. The VM resolves
each offset (`debugger::breakpoints`) against every current body of the
module -- methods, blocks (a block's body is a literal of its home, so a
block never yet created is patched in place all the same), slot
initializers, superclass clauses and initializer expressions, each a
body of its own, of every level of the module, both sides -- through
their metadata, compiled again for the purpose (`metadata::Cache::level`):
to the first statement that compiled to an instruction and starts at or
after the offset, and patches the first instruction of every statement
with that same span, in every body. A body without metadata is not
patched. A shipped module's text is its shipped text. A breakpoint whose
module is not built yet -- a program module not loaded, a shipped module
built on first demand through the mirror -- is pending
(`BreakpointState::statement` is `None`) and is resolved when the
module's root class is built (`Mutator::load_root_class`,
`core_library::module_class`, before its factory and initializer run),
reported unasked by `Event::Breakpoints`. An image's load builds its
modules in an interpreter of its own, which no debugger is attached to
yet. An install replaces bodies: the edited module's breakpoints are
resolved again by their offsets against its current bodies, a patch on a
current body a breakpoint no longer stands at is removed, a moved one is
reported unasked, and frames still running the old bodies keep theirs,
whose patches stay until the breakpoint is removed.

An inlined control-flow block -- an `ifTrue:`'s, an `and:`'s, a
`whileTrue:`'s -- is lowered twice: inline in its home, and as the
fallback closure a receiver that is not a Boolean runs
([execution.md](execution.md), "Fallback blocks"). A line breakpoint
patches both copies, as a flood and a method breakpoint do, or a
non-Boolean receiver runs past the breakpoint.

`SetBreakpoints` changes only the slots the module's old breakpoints
and its new ones do not share: the old ones' patches are taken off
without restoring a slot (`Breakpoints::strip`), the new ones set, and
only then is every slot no breakpoint holds any more given its original
back (`Breakpoints::sweep`), so that an agent sending the same set again,
as an IDE does at every change, frees no machine code and switches no
fast path. `SetMethodBreakpoints` does the same.

The plugin needs the breakable places before any session runs, since
IntelliJ asks `canPutAt` and the variants while breakpoints are set before
a launch; `breakpointLocations` answers only within a session. So the
places are proposed to come from `ns-next-analyzer`, through a request of
the language server, which can compile a module, and to be verified by
`breakpointLocations` when the session starts ("Open questions").

### Step over in recursion and blocks

**Built** (`debugger/stepping.rs`). A step command at a stop begins the
step (`stepping::begin`) in the top frame: a step over and a step into
patch every statement start of the stepping frame's body with a one-shot
breakpoint for that frame -- the flood -- and every step sets one at the
frame's return point, the index after the instruction its caller stands
at, for the caller. The one-shots are a kind of the breakpoint table's
beside the line kind (`breakpoints::OneShot`), so that a slot holding
both is restored only when the last goes, and every step's patches
change through the table's `settle`: their bodies lose their machine
code for the step and are compiled again once it ends, with every
optimized copy that inlined them. The top frame fetches the slot it
stands at again as it goes on, which the flood may just have patched --
a pause at a push leaves a callee at its first statement start, one at a
backward jump at its loop's head -- so `begin` sets the past-once token
for it ("Stopping at and going past a breakpoint"), and the step stops at
the next place, not where it stands. The next stop ends the step
(`stepping::end`), whatever made it -- the step's own one-shot, a
breakpoint, a pause -- every one-shot removed; a stop at a slot only a
one-shot stops at reports `StopReason::Step`, and one a line breakpoint
holds too reports the breakpoint. Detaching, a replacing attachment and
the end of the run end it as well.

A one-shot carries the frame it is for (`stepping::Holder`), by its
position on the activation stack: a `FrameReference` would reify the
frame and send it through its body's twin and a divorce on return, while
a position is reused only after a pop, which the return point's one-shot
stops at first, ending the step; `ActivationReference::names` compares
it, refusing in a debug build a handle whose activation returned. The
`Breakpoint` arm (`interpreter::breakpoint::reached`) takes a one-shot
only in that activation, or in a block's whose captured frames lead to
it (`stepping::takes`) -- only the blocks the compiler lowered inline in
the stepping body are flooded for it -- and ignores it in any other
activation of the body: a recursive call deeper on the stack, or a block
of the same home running in a callee. V8 checks the frame count the same
way ("We have to ignore recursive calls", `v8/src/debug/debug.cc:822-855`),
and the JDWP agent compares `fromDepth` (`stepControl.c:523-697`).

The flood covers the stepping body's own statement starts and those of
the inlined blocks in it, from its metadata, and the statement starts of
each block literal the compiler lowered inline (`stepping::inlined_blocks`):
under the optimized lowering, the fallback closure a receiver that is not
a Boolean runs, recognized as a block literal whose `source` encloses
one of the body's statements, its inline copy's; under the reference
lowering, which inlines nothing, a block literal a `SendInlinable` takes
as its receiver or an argument, the blocks the optimized lowering
inlines; and so on into those blocks' own. A simultaneous slot's
computation, the closure a `MakeFuture` takes, is none of them, though
its source is the very span of the initializer's statement for the slot:
its future's `resolve` runs it, a frame of its own. So a step over a non-Boolean
receiver's `ifTrue:` stops in its fallback block as in the inline copy,
and a step under the reference lowering stops where it stops under the
optimized one. A step taken in such a block steps in its creator, the
frame whose inline code the block stands for (`stepping_frame`), so that
a step over the block's last statement goes on at the creator's next
statement start rather than in the receiver's `ifTrue:` or in a loop's
`value` send; a creator that returned leaves the block a frame of its
own. A block the body creates otherwise is a body of its own and not
flooded, so a step over a `do:` whose argument block contains a
breakpoint stops only at that breakpoint, as IntelliJ's does.

The suspend stress mode suspends every activation of the run into its
frame object and resumes them one by one at other positions; a frame
suspended when the step begins, the stepping frame's caller or its
creator, is the same. The step holds each such frame by its frame
object (`Holder::Suspended`, `stepping::suspending`), and every handle it
knew the frame by names the activation the frame is resumed as
(`stepping::resumed`, called from `Mutator::resume_frame`), so that its
one-shots, its return point and its push test follow the frame.

### Step into's held request

**Built** (`stepping::held_step`). A step into is a step over that also
holds the request bit `DEBUGGER_STEP` until the next push. The held
request is taken at every poll until then, not only at pushes: a
backward jump in the stepping frame, or a caller resumed, also reaches
the safepoint, which goes on without stopping when nothing was pushed. A
loop with no send, or a statement whose sends are all answered by
primitives, so takes the safepoint at every iteration until a push or a
one-shot breakpoint ends the step, and machine code's loop-head poll
exits at each. The bit is one of `LOWERS_DEPTH_LIMIT`, as the pause bit
is ("The poll words as atomics"), so that a compiled prologue refuses
the push and the interpreter's push takes the safepoint before the
callee runs. The safepoint recognizes a push by a frame on top whose
sender is the stepping frame or a block the step flooded for it -- not by
a `program_counter` of 0, which a backward jump to the stepping body's
first instruction, or a frame `retry` restarted, also shows, nor by a
frame merely above the stepping one: a step taken in an inlined block
steps in the block's creator, and the frames between the two, the
receiver's `ifTrue:` among them, and what they send to are not the
step's. A frame that runs a flooded block stands for the stepping
frame's inline code and is no push, nor is a frame that lay between the
one the step was taken in and the stepping frame when the step began,
which goes from that list once the top lies below it, so that a frame
the stepping frame pushes later at its offset is a push. The push drops the bit and stops at the new body's first
statement start: at once when its frame stands there, at index 0, and
through a one-shot for the pushed frame at index 1 for a body -- a
method's or a block's -- whose first instruction makes its indirection
vector (`MakeVector`, emitted before the local initializers and outside
every statement's range, `compiler/lower.rs`). A send answered by a
primitive or a leaf pushes nothing, and the next one-shot stops.

### Writing a frame

Reading a compiled frame needs nothing; a frame whose variable is set, or
that is restarted or forced to return, goes on in the interpreter -- a stop
at an optimized poll having been deoptimized first ("The stop"): its
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
  `Interpreter::safepoint` answers today only for the suspend stress mode
  (`SafepointOutcome::Rebuilt`), never in a release build -- when the
  debugger pushes the evaluation's frame and when it returns. The variant is named in
  [image-format.md](image-format.md), "Planned: suspend and resume", for
  completeness; no image is written while an evaluation runs, so it is
  never encoded.
- **The frame it reads.** Reifying the stopped frame clears its compiled
  mark first, so that it goes on interpreted -- a frame the debugger may
  write needs no machine code; the run loop could enter a reified frame in
  its body's reifying twin (`jit::entry_of_frame`), so this is a choice,
  not a necessity; a stop at an optimized poll is
  deoptimized before ("The stop"). The debugger makes
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
above it; with step filters (decision 16) that is the first frame of
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

### Method breakpoints

**Built** (`Command::SetMethodBreakpoints`, `debugger::breakpoints`). A
method breakpoint names its method by its class declaration -- a
`ClassName`, as a step filter names one -- its side and its selector,
and stops every activation of it at its entry and at each of its exits.

- **The entry** is the body's first statement start, index 0, or 1 for
  a body whose first instruction makes its indirection vector, which
  runs before every statement (`PatchKind::Entry`). A step into stops a
  push at the same place without running its slot, and reports the
  entry there too (`breakpoint::standing_hits`).
- **The exits** are the body's own returns -- `Return`, `ImplicitReturn`
  and a lazy slot's getter's `ReturnSlotIfFilled` -- and the
  `NonLocalReturn`s of the blocks the compiler lowered inline in it,
  found as a flood finds them (`stepping::inlined_blocks`): its fallback
  closures, and under the reference lowering the blocks a
  `SendInlinable` takes, so that both lowerings stop before the same
  `^` (`PatchKind::Exit`). The stop stands before the return, the method's
  frame or its block's still on the stack. An exit is one only where the
  slot leaves the method (`breakpoint::exit_is_taken`): a lazy getter's
  first instruction once its slot is filled, a non-local return while
  its home runs.
- **An exit by an unwind** -- an exception's, a `retry`'s, a non-local
  return of a block of another method passing through it, or a return
  that arrives at it as a home without a patched return, from a block
  it passed on or through `return:` -- is reported by the unwinder,
  which pops the frame (`breakpoint::leaving_by_unwind`, read before the
  pop, `left_by_unwind` after it) or arrives at it
  (`breakpoint::returning_by_unwind`); a termination reports none. The
  exit stops where the program goes on, by a step of the same kind as a
  step whose frame an unwind pops (decision 17, `stepping::method_left`):
  a step into from the frame now on top, or a step out from the method's
  frame the unwind arrives at, which stops at its caller's return point.
  That step replaces any step in progress, carrying its value of the
  last call, and its stop reports the exit. A stop at an exit at a
  non-local return records the home it returns to
  (`AfterPast::ExitReported`, `Breakpoints::report_home`), held as its
  frame object, so that the unwind's arrival there does not report the
  exit again. The cut, which pops compiled frames without the unwinder,
  answers its exit for a frame of such a method (`jit::cut::cut_frame`):
  the entry's patch makes the body exit at once, but the JIT stress mode
  installs such a body all the same, and an on-stack replacement at a
  loop's head then runs it as the stretch's entry frame, the home the
  cut would return from when its own block returns non-locally.
  A pause served after an unwind left such a method, before the step
  that reports the exit stopped, reports `MethodExit` rather than
  `Pause`, as any stop does that serves a pause.
- **A stop that is both** a method's entry or exit and a line
  breakpoint's statement, or two method breakpoints' of the same method,
  is reported once: `StopReason::MethodEntry { id, others }` before
  `MethodExit`, before `Breakpoint { ids }`, `id` the first and `others`
  every other breakpoint the stop is for, the line breakpoints'
  included (`Hits::reason`); DAP's `stopped` event names them all in its
  `hitBreakpointIds`. A body whose entry is also one of its returns -- a
  body of one instruction, a lazy getter -- stops at the entry and,
  going past it, again for the exit before the slot runs
  (`AfterPast::ExitDue`).
- **Resolution**: a method breakpoint whose module is not built, or
  whose class declares no such method, is not verified
  (`MethodBreakpointState::verified`) and is set when a module's build
  or an install declares the method (`breakpoints::module_built`),
  reported unasked by `Event::MethodBreakpoints`; an install sets it on
  the method's new body, and the body it replaced keeps its patches for
  the frames still running it. A method with a fast path loses it while
  the breakpoint is set ("A breakpoint in a method with a fast path").

### Smart step into a block

IntelliJ offers a lambda on the line as a target. Here a block argument's
body is pushed from inside its callee -- `do:`'s loop, not the chosen
`CallSite` -- so a block target is a one-shot breakpoint at the block
body's first statement start, carried for every activation of that block
created by the stepping frame, rather than a push from a call site.

### The return value of the last call

**Built** (`stepping::record`). IntelliJ shows the value the last call
returned after a step over or a step out. It is the value in the
destination register of the instruction before a return point --
written by `resume_caller` for a callee that returned, or by the send's
own arm for a send a primitive answered. A step over stops at the next
statement start, past the return points of the stepping frame's sends,
so the flood also patches the index after each send of the flooded
bodies with a recording one-shot, which never stops: it records that
destination register as a `Root` on the stepping state and goes on. The
return point's one-shot records too, so a step out, and a step over that
leaves its frame, report the callee's answer. A destination the leftover
analysis finds tainted before the one-shot's index -- reached by a jump
that skipped the call -- is not read. The step's stop reports the last
value recorded as `Stopped::returned`, the stop's first value; a stop in
a frame a step into pushed, and every stop that is not a step's, report
none.

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

## Open questions

Raised by the reviews of these decisions and of stage 1; each is to be
decided with the user, one at a time, before the stage that needs it.
The proposal is a proposal.

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
  built with `--strip-source` shows its frames' names but no variable
  names and no lines.
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
- **Breakable places before a session.** Whether `ns-next-analyzer`
  answers the breakable places and their variants to the plugin, through a
  request of the language server, before any session runs (proposed), or
  the plugin allows a breakpoint on every line and the session moves it.
- **Terminating a program that reaches no safepoint.** A program blocked
  in a leaf waiting for input, or in a long leaf, serves no `terminate`.
  Proposal: the agent's thread ends the process after a short grace
  period, as IntelliJ's stop button kills a JVM that does not answer.
- **Commands between runs.** A command queued while no run executes, or
  still queued when a run ends, is dropped without an answer today
  ("Commands while the program runs"), which stage 1's clients, the tests
  and the pause stress mode, never meet; an agent that sends a stop-only
  command between runs gets no event and waits for its timeout. Whether
  the DAP agent of stage 4 needs a `Refused` for each, sent when the
  command is dropped, or whether it never sends a command between runs,
  is to be decided with stage 4.
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

Stages 3 to 6 remain, in this order. Stage 1, the debugging API at the
safepoint -- stopping, the command channel and the commands served while
running, the atomic poll words and pause, the deoptimization of a stop at
a poll of optimized code in every build, the stack and values -- is built
("The debugging API at the safepoint"), and so is stage 2, the metadata:
both lowerers' stop locations and variable tables, compiled again on
demand and verified, which give a stopped frame its name, its place and
its named variables ("Reading the stack at a stop", "Compiling the
metadata again"). The stages keep their numbers, which the text above
refers to.

3. Breakpoints (pending ones included), stepping, run to cursor, method
   breakpoints and exception breakpoints; the breakpoint and step stress
   modes.
4. The DAP agent in `ns-cli` and the plugin's `XDebugProcess`: from here
   the debugger is usable in the IDE.
5. Evaluation, conditional breakpoints and logpoints, watches and setting
   variables, smart step into, restart frame and force return.
6. Hot swap, then edit-and-continue of running activations;
   `ActivationMirror` for programs.
