# The debugger

> **Status:** **Partial:** stage 1 is built -- the debugging API at the
> run loop's safepoint ("The debugging API at the safepoint"): a host
> attaches a debugger and hands its remote to another thread, which pauses
> the program wherever it runs, interpreted or in machine code, optimized
> code included, queues commands the next safepoint serves, and, while the
> program is stopped, reads its stack, each frame's receiver, arguments
> and live registers, and the fields and elements of the values those
> answer, then continues or terminates the run; the request word the poll
> reads is a set of bits in atomic machine words, which the agent arms
> from its own thread; and the pause stress mode stops every program at
> the safepoints its schedule picks and reads its whole stack. Stage 2 is
> built too -- the metadata ("Compiling the metadata again"): both
> lowerers collect a body's statement starts, its sends with their
> receiver and selector spans and its named variables when the debugger
> compiles the body's declaration again from its module's current text,
> kept per body only when the body compiled again is alike; a stopped
> frame shows its name, the statement and the send it stands at, and its
> named variables over its whole lexical chain, and the oracle holds every
> body of every golden and of the shipped modules to compiling again
> alike. Stage 3 is built as well -- breakpoints and stepping: a line
> breakpoint, set by its module and an offset into its text, pending until
> its module is built and set again on the bodies an install replaces,
> patches its statement's slots in place and stops the interpreter there
> ("Stopping at and going past a breakpoint", "A breakpoint names a source
> position"); a patch frees the body's machine code and every optimized
> copy that inlined it, the baseline lowers a patched slot as an exit and
> the optimizing tier leaves a patched body alone, and a method's fast
> path is off while a breakpoint stands in its body ("Where the original
> lives", "A breakpoint in a method with a fast path"); step over, step
> into and step out stop at the stepping frame's next statement start, in
> a frame it pushes and at its caller's return point, by one-shot
> breakpoints for the stepping frame and a request bit a step into holds
> until the next push, and report the value of the last call ("Step over
> in recursion and blocks", "Step into's held request", "The return value
> of the last call"); a step out taken in a block a send took, inlined or
> not, leaves that whole send (decision 17); a step whose frame an unwind
> pops, or whose frame returns otherwise than to its caller's send, goes
> on as a step into from the frame the program goes on in, the next host
> run's first statement when the frame returns to the host (decision 17);
> a step into an instantiation stops in the factory's frame, at the
> class header's factory pattern, a step over or into there goes on to
> the first statement of the initializer it pushes, a step into a
> superclass clause or a `<:` clause's message stops at the entry of the
> initializer it enters, at that class's header, `Object`'s never, every
> initializer's statements are stepped in the order they run, and a step
> on its way out passes over the factory to its caller (decision 12); and
> no step lands in a frame the step filters filter, by default the shipped
> modules' -- a stop it would make there goes on as a step into from the
> first unfiltered frame below, and a step into's request outlasts the
> pushes of filtered bodies (decision 16); run to
> cursor stops at a position resolved as a line breakpoint is, in any
> activation, once (decision 3); and a method breakpoint stops at its
> method's entry and before each of its returns, and reports an exit an
> unwind makes where the program goes on ("Method breakpoints"); and
> exception breakpoints stop an uncaught exception before its
> `defaultAction` runs, a caught one before its handler runs, each
> filtered by class, and an uncaught `halt`, which goes on resumed with
> nil, the stack trace marking the frames above the first one below the
> exception's machinery hidden, and the handler search reports the handler
> it found to a step in progress ("Exceptions"); and the breakpoint stress
> mode patches every statement start of every body it finds running at a
> safepoint with a breakpoint that never stops ("The breakpoint stress
> mode"); and the step stress mode stops every program at the safepoints
> its schedule picks and takes a burst of steps from there, reading the
> stack at every stop ("The step stress mode"). Stage 4 is built too --
> the debugger in the IDE: `ns-next run` and `load` take `--debug`, which
> listens on 127.0.0.1 behind a token, and `--debug-connect`, which
> connects to a client that listens, and the DAP agent in `ns-cli` speaks
> the session over the remote's two halves -- line breakpoints with block
> variants and hit conditions, which the VM counts, function and exception
> breakpoints, steps by line or statement, run to cursor, pause, terminate
> and disconnect, and a stop's stack, variables and sources, a module's
> text asked of the VM by revision ("Launching, the start and the end",
> "Hit counts and temporary breakpoints"); the language server answers a
> module's breakable lines before any session runs, from
> `Interpreter::breakable_statements` ("A breakpoint names a source
> position"); and the plugin debugs a Newspeak run configuration in
> IntelliJ's debugger over a DAP client of its own, with line, method and
> exception breakpoints ("The JetBrains plugin"). An attached debugger
> with its defaults costs a running program nothing measurable ("What
> stage 4 moved"). The VM evaluates an expression in a stopped frame,
> with a depth reserve of its own, and a pause aborts it
> ("Evaluation"), and sets a stopped frame's variable, refusing one a
> block may already hold a copy of ("Writing a frame", "Assigning a
> captured variable"), and decides a breakpoint's condition and a
> logpoint's message, each an evaluation in the frame that reached it
> ("Hit counts and temporary breakpoints"), and restarts a stopped frame
> or forces it to return, through the one unwinder ("Restart frame and
> force return"); the DAP agent evaluates, sets variables, marks
> objects, passes conditions and logpoints on, smart-steps into a chosen
> target, restarts frames, forces returns, answers the memory view and
> marks the shipped frames for folding ("Launching, the start and the
> end"); `ns-next mcp`, a Model Context Protocol server, lets an
> agent such as Claude debug a program through its tools, each stop
> answered as a summary in text ("The MCP server"); and the plugin
> evaluates in the Evaluate dialog, its watches and its hovers, sets
> values, sends conditions and log expressions, smart-steps, resets
> frames, forces returns, marks objects, shows the memory view and folds
> the shipped frames as library frames ("The JetBrains plugin"); none
> of it adds to the hot path of a program run without a debugger ("What
> stage 5 moved"). Everything else is planned: hot swap,
> edit-and-continue and `ActivationMirror` (stage 6) ("Planned: the
> order of building it"). The
> design rests on seventeen decisions made with the user, nine on
> 2026-11-10, stage 2's six and stage 3's two on 2026-11-12 ("The
> decisions"), on the mechanisms they imply, stage 4's questions among
> them, decided on 2026-11-14 ("Planned: the mechanisms"), and leaves the
> questions in "Open questions" to be decided. It builds on groundwork
> that exists: reified activations and the activation stack, suspension
> into frames, the statement→program-counter table, stable call-site
> numbers, retained source, the unwinder's frame targets, the JIT's
> materialization and invalidation, the safepoint poll, and the
> MirrorBuilder's atomic install, each described in the document for its
> own aspect ("What exists").
> **Code:** the VM's side is `crates/ns-runtime/src/debugger/` (`mod.rs`:
> `Interpreter::attach_debugger`, `serve_debugger`, the service between
> runs, the stop, a module's text by revision, the arming lock `Arming`,
> the pause stress mode's `StressReader`, the step stress mode's
> `StepReader` and `StepStress`; `evaluation.rs`: an expression compiled
> in a stopped frame, pushed and its end, a variable set and the
> marks' labels; `conditions.rs`: a breakpoint's condition and a
> logpoint's message decided at the safepoint; `memory.rs`: the memory
> view's counts, instances and referrers; `unwinding.rs`: a frame
> restarted or forced to return, the unwind the stop starts and the step
> it begins; `breakpoints.rs`: the breakpoint table,
> line breakpoints, the one-shots, the hit counts and the breakpoint
> stress mode's patches; `stepping.rs`: the step in progress,
> `Stepping`; `exceptions.rs`: the exception breakpoints, decided by the
> handler search's two mirror rows (`builtins/activation.rs`), and the
> first frame an exception's stop shows; `remote.rs`: `DebugRemote`, its
> halves `DebugControl` and `DebugEvents`, `Command`, `Event` and the
> descriptions that cross to the agent's thread; `stack.rs`: the stack
> trace, a frame's body found by its declaration (`origin_of`) and its
> variables; `values.rs`: the values a stop holds, `StopValues`;
> `metadata/`: the tables (`mod.rs`: `BodyMetadata`,
> `MetadataCollector`), a declaration's level compiled again and paired
> (`again.rs`), a module's breakable statements without loading it
> (`breakable.rs`), the cache (`cache.rs`) and the oracle,
> `Interpreter::check_debug_metadata` (`check.rs`)), the collection in
> both lowerers (`compiler/lower.rs`, `compiler/reference.rs`, the
> `collector` parameter of the entry points in `compiler/mod.rs`),
> the evaluated expression parsed, resolved and compiled
> (`ns_syntax::parse_code_body`, `resolver::resolve_evaluation`,
> `compiler/evaluation.rs`), `Mutator::mixin_level`
> (`interpreter/instantiate.rs`),
> `bytecode::compiles_alike`, the cache cleared at an install
> (`interpreter/install.rs`), the selector spans
> (`ns_syntax::ast::MessageSend::selector_spans`), the request word's
> bits in `heap.rs` (`REAL_REQUEST`, `STRESS_HOLD`, `DEBUGGER_PAUSE`,
> `DEBUGGER_COMMANDS`, `DEBUGGER_STEP`, `DEBUGGER_STOP`,
> `LOWERS_DEPTH_LIMIT`, `Heap::synchronize_depth_limit`,
> `Heap::clear_debugger_bits`), `heap/machine_words.rs` (the machine
> words as atomics behind an `Arc`), `interpreter/safepoint.rs`
> (`Interpreter::safepoint`, which serves the debugger first,
> `SafepointOutcome`, the pause stress mode's schedule), `vm.rs`
> (`take_safepoint`, `unwind_at_safepoint`), `jit/mod.rs`
> (`deoptimize_stopped_at_poll`, in every JIT build); the patchable
> slots and their originals (`bytecode/compact.rs`,
> `EncodedBody::patch`, `restore`, `original`; `CompiledCode::patch`,
> `is_patched` in `bytecode.rs`), the run loop's `Breakpoint` arm and
> its decision (`vm.rs`; `interpreter/breakpoint.rs`: `reached`, the hit
> counts, the method breakpoints' exits and the reports of a frame left
> otherwise, `frame_left`), the unwinder's reports
> (`interpreter/unwind.rs`, `Mutator::pop_for_unwind`), the fast path
> switched off (`mixin.rs`, `MixinMethod::switch_fast_path`), the
> machine code a patch frees and the patched index lowered as an exit
> (`jit::free_for_patch`, `jit/baseline.rs`, `jit/optimizing/`,
> `jit/cut.rs`), and the handler search's two rows, a leaf and an exit
> (`Kernel.ns`, `builtins/activation.rs`). The DAP agent is
> `crates/ns-cli/src/debug/` (`mod.rs`, its start and the event pump;
> `wire.rs`; `connection.rs`, the connection and its writer thread;
> `agent.rs`, `host.rs`, `protocol.rs`, `stopped.rs`, `sources.rs`,
> `breakpoints.rs`, `stepping.rs`, `evaluation.rs`), the MCP server
> `crates/ns-cli/src/mcp/`, its token drawn by `ns-memory`'s
> `random.rs` and its lines and columns counted by `ns-tooling`'s
> `lines.rs`; the breakable
> lines are `crates/ns-analyzer/src/breakable.rs`; and the plugin's
> debugger is
> `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/debugger/`
> (its DAP client in `dap/`). The tests are in
> `crates/ns-runtime/tests/debugger.rs`, `tests/stress.rs` (the pause,
> breakpoint and step stress modes), `debugger/metadata/tests.rs`, the
> oracle's runners `crates/ns-cli/tests/debug_metadata.rs` and
> `golden_round_trips.rs`, the agent's
> `crates/ns-cli/tests/debug_adapter.rs`, the MCP server's
> `crates/ns-cli/tests/mcp.rs`, and the plugin's
> `DapSessionTest`, `NewspeakDebugProcessTest` and
> `NewspeakBreakpointTypesTest` ("Testing and gates"). The groundwork is
> in `crates/ns-runtime/src/bytecode.rs`
> (`CompiledCode::statement_program_counter`, `CallSite`),
> `bytecode/compact.rs` (`EncodedBody`), `bytecode/leftovers.rs`,
> `vm.rs` (the run loop, its safepoint, `resume_caller`,
> `send_destination`), `types.rs` (`Continuation`, `SourceOrigin`),
> `interpreter/unwind.rs`, `interpreter/install.rs`,
> `jit/materialize.rs`, `jit/mod.rs` (`abandon`,
> `invalidate_applications`, `CopyDependencies`),
> `core-library/Kernel.ns` (the exceptions) and
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
- marking an object and naming it in later evaluations ("Marking
  objects");
- the value the last call returned, shown after a step over or a step out;
- the memory view: the instances of a class and the objects that refer to
  one ("The memory view");
- the shipped modules' frames folded in the stack ("Folding the shipped
  frames").

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
  loop reads its state again, never in a release build; `Terminate`
  when the debugger terminated the run, in every build; and `Unwind`
  when it restarts a frame or forces one to return ("Restart frame and
  force return").
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
  mirror's `terminate…` leaves; an attached debugger stops just before
  `defaultAction` runs ("Exceptions"), and a test-only
  `Interpreter::resume_unhandled_exceptions` (`#[doc(hidden)]`) resumes
  the exception instead, as the debugger's "Create Method" will
  ([core-library.md](core-library.md), "Exceptions").
- **`SourceOrigin`** has `File` and `Embedded` only (`types.rs`).
- **A tuple literal** is lowered to one `MakeArray`, so it makes no
  `fromArray:`, `new:` or `at:put:` send a step could stop in; §5.1.7 lets
  a debugger treat it as one composition of its elements' values
  (`newspeak-spec.pdf` p. 26), never skipping the element expressions,
  which are ordinary statements' parts here.
- **The JetBrains plugin** runs a project or a module through its run
  configurations, debugs one through IntelliJ's debugger ("The JetBrains
  plugin"), and talks to `ns-next-analyzer lsp` through the platform's
  LSP API ([editor-support.md](editor-support.md)).

## The debugging API at the safepoint

Stages 1 to 5 of the debugger ("Planned: the order of building it") are
built: the VM's side of decisions 7 and 9 and of decision 1 but for
`ActivationMirror`; the metadata of decisions 4, 5
and 10 to 15, which give a stopped frame its name, its place and its
named variables ("Reading the stack at a stop", "Compiling the metadata
again"); the breakpoints and steps of decisions 2, 3, 16 and 17;
decision 6's agent with what it asks of the VM -- the remote's two
halves, hit conditions, a module's text by revision, the breakable
statements of a module not loaded, and a pause between runs stopping the
run's first activation at its first statement; and decision 8's
evaluation, with what is built on it -- setting a variable, a
breakpoint's condition and a logpoint's message, marks -- beside a
frame restarted or forced to return, smart step into and the memory
view. It lives in
`crates/ns-runtime/src/debugger/` and is reached through
`Interpreter::attach_debugger`; its clients are the test suite
(`crates/ns-runtime/tests/debugger.rs`), the pause, breakpoint and step
stress modes, and the DAP agent in `ns-cli` ("Launching, the start and
the end"). A program stops for a pause, at a line breakpoint, a run
to cursor's target or a method breakpoint's entry or exit ("Stopping at
and going past a breakpoint", "Method breakpoints"), where a step arrives
("Step over in recursion and blocks"), and at an exception an exception
breakpoint takes ("Exceptions"); at a stop an expression is evaluated in
a stopped frame, and may write its variables ("Evaluation"), a variable
is set, and a frame restarted or forced to return ("Writing a frame",
"Restart frame and force return").

### The attachment and the remote

`Interpreter::attach_debugger` (`debugger/mod.rs`) attaches a debugger to
the interpreter and answers its remote, a `DebugRemote`
(`debugger/remote.rs`), the agent's end, which may move to another
thread. The two ends talk through two `std::sync::mpsc` channels, the
commands to the VM and the events back; the interpreter holds the VM's
end (`Interpreter::debugger`, a `debugger::Debugger`: the command
receiver, the event sender, the id the next stop's first value takes and
the stop an evaluation left to run, kept for it ("Evaluation")).
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
  executes stops the next run's first activation, the method or block the
  host sent, at its first statement start.
- `send(command)` queues a command and then sets the commands bit, so that
  the next safepoint serves it while the program runs, or the stop serves
  it while the program is stopped. A command queued while no run
  executes, or still queued when a run ends, is dropped and answered by
  `Event::Refused`, but a configuration command -- `SetBreakpoints`,
  `SetStepFilters`, `Unmark` -- which the start or the end of a run serves
  ("Commands while the program runs"). A remote whose VM is gone queues
  it nowhere and arms nothing.
- `next_event(timeout)` answers the VM's next event, or `None` once the
  timeout passed or the VM is gone.

The remote is served from another thread than the one running the
interpreter: a stop blocks the running thread until a `Continue` or a
`Terminate` arrives, so a host that pauses and then runs the program on
one thread waits in the stop forever.

An agent that hears the VM on one thread while another commands it
splits the remote (`DebugRemote::into_parts`) into its two halves: a
`DebugControl`, which pauses and queues commands and is `Clone`, `Send`
and `Sync`, so that several threads command the VM each through its own
handle, and a `DebugEvents`, which moves to the thread that waits for the
VM's events. The VM notices that every control is gone where it next
takes commands -- a drain or a stop -- which the last control's drop
asks for, closing the command channel and then arming the commands bit
(`Drop for DebugControl`), so that a program that runs on without a
stop, an endless evaluation among them, notices too; and that the events
are gone at the next event it cannot send; either detaches the
debugger.

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
- `Evaluate { frame, expression }` -- the expression, assigning with
  `::`, evaluated as a block standing in the activation at index `frame`
  of the stack trace (decision 8, "Evaluation"); answered by
  `Event::Evaluated` -- `Evaluation::Value`, a value of the stop, or
  `Evaluation::Error`, the error that ended it and its exception -- the
  program still stopped, or refused with "no frame at that index", "the
  frame is the core library's", "the frame's source is not known", "the
  expression does not compile: <message>", "<name> is not assignable",
  "a ^ cannot leave an evaluation", "<name> is captured by a block made
  before this point" ("Assigning a captured variable") or, at the depth
  limit, "the stack has no room for the evaluation".
- `SetVariable { frame, name, expression }` -- sets the variable `name`
  of the activation at index `frame` to what `expression`, one
  expression, answers, by evaluating the assignment `name:: expression`
  as `Evaluate` does ("Writing a frame"); answered by
  `Event::VariableSet`, the variable as `Variables` now shows it, by
  `Event::Evaluated` with the error the expression ended with, or refused
  as `Evaluate` is, "<name> is not assignable" also for a name that is
  not an assignable variable the frame shows -- `self` among them -- and
  "the expression does not compile: a variable's new value is one
  expression" for statements or temporaries.
- `RestartFrame { frame }` -- runs the activation at index `frame` again
  from its start, every frame above it unwound and its `ensure:` blocks
  run ("Restart frame and force return"); answered by `Continued`, then
  the stop at the frame's first statement start, a step's, or refused,
  the program still stopped, with "no frame at that index" or "the frame
  is the core library's".
- `ForceReturn { frame, value }` -- makes the activation at index
  `frame` return `value`, a value of the stop, or `nil` for `None`, to
  its sender, every frame above it unwound; answered by `Continued`, then
  the stop where the sender goes on, a step out's, or refused, the
  program still stopped, as `RestartFrame` is, with "no value of this
  stop has that id", or "an initializer returns self" for a value given
  to an initializer's frame.
- `StepInTargets` -- where a smart step into from the top frame -- the
  frame a step steps from, the first shown below an exception's
  machinery -- may stop (decision 3, "Smart step into a block"): the
  sends of the frame's statement from where it stands -- past the send
  a frame below the top waits on -- to the statement's end, in the order
  they run, an inlinable form's own selector left out and its blocks'
  sends listed in its place, then the block literals those sends take
  that run as closures; answered by `Event::StepInTargets`, a
  `StepInTarget` each -- `id`, its position, valid until the program
  goes on; `label`, a send's selector (`at:put:`) or `[] in ` and the
  selector of the send that takes a block; `span`, the selector's first
  token or the block literal; `kind`, `Send` or `Block` -- empty for a
  frame without metadata, a factory's and an initializer's at its
  header stop.
- `StepIntoTarget { target }` -- a smart step into aimed at the target
  of that id; answered by `Continued`, then the step's stop, or refused,
  the program still stopped, with "no such target".
- `SetBreakpoints { module, breakpoints }` -- replaces every line
  breakpoint of the module `module` names, its id and version as
  `FrameLocation::module` names it ("The agent's sources"), by
  `breakpoints`, each a
  `LineBreakpoint { offset, hit_condition, condition, log }`: a byte
  offset into the module's current text ("A breakpoint names a source
  position"), which of its hits stop, every one for `None`, the
  condition a hit is counted only where it answers `true`, and the
  message, `LogPart`s, a logpoint logs instead of stopping ("Hit counts
  and temporary breakpoints"); answered by `Event::Breakpoints`. A configuration
  command: served wherever commands are taken, at a stop, at a safepoint
  while the program runs, and at the start and the end of a run.
- `SetStepFilters { shipped, classes }` -- replaces the step filters
  (decision 16): `shipped` filters the shipped modules' bodies, and each
  `ClassName { module, path }` of `classes` the bodies of the class
  declaration it names and of every declaration nested in it, `module`
  the module's id, and its version where only that version is meant
  (`ModuleName`), and `path` the names of the
  declarations from that root down, the root's own excluded, so that
  an empty path names the whole module. Until the first, the shipped
  modules are filtered and no class is, and every attachment begins
  with that default. Answered by `Event::StepFiltersSet`; a
  configuration command, served as `SetBreakpoints` is.
- `SetMethodBreakpoints(Vec<MethodBreakpoint>)` -- replaces every method
  breakpoint ("Method breakpoints"), each naming a method by its
  `class`, a `ClassName` as the step filters name a declaration, its
  side (`class_side`) and its `selector`, and its `hit_condition`,
  `condition` and `log` as a line breakpoint's, which count and decide
  each entry and each exit; answered by
  `Event::MethodBreakpoints`, a configuration command served as
  `SetBreakpoints` is.
- `SetExceptionBreakpoints { uncaught, caught, halt, classes }` --
  replaces the exception breakpoints ("Exceptions"): `uncaught` stops an
  exception no handler takes before its `defaultAction` runs, `caught`
  one a handler takes before the handler runs, each only for an
  exception whose class chain holds a class of a declaration `classes`
  names -- `ClassName`s, as the step filters name a declaration -- or for
  every exception when `classes` is empty; `halt` stops a `Halt` no
  handler takes, which then goes on resumed with nil, and with `halt` off
  a `Halt` is an exception as any other. Until the first, and at every
  attachment, `uncaught` and `halt` are on, `caught` is off and no class
  is named. Answered by `Event::ExceptionBreakpointsSet`; a
  configuration command, served as `SetBreakpoints` is.
- `ModuleText { module, known_revision }` -- the current text of the
  module `module` names, a shipped module's too, built or not, unless
  `known_revision` is the revision the answer would carry; answered by
  `Event::ModuleText { module, revision, text }`, `text` the text, or
  `Unchanged`, or `None` for a module not built or without text (an
  image built with `--strip-source`). The revision is the interpreter's
  count of installs and of builds of a module's root class, one count
  for every module: a text read at a revision is current until it is
  raised, so that an agent asks again only whether it changed. Served
  only at a stop, where the agent reads positions in it.
- `FrameCode { frame }` -- the code of the activation at index `frame`
  of the stack trace ([developer-tool.md](developer-tool.md), decision
  15, "A stopped frame's code"); answered by `Event::FrameCode { body,
  bytecode, machine }`: the frame's name as the stack trace names it,
  its body's bytecode marked at the instruction it stands at, and, for a
  frame that goes on in machine code, the listing of its baseline code
  or its twin, marked where the call it waits on returns
  (`jit::frame_code_listing`), `None` otherwise; or refused with "no
  frame at that index". Served only at a stop.

The events (`debugger::Event`): `Stopped { reason, returned, elsewhere }`,
the program stopped and serves commands until a `Continue` or a
`Terminate` -- `StopReason::Pause`, `StopReason::Breakpoint { ids }`, the
breakpoints standing at the statement it stopped at, `StopReason::Step`, a
step's or the run to cursor's arrival, or `StopReason::MethodEntry { id,
others }` and `StopReason::MethodExit { id, others }`, a method
breakpoint's ("Method breakpoints"), or `StopReason::Exception {
class_name, description, caught, exits }` and `StopReason::Halt { exits
}`, an exception breakpoint's, `exits` the method breakpoints whose exit
by an unwind it reports ("Exceptions", "Method breakpoints"), or
`StopReason::ConditionFailed { ids, error }`, the breakpoints standing
there whose condition or log expression did not compile or raised an
error, and the first failure's description ("Hit counts and temporary
breakpoints"); `returned`, at a step's stop, a `ReturnedValue`: the value
the stepping frame's last call answered, described as the stop's first
value, and the call that answered it as a reader names it, a `String`;
`None` at every other stop ("The return value of the last call");
`elsewhere`, at a step's stop, whether the step went on as a step into
from where the program went on rather than from the frame it was taken in,
so that it stands in another activation (decision 4), and `false` at every
other stop; `StackTrace`; `Variables`, the answer to both `Variables` and
`Children`; `Refused { command, reason }`, the command `Command::name`
names not served; `Continued`; and `Breakpoints { module, breakpoints }`,
the module's line breakpoints in the order of their offsets, each a
`BreakpointState` -- its `BreakpointId`, its offset and the span of the
statement it stops at, or `None` while it is pending -- the answer to
`SetBreakpoints`, and sent unasked when a pending one resolves or an
install moves one; `StepFiltersSet`, the answer to `SetStepFilters`;
`MethodBreakpoints`, each method breakpoint's `MethodBreakpointState` --
its id and whether its method is declared (`verified`) -- in the order
they were set, the answer to `SetMethodBreakpoints`, and sent unasked when
a module's build or an install changes whether one is verified;
`ExceptionBreakpointsSet`, the answer to `SetExceptionBreakpoints`;
`ModuleText`, the answer to `ModuleText`; `FrameCode`, the answer to
`FrameCode`; and `Logged { breakpoint, text
}`, sent unasked where a logpoint holds, its message made, while the
program goes on. `Continue`, the three steps, `RunToPosition`,
`StackTrace`, `Variables`, `Children`, `ModuleText` and `FrameCode` are
served only at a stop; one that arrives while the program runs is
refused with "the program is running", and one queued between runs is
dropped and refused with "no run executes". Every command a stop or a
safepoint serves but `Terminate` is answered by one event, in the order
the commands came. An agent therefore waits for `Stopped` before it
sends a command only a stop serves: one sent right after `pause()` is
refused or served depending on whether a safepoint drains the queue
before the stop or the stop serves it.

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
  arming lock as the debugger's other bits are cleared;
- `DEBUGGER_STOP` (bit 5) -- an exception breakpoint's stop, which the
  handler search's rows recorded on the `Mutator` and ask of the next
  poll ("Exceptions"): the VM's own, set (`Heap::request_debugger_stop`)
  and cleared on its thread alone, under the arming lock, by the
  safepoint that takes the stop, and dropped with the stop at a
  termination and at the run's end (`exceptions::drop_pending`).

Every write of the word, the VM's and the agent's, is a `SeqCst`
read-modify-write -- a `fetch_or` to set a bit, a `fetch_and` to clear one,
a locked instruction on x64, an atomic or exclusive-pair sequence on
AArch64, on the cold path only -- so that no writer erases another's bit.
The poll compares the whole word with zero: every bit takes the safepoint.
`Heap::real_safepoint_requested` masks it with `LOWERS_DEPTH_LIMIT`
(`REAL_REQUEST | DEBUGGER_BITS`), every bit but the stress hold's, and
`Heap::debugger_bits` with the debugger's four (`DEBUGGER_BITS`). The VM
clears its own bit and the hold once it served them
(`Heap::clear_safepoint_request`), never the debugger's; only
`Heap::clear_debugger_bits` clears those, once the VM served them, or
once the push a step into waited for came or the step ended; an
attachment and a detach clear all four. The agent never clears a bit.

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
The breakpoint's stop was decided by the arm, before the commands: once
they are served it is decided again from the table
(`PendingStop::still_standing`), so that it reports only the
breakpoints still standing at its slot, and is not made when none is
left and no step's one-shot stops there -- the frame fetches the slot
again as it goes on, and what stands there decides afresh, counting
no hit the first reach counted ("Hit counts and temporary breakpoints";
`a_breakpoint_removed_before_its_stop_is_served_is_not_reported`).

### Commands while the program runs

With the commands bit set, the queue is drained
(`serve_queued_commands`): the bit is cleared first, under the arming
lock, then every queued command is taken without waiting (`try_recv`). A
`Terminate` ends the run, and the commands queued after it are dropped
at once, answered by `Event::Refused`, but for a configuration command;
a `SetBreakpoints` is served -- the breakpoints patched or restored,
answered by `Event::Breakpoints` -- and so is every other configuration
command, `SetStepFilters`, `SetMethodBreakpoints`,
`SetExceptionBreakpoints` and `Unmark` ("Marking objects"), and the
program runs on; an
`AbortEvaluation` aborts the evaluation that runs, and is refused, "no
evaluation runs", where none does ("Evaluation", "Aborting"); any other
command is refused, "the program is running". The agent queues a command before it
sets the bit, and the VM clears the bit before it drains the queue, so
that a command queued while the queue is drained keeps its bit set for
the next safepoint; the other order would lose it, a `Terminate` sent to
a fully compiled loop included.

A command that acts on a run is served by the run it arrives during.
`run_from_host` clears the commands bit, under the arming lock, and then
takes every queued command, before a run starts and once it ended
(`Interpreter::serve_between_runs`), in the order the drain keeps and for
its reason: a command the drain misses keeps its bit and is served, or
dropped, at the next safepoint. A configuration command is served there,
so that breakpoints an agent sets before the program runs, or between two
runs, are set when the next run starts; every other command is dropped and
answered by `Event::Refused`. Without the drop, a command left queued -- a
`Terminate` sent after its run ended, or queued after the one that ended
it -- would otherwise end the next run at its first safepoint, or be taken
by its first stop, a run nobody terminated
(`a_terminate_after_its_run_ended_does_not_end_the_next`,
`a_terminate_queued_after_the_one_that_ended_the_run_ends_no_other`). A
pause is no command: one asked between runs stays, and stops the next run.
It stops the run's first activation at its first statement start, not at
the run's first safepoint, the first push that activation makes, so that a
client's stop on entry stands in the method the host sent: the run's start
(`Interpreter::serve_before_run`, cold) floods the activation as a step
over does, before it runs anything, and the first one-shot it reaches --
its first statement's -- makes a stop reported as the pause's, which
serves the pause bit; the bit stays set meanwhile, so that a body without
metadata, which floods nothing, stops at the first safepoint as before,
and the stop is made whatever the step filters say, as a pause's is
(`a_pause_between_runs_stops_the_sent_method_at_its_first_statement`).
A factory the host sends, which has no statement start, is stopped at
its entry by a one-shot at its first instruction (decision 12;
`a_pause_before_an_instantiation_stops_in_its_factory`).
A step is no command either: one a run's end left in progress -- its
frame returned to the host, or was popped there, without a stop -- goes
on in the next run (decision 17). The run's end ends it, its frames gone,
and carries its method exits on the breakpoint table
(`stepping::run_ended`); the next run's start, without a pause pending,
goes on with it as a step into from the run's first activation
(`stepping::carry_into`), which stops at its first statement start, a
step's stop. A `Terminate` between runs, refused all the same, ends the
carried step, as it would end one in a run, and so do a detach and a
pause asked between runs, whose stop the next run's start makes instead
and which reports the carried step's method exits, as a pause in a run
reports those of the step it ends
(`a_step_that_leaves_its_run_stops_at_the_next_run_s_first_statement`,
`a_terminate_between_runs_drops_the_step_its_run_left`,
`a_pause_between_runs_reports_the_exits_the_last_run_carried`).

Every command dropped between runs is answered by `Event::Refused`, its
reason that no run executes, so that every command the debugging API
takes has exactly one answer, as every DAP request has one response and
JDWP answers a command it cannot serve with an error (`VM_DEAD`,
`THREAD_NOT_SUSPENDED`; `a_command_queued_between_runs_is_refused`).

`ns-next run` is several host runs -- a project's manifest instantiated,
the root module, then `main:args:` (`crates/ns-cli/src/program.rs`) -- so
a `terminate` can arrive between two of them, where the drop would let
the program go on (decided with the user on 2026-11-14). The DAP agent
records a `terminate` in `ns-cli` as well as queuing it
(`crates/ns-cli/src/debug/host.rs`), and the host checks the record
before each run it starts and ends the program as a terminated one, with
the line and the status a terminated run reports, `exited` sent with it
(`a_terminate_between_runs_ends_the_program_before_main`). A `Terminate`
the VM refuses because no run executes is sent again while the record
stands, so that one that reached the queue just after the host's check,
and that the start of the run then dropped, still ends that run; once no
run follows, it waits in the queue unserved. The VM's drop and its two
tests stand, since only the host knows which runs are one program.
Decided with the user on 2026-11-14: the agent sends a command that acts
only at a stop -- `StackTrace`, `Variables`, `Children`, the steps,
`RunToPosition`, `Continue` -- only while the program stands at one it
showed the client, and answers such a DAP request itself while the
program runs, with the error "the program is running", never reaching
the VM (`a_stack_trace_while_running_is_an_error`). The VM's refusal
stays for an agent that sends one all the same.

Once the queue is drained, `serve_debugger` reads the pause bit again;
if it is set, it is cleared, under the arming lock, and the program stops
("The stop" below).

A leaf that runs long -- a large `LargeInteger` multiplication, a large
copy -- delays the next poll, and with it the pause and a `Terminate`,
until it returns. No leaf waits for input today. One that will -- a read
of the standard input -- waits in slices and answers "re-execute"
between them, before it changed anything, so that the safepoint serves
a pause and a `Terminate` while it waits (decided with the user on
2026-11-14). A program that reaches no safepoint is ended the way DAP
provides ("Launching, the start and the end"): `terminate` stays
orderly, and `disconnect` with `terminateDebuggee` ends the process at
once.

The DAP request `setBreakpoints` arrives while the program runs, as
`pause` and `terminate` do, and becomes `SetBreakpoints`, which the next
safepoint serves from the queue without stopping
(`a_breakpoint_set_while_running_takes_effect`). `disconnect` is no
command of the VM's: a connecting agent drops both halves of the remote,
which detaches the debugger with every breakpoint removed ("Detaching"),
and a listening one, which serves another client next, sends the
configuration commands that remove every breakpoint and continues a
stopped program; either way the program runs on ("Launching, the start
and the end").

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

The stop then sends `Stopped { reason, returned, elsewhere }` -- a pause's, a
breakpoint's with the ids standing at its slot, an exception
breakpoint's ("Exceptions"), or a step's with the
value of the last call, the first value of the stop's table ("Values the
IDE holds") -- and blocks on the
command channel (`recv`), on the VM's thread, between two stretches: no
handle is live, and nothing pushes or pops until the program goes on. The
stack is the activations on the activation stack and, below them, the
senders a suspension left in their frame objects ("Reading the stack at a
stop"). It serves `StackTrace`, `Variables`, `Children` ("Reading the
stack at a stop", "Values the IDE holds"), `Evaluate` ("Evaluation"),
`SetVariable` ("Writing a frame") and the configuration commands until a
`Continue`, a step, a `RestartFrame`, a `ForceReturn` ("Restart frame
and force return") or a `Terminate`; a step begins there
(`stepping::begin`), is answered by `Continued` and goes on as a
`Continue` does. A command the stop serves was queued with its
bit set, which stays set: once the program goes on, the next safepoint
clears it and finds the queue empty. A pause asked while stopped likewise
stays set, and stops the program again at the next safepoint.

The stop is a `debugger::Stop`: where it stands -- whether it is an
exception's and hides its machinery, a caught one's, a header stop --,
the run's floor, the agent it serves, the values it handed out, the
frames it walked and what it owes the program once it goes on -- the
slot its frame stands at, gone past once, and whether the run loop reads
the frame on top again (`AfterStop`). An `Evaluate` or a `SetVariable`
leaves it with `Served::Evaluate`, kept in the `Debugger` while the
expression runs, with the name of the variable a `SetVariable` sets
(`Stop::setting`), and the safepoint the evaluation's end asks for serves
it again (`Interpreter::serve_evaluation`): it answers `Evaluated`, or
`VariableSet` for a `SetVariable` whose expression answered, and goes on
serving, its value ids still valid, with no second `Stopped`. Its frames
are walked again after an evaluation, which may have moved them, and the
slot it owes the top frame is named by the top frame as it is then.

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
(`vm::take_safepoint`) calls `vm::unwind_at_safepoint`, which raises the
termination in the top activation (`debugger::termination`: a
`Signal::Terminate` whose error has the selector `terminate`, the top
activation's receiver and the message "the debugger terminated the
program") and hands it to the one unwinder (`interpreter/unwind.rs`) with
the run's floor, as the termination a leaf raises is handed: every pending
`ensure:` and `ifCurtailed:` block runs on the way, each started
interpreted, and once the floor is popped the host's send answers the
error (`HostSignal::Error`; `terminate_runs_the_pending_ensure_blocks`,
`a_terminate_queued_while_running_ends_the_run`).

A breakpoint in one of those `ensure:` blocks still stops, and its stop
reads the stack and values as any does, but refuses `Evaluate`,
`SetVariable`, `RestartFrame` and `ForceReturn` with `"the program is
terminating"`: an evaluation's error would take the termination's way
past its base, ending the run before the evaluation answered, and an
unwind the stop started would race the termination's own. A condition or
log expression at such a breakpoint is evaluated as anywhere: the
evaluation a termination ended is no longer running
(`evaluation::terminated`), so only it goes past its base, and one begun
during the termination ends at its own. No stop kept for an evaluation
and no decision of conditions outlives the run
(`Interpreter::drop_kept_at_run_end`, debug-asserted), so the next run
of the same interpreter is served as any
(`a_stop_during_a_termination_refuses_evaluations_and_the_next_run_stops`,
`a_condition_failing_during_a_termination_stops_and_the_next_run_stops`).

### Detaching

Dropping the remote detaches the debugger. The VM notices it where it next
talks to the remote -- a command channel found disconnected while draining
or while stopped, or an event that cannot be sent -- and the last
control's drop makes that next talk come soon: it closes the command
channel and then arms the commands bit (`Drop for DebugControl`), so that
a running program drains, finds the channel disconnected and detaches at
its next safepoint, and an evaluation running at a stop, which may never
end, is aborted there ("Evaluation", "Aborting"). Then `detach_debugger`
makes no attachment current and clears every debugger bit under one hold
of the arming lock, removes every breakpoint, each slot given its original
back but one the breakpoint stress mode patched too, which keeps that
patch ("The breakpoint stress mode"), ends a step in progress and puts the
step filters and the exception breakpoints back to their default, then
drops the debugger. A stopped program goes on as after a `Continue`, and a
running one is never stopped again
(`a_dropped_remote_lets_the_program_finish`,
`a_remote_dropped_while_stopped_lets_the_program_finish`,
`a_detach_during_an_evaluation_aborts_it`).

### Reading the stack at a stop

**Partial:** a stop answers the run's frames and, for each, its name,
its receiver and, from its body's metadata ("Compiling the metadata
again"), the statement and the send it stands at in its module's current
text and its named variables over its whole lexical chain (decisions 10,
12 and 13); a frame whose body has no metadata shows its arguments and its
registers below the first scratch register, by number. A frame is
written by an evaluation's assignment and a variable set ("Writing a
frame").

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
  factory, `top level`, `Foo` the declaration written as every tool
  writes a class (`docs/design/modules.md`, decision 8): its module's
  id, `@` its version whenever it has one, and `/` before the path to a
  class inside the module -- `com.acme.util.Util@1.0.0/Inner>>run`,
  `[] in Demo>>main:args:`, `newspeak.core.Kernel/Array>>do:` -- so that
  each frame names one module (`metadata::again::qualified_name`,
  `frames_are_named_with_id_and_version`); after its current class where
  no declaration holds the body;
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
  for the return it ends with -- or a factory's class header pattern,
  and an initializer's at its header stop (decision 12), and for a frame
  below the top the receiver and selector
  spans of the send it waits on; every span in the module's current text
  (decision 11). `None` without metadata (decision 5): a module without
  text, an image's without its source among them, or a body its text no
  longer compiles to, as a superseded version an edit changed;
- `receiver` -- its receiver, described as every value is;
- `hidden` -- whether a client shows it only on demand: at an uncaught
  exception's and a `Halt`'s stop, every frame above the first one below
  the exception's machinery (`exceptions::first_shown`), and at a caught
  exception's, every frame between the handler block on top, which is
  shown, and that first one below the machinery (`exceptions::hidden`,
  "Exceptions"); false at every other stop.
  A hidden frame stays in the trace, so that every index names the same
  frame for `Variables`.

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
(`SendEntry`: the index, the `CallSite`, the receiver's span, the
selector tokens' spans, the call's span, whether a receiver spelled
apart from it is named in parentheses and whether it is a setter
assignment's), its sends that take block literals (`BlockSendEntry`:
each block taken, a `TakenBlock` of its span and its
place -- `BlockPlace::Inline`, the instructions it lowered to, or
`BlockPlace::Called { send, closure }`, the send that runs its closure and
the `MakeClosure` that made it, an index that names the running body's
block where an install moved the spans -- whether the send is an inlined
form, and the index after it), its named variables (`VariableEntry`: the
name, the lexical depth in frame scopes, and the place, a `VariablePlace`
-- `Register`, `Outer { hops, register }` or `Vector { vector, slot }`,
the vector held in a register of the frame or of one `hops` out), the
body's own first, parameters before temporaries, then each enclosing
scope's outward, and, for a factory, the header's factory pattern
(`factory_pattern`), for an initializer body the same, where its header
stop stands (`header_pattern`). Every span is in the module's current text (decision
11).

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
  that a field added to it must be decided. A patched slot is compared
  by its original, which the decoded view answers ("Where the original
  lives"), so that a breakpoint never costs a body its metadata.
- **A factory's body** (decision 12) takes the metadata `factory_again`
  makes from the class header: the header's parameter names in the
  registers a factory takes its arguments in, no statement start and no
  send, and as its place the header's factory pattern, or the class's
  name for a header that writes none. Any body that compiles alike with
  `compiler::factory_body` of the header's arity takes it, a factory body
  no mixin holds -- a host's `instantiate` builds its own -- included.
  Each initializer body of the level, not its blocks, takes the same
  place as its `header_pattern`, where its header stop stands; the
  metadata's check (`metadata/check.rs`, decision 14) holds every
  initializer body to having one within the text and every other body
  to having none.
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
instance's fields and an `Array`'s elements, paged, each saying whether
it is withheld and its mark; a marked object is held past its stop
("Marking objects"). Pinning any other value past its stop, the
elements of a `ByteArray` and a `String`, and a Newspeak collection's
renderer are planned (below).

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
(`Heap::class_display_name`), how many children it has and whether they
are elements, decided by the object's shape rather than its class's name,
which a program's own class may share (`ValueDescription::indexed`),
whether it is withheld -- decided by the way it came, which the table
keeps beside its `Root` for its children to inherit -- and the label it
is marked with ("Marking objects").

`Children` answers, from `start` and at most `count` of them: for an
instance, its fields (`Heap::instance_field`), in field order, each
named as the program declared it; for an
`Array`, its elements, named `[1]` up as `at:` counts
(`Heap::array_element`; `an_array_s_children_are_its_elements_paged`).
Every other value has none: an immediate, a `ByteArray`, a `String`, and
a reified mixin, whose one field is an id that tells a reader nothing.
An instance whose class declares no slot and no nested class has no
field and so no children, and the agent gives it no
`variablesReference`, exactly as it gives one to every value with a
child (`only_an_element_with_fields_expands`). Each child is described,
and held, as every value is. The heap's accessors
alone describe a value: no Newspeak code runs while the program is
stopped.

An instance's fields are named from the layout of its class's application
(`values.rs`'s `field_labels`, over `MixinApplication::field_layout`, the
one walk of the layout, which an install copies fields by too;
`object-model.md`, "Mixin application"): each level of the chain, from the
root, lays its own slots down at its slot base in declaration order, then
one hidden slot per nested class it declares, so a field is named by its
slot's name or by its nested class's (`KindA`), a superclass's slots
included and a shipped class's alike
(`an_instance_s_fields_are_named_by_its_slots_and_nested_classes`,
`an_instance_s_fields_include_its_superclass_s_and_name_a_shadowed_one`).
The names are the mixins' structure, never their source, so an image built
with `--strip-source` names them too
(`a_stripped_image_names_an_instance_s_fields`). A name more than one
level declares -- a superclass's private `x` under a subclass's `x`, a
nested class declared again -- is the plain name on the level nearest the
object's class, the one its own code reads, and `x (Base)` on each level
above, the declaring mixin's name added. Where `<:` clauses apply one
mixin more than once, its levels are told apart by number, counted by the
mixin's id from the nearest shadowed level upward: `x (M)`, then `x (M 2)`
(`a_mixin_applied_more_than_once_numbers_its_shadowed_slots`). Two
different mixins that share a name, each shadowing the same name, are not
numbered: each is named by its declaration's path from its module root
instead, `x (Outer.Base)` and `x (Other.Base)`, and by that path after
its module, as every tool writes a class ([modules.md](modules.md),
decision 8), where even the paths are alike: `x (Library/Outer.Base)` and
`x (Kit/Outer.Base)`, and for two versions of one module, which share
their id, `x (com.acme.Library@1.0.0/Outer.Base)` and
`x (com.acme.Library@2.0.0/Outer.Base)`, while a mixin whose name no other
shares there keeps the short form
(`two_shadowed_slots_of_mixins_sharing_a_name_are_named_by_their_paths`,
`shadowed_slots_of_modules_sharing_a_path_are_named_after_their_modules`,
`shadowed_slots_of_two_versions_of_a_module_are_named_by_their_versions`).
Only that form holds a `/`, so it never spells another mixin's path from
its root (`shadowed_slots_across_their_tiers_are_never_alike`). Where two
labels still read alike -- the roots of two modules a host loaded without
an identity, each named by its class's name -- those mixins are told apart
by their places in the chain, numbered from the nearest upward,
`x (Library/Outer.Base #1)` and `x (Library/Outer.Base #2)`, so that no
two labels are ever alike
(`shadowed_slots_of_modules_sharing_a_name_and_a_path_are_numbered`).
A field no declaration names is `field <index>`, which only an instance
whose class holds no application, one with more fields than its
application lays out, or one whose layout names a mixin the heap holds no
id for would show; none happens to an instance the VM made, and the last,
which the install treats as a broken invariant, the query answers by
number rather than fail on (`MixinApplication::checked_field_layout`;
`a_layout_naming_an_unregistered_mixin_numbers_the_fields`). Paging counts
the named fields as it counts elements (`named_fields_are_paged`).

A field that holds no value yet is shown as not computed rather than as
its word, `nil` (`VariableValue::NotComputed`): a lazy
slot (§6.3.3) its getter has not filled -- `nil` is what its getter reads
as unfilled (`Instruction::ReturnSlotIfFilled`) -- and a nested class's
hidden slot before its accessor built the class, or while it builds it.
A lazy slot whose initializer answered `nil` reads as not computed too,
and rightly: §6.3.3's getter evaluates the initializer whenever the slot
contains `nil`, so its next read computes it again.
No stop computes one, since no Newspeak code runs at a stop; one that
holds a value shows it (`a_lazy_slot_not_computed_yet_is_shown_so`:
the platform's facets, of which only `transcript`, used, holds one). The
agent shows such a field's value as `<not computed yet>`, without
children and without DAP's `lazy` hint, which promises a value fetched
through the variable's reference
(`an_instance_s_fields_are_named_and_a_lazy_slot_not_computed_says_so`).

An object the debugger marks outlives its stop, held by the attachment
until it is unmarked, as JDWP's `DisableCollection` holds an object id
("Marking objects"); a value is pinned past its stop in no other way --
an id the IDE keeps names nothing at a later stop -- which is still open.
Planned: a `ByteArray`'s and a `String`'s elements are read by the
heap's accessors and paged; and a Newspeak
collection (`List`, `Map`, `Set` of `CollectionsFactory.ns`), which shows
its slots, is shown by a renderer that runs Newspeak code (`size`, `do:`,
`printString`) as an evaluation ("Evaluation") under a limit of time and
sends, skipping breakpoints as every evaluation does.

### The pause stress mode

The pause stress mode (`NS_NEXT_STRESS_PAUSE`, debug builds, one of the
`stress` gate's fifteen modes; `Interpreter::set_stress_pause` for a
test, `Interpreter::stress_pauses` the count of its stops) stops the run
at the safepoints its schedule picks -- paced as the collect and suspend
modes are (`Pacing::STANDARD`, `StressSchedule`,
`interpreter/safepoint.rs`), but fed the traced size plus eight times the
stack's words (`size_with_stack`, `PAUSE_STACK_WEIGHT`), since a stop reads
every frame and the cost of a stop grows with the stack: measured at 70 to
180 ns per stack word in a debug build, several times a move's cost per
traced unit, a golden recursing to the depth limit spends 99% of its run
in the stops, so that the weight makes a deep stack's stops five to
eight times rarer than counting a stack word once did, while a shallow
program's, which cost little, are paced by its heap -- exactly as a pause
does: right after the
debugger is served, before anything else at the safepoint changes the
stack or the heap, a frame its optimized code's poll left deoptimized
first. No debugger need be attached: the stop takes its commands from a
`CommandSource`, the attached debugger's channels or the mode's reader
(`debugger::StressReader`), which asks for the stack trace, the variables
of every frame and the first eight children of every value a variable has,
evaluates the first eight named variables of the top frame that have a
value or are shown as not available, then continues; a refusal is the
stop misreading its own stack, and panics. The stack trace and the
variables are read through the metadata
as an agent's are, so every frame's body is found by its declaration and
compiled again on the first stop in it, and its name, its location and
its named variables, the outer chain of a block included, are read at
real stops; a variable not available has no value to ask children of.
Each evaluation (`Command::Evaluate`, the variable's name as the
expression; "Evaluation") must answer the value the variables view
showed -- its class, its label, its child count and whether its children
are elements, since a stop hands out a fresh id for every value it
describes -- or, for a variable the view showed as not available, be
refused with exactly `"<name> is not available here"`, or be refused for
a reason the stopped frame gives, a frame without metadata, a stack
without room for the push or a stop during the debugger's termination;
any other answer panics, naming the variable, the frame and both
answers. The top frame is
evaluated only when it is no shipped one, which refuses every evaluation,
and only at a stop made while no evaluation runs or waits for its stop,
no step is in progress and no breakpoint's token waits for its frame's
fetch, since evaluations do not nest, a step would take the
evaluation's push for its own, and the suspend stress mode follows a
token only to the top frame it resumes, which under an evaluation is not
the token's (`Breakpoints::waits_for_a_fetch`;
`a_stress_pause_beside_suspensions_leaves_a_token_to_its_frame`). The stop is kept for the
evaluation as the attached debugger's is, in `Interpreter::stress_stop`
instead of the `Debugger`, the safepoint answering
`SafepointOutcome::Rebuilt` so that the run loop reads the evaluation's
frame; while it runs, a safepoint serves the commands an attached
debugger queued and leaves its pause for later, since the debugger never
sees the stress stop. Each body and name is compiled once
(`evaluation::StressCompilations`): an evaluation's block is a body of
its own, which takes a code id as it is pushed, so that compiling the
same name at every stop would grow the code id table to its limit and
make collections no program makes without the mode, which the goldens'
table check refuses.
While the mode is on it holds the stress bit
(`hold_safepoints_for_stress`), so that every poll comes to the
safepoint. Every golden and the corpus run under it, so reading a stopped
stack and evaluating its top frame's variables are shown never to change
a program's outcome nor to read a register the leftover analysis calls
dead, which a debug build refuses -- a variable table that named a dead
register included;
`the_pause_stress_mode_keeps_outcomes` (`tests/stress.rs`) holds a program
to its outcome under both lowerings with the mode on and off.

### The breakpoint stress mode

The breakpoint stress mode (`NS_NEXT_STRESS_BREAKPOINTS`, debug builds,
one of the `stress` gate's fifteen modes;
`Interpreter::set_stress_breakpoints` for a test,
`Interpreter::stress_breakpoint_hits` the count of the patches gone past
since it was switched on) runs every program through the `Breakpoint` arm
at the statements of every body it finds running at a safepoint, which is
every body whose first entry is interpreted (below). No debugger need be
attached. At every safepoint, right after the pause and the step stress
modes' stops, `breakpoints::patch_running_for_stress` walks the running
activations from the top and patches every statement start of each body it
has not resolved yet -- the first instruction of every statement its
metadata names, an inlined block's included -- with a breakpoint of its
own kind (`PatchKind::Stress`), which never stops: the arm counts the hit
and executes the original through its ordinary arm, as for a slot nothing
stops at. The walk stops at the first body it resolved before, which keeps
a deep recursion's safepoints from walking its whole stack. A body without
metadata -- an image's without source -- is resolved to no slot. The
patches change through `settle`, so a patched body loses its machine code
and is compiled again with an exit at every statement start, and is never
optimized nor inlined; under the JIT the mode thus runs every exit at a
patch and every abandonment it leads to.

What brings a body to the walk is its first entry. While the mode is on
it holds the stress bit (`hold_safepoints_for_stress`), which the
interpreter's poll after a push takes, so an interpreted push comes to
the safepoint with its frame on top. A body is compiled at its second
entry, so its first runs interpreted and is resolved there, before any
machine code of it exists; and a patched caller exits at every
statement start, so its sends are the interpreter's. The hold is not a
real request: it lowers no depth limit (`LOWERS_DEPTH_LIMIT`), so a push
in machine code never takes it, and an interpreted push that switches
into a body's machine code is polled only once that code has run. The
mode therefore misses a body whose first entry is in machine code --
under the JIT stress mode, which compiles at the first entry, or one a
body without metadata, whose machine code the mode leaves alone, calls
directly -- unless the body is on the stack at a later safepoint above
every resolved frame, at a poll of its own loop, say; and a method with
a fast path the send answers without a push is never on the stack at
all.

The mode's patches are its own, not a debugger's: an attachment, a
detach and `clear` leave them, a line breakpoint and a stress patch at
the same slot stand side by side -- the line breakpoint stops, the
stress patch never does -- and a method's fast path comes back once the
last breakpoint but the mode's leaves its body. Like a debugger's, the
patches keep their bodies and their code ids for as long as the mode is
on; switched off (`breakpoints::end_stress`), every slot only the mode
held gets its original back. The table of resolved bodies holds each
weakly, as the metadata cache does, so that no other body takes a
resolved one's address and a dead one is forgotten.

Every golden and the corpus run under it, under both lowerings, with the
JIT and with `NS_NEXT_NO_JIT=1`, so that a patched slot at every
statement start, each executed through the arm, is shown never to change
a program's outcome: no instruction is read past the fetch of its slot.
`the_breakpoint_stress_mode_keeps_outcomes` (`tests/stress.rs`) holds a
program of a loop, blocks, a non-local return out of a `do:`, an
exception handler and a recursion to its outcome under both lowerings,
the JIT on and off, the mode on and off. The tests that pin what machine
code, the optimizing tier or a collection's reclamation does switch the
mode off, as they switch off the cache stress mode, since a patched body
is never optimized and the patches keep every body they patched.

### The step stress mode

The step stress mode (`NS_NEXT_STRESS_STEP`, debug builds, one of the
`stress` gate's fifteen modes; `Interpreter::set_stress_step` for a
test, `Interpreter::stress_steps` the count of its steps that stopped,
`Interpreter::stress_steps_ended` the count of those it ended itself)
stops the run at the safepoints its schedule picks -- the pause stress
mode's `StressSchedule`, paced by the traced size with the stack counted
once more for each step of a burst -- as a pause does,
and from each such stop takes a burst of sixteen steps
(`debugger::STEP_STRESS_BURST`): a step into, a step over and a step out
in turn, the shipped filter switched on before every sixth step and off
three steps later, so that each action steps with the filter on and
off. At every stop, the one that begins the burst and each a step makes,
the mode's reader (`debugger::StepReader`, a `CommandSource` of its own)
reads the stack, the variables of every frame and the first children of
every value as the pause stress mode's reader does, then sends the
burst's next step, or `Continue` once the burst is over. No debugger
need be attached: a step's stop, which `serve_debugger` makes for an
attached debugger, takes the mode's reader as its agent when none is,
and the mode stands aside while one is attached, so that it never steps
in the debugger's place and every debugger test runs under it as
without it. The stop refuses no step command -- a step with no frame to
step in goes on as a `Continue` -- so any refusal is the stop misreading
its own stack, and panics.

Stepping every instruction of a program to its end would make the
`stress` gate's run far too slow; a burst steps into a callee's first
statements, over its sends and out of it, through blocks, unwinds,
handlers and the shipped frames the filter passes, while the schedule
spreads the bursts over the whole run. Every stop of a burst reads the
whole stack, so a burst costs seventeen readings of it where the pause
stress mode's stop costs one: the schedule is fed the traced size plus
sixteen times the stack's words (`size_with_stack`, the pause stress
mode's with eight), which leaves a shallow program's bursts paced about
as the pause stress mode's stops and makes a deep stack's as much rarer
as they cost more. The schedule counts every safepoint, a
step in progress included, and a burst is due only once no step is in
progress: a step still in progress when the schedule has fallen due
four times more (`STEP_STRESS_GRACE`), and after 64 safepoints at least
(`STEP_STRESS_GRACE_SAFEPOINTS`), a floor that keeps the grace from
shrinking with a heap the collect and scavenge stress modes keep small,
and so the schedule's period with it -- a step out of the run's first
frame, which stops nowhere before its run ends, or out of a shipped
`to:do:` looping long, stopped in with the shipped filter off, or out of
the block such a loop runs, which leaves the whole loop (decision 17) -- is
ended there, as a debugger's pause ends a step, so that no step keeps
the mode from its next burst; one whose frame has just returned to its
caller, which stands at the step's return point it fetches next, is
left to stop there. A step the run's end leaves in progress goes on in
the interpreter's next host run, as a debugger's does (decision 17): the
run's start serves the mode too (`Interpreter::serve_before_run`).

Ending a step that way would also hide a step that can never stop
because a frame left without the step learning of it -- a pop no
unwinder report and no continuation's arm made, the defect decision 17's
reports exist to prevent -- so a debug build asserts, as the mode ends a
step, that the step knows where its frame is: a running activation of
the stack, a frame suspended into its object, a frame that just returned
to its caller standing at the return point, or a frame whose pop was
reported, `Holder::Left` -- a step waits so for the handler search's
report of an error the popped frame's continuation raised, while the
exception's machinery runs (decision 17) -- and that it knows where the
frame whose send a step out of a block leaves is, a running activation
of the stack unless it was renamed so (`stepping::frame_still_there`);
a running name no live activation answers to, a pop no report reached,
fails it. A step waiting `Left` keeps the frame that was on top when
the error was raised, and the check requires it still to run below the
top, the exception's machinery above it
(`stepping::still_waiting_for_a_handler`), so that a step waiting for a
report that never came fails it too. The check is made only at a forced
end: a short run, which ends before its grace is spent, ends its step
with it unchecked, so a missed report in a short program is caught less
often than in a long one. The steps it ends are counted apart from
those that stopped, and `the_step_stress_mode_keeps_outcomes`, which
runs under every mode of the `stress` gate, requires both many whole
bursts and the steps that stopped to be at least four times those
ended. Every golden and the corpus run under it, under both
lowerings, with the JIT and with `NS_NEXT_NO_JIT=1`, so that the
steps' one-shot breakpoints, their held request, the reports of a frame
left otherwise than by a return and the filters are shown never to
change a program's outcome, and reading the stack at every stop never to
read a register the leftover analysis calls dead;
`the_step_stress_mode_keeps_outcomes` (`tests/stress.rs`) holds a
program of 200 rounds of a recursion, blocks, a non-local return out of
a `do:`, an exception handler and a deeper recursion to its outcome
under both lowerings, the JIT on and off, the mode on and off, and
requires that its steps stopped, as above: its rounds are frames rather
than a loop's block, which a step out leaves for the whole loop, so that
most steps stop.

### Testing and gates

**Partial:** the tests of stages 1 to 5, the pause stress mode, the
breakpoint stress mode and the step stress mode exist; the model test of
the poll words' ordering, the debugger goldens, and stage 6's tests do
not.

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
  the send it waits on included; a frame's named variables; a block's view
  of its method's variables; an outer variable of a creator that returned
  without keeping its registers, not available; a block two deep reading
  its method's variable two captured frames out, while the method runs and
  after it returned; a returned creator's frame an image decoded, read
  through the closure that reads it by reference; a factory's frame named
  by its header's parameters; a superseded body its edit changed, shown by
  number and without a location, its block's frame still named for its
  method; a superseded body an install into its declaration left alone,
  and its block's, named, located in the current text and showing their
  named variables; an image without source, its frames named and their
  variables numbered; and a vector variable's current value in every frame
  of its chain.
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
  breakpoint again on the new body; a patched body keeping its code
  id across a full collection; and a line breakpoint at a slot the
  breakpoint stress mode patched stopping at every pass, the stress
  patch at none, the slot still the mode's once the breakpoint is
  removed (debug builds). With the JIT on, under both lowerings: a
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
  stopping not in the `signal` but, once the handler search reports the
  handler it found, at the handler block's first statement, and with the
  shipped filter off at the first statement of the kernel's `cull:` that
  runs the handler.
  Of a step out of a block a send took (decision 17): out of a
  `whileTrue:`'s body, in its first round and in its last, and out of
  its condition, stopping after the loop; out of an `ifTrue:`'s block
  stopping after the conditional; out of an `ifTrue:` nested in a loop's
  body leaving one level, then the loop; out of a `do:`'s block stopping
  after the `do:` with its answer; out of an `on:do:`'s protected block
  and out of its handler block, each stopping where the `on:do:`
  answered; a block handed on in a variable, computed by an expression
  and written as a parenthesized literal, each leaving the `do:`; an
  inlined form mid-statement and in a send's argument stopping at its
  join; out of the innermost of three activations of a recursive home
  leaving its own `do:`; out of an `ensure:`'s block, and of one a
  non-local return's unwind runs; a block kept and run by another send,
  one whose home returned, and one a sibling closure runs, stepping out
  to their callers; a `do:`'s block whose next run returns
  non-locally from the home stopping at the home's caller's return
  point; a block a proxy's `doesNotUnderstand:` runs, its send's first
  argument and its second, leaving the send; a block the body's last
  statement took standing, once left, at that statement; an inlined
  form's fallback send to a user class's `ifTrue:` left as the form, as
  a statement and mid-statement; an inlined form that is an
  initializer's last statement left past the factory, after the `new`
  in a run and in the next run's first statement after the root's
  instantiation; and a block a slot initializer runs left with the
  factory's send. Of the factory (decision 12): a step into an
  instantiation stopping in the factory at its header's pattern, its
  parameters shown; a step over and a step into there stopping at the
  superclass clause, at the first slot initializer of a class that
  writes no clause, and after the `new` for an initializer with no
  statement; a step out there stopping after the `new`; a body-less
  declaration's factory; and a step carried into an instantiation's run
  stopping in its factory, as does a pause asked before it; the stop and
  the step from it after a thousand warming instantiations; a line
  breakpoint on the initializer's first statement taking the step from
  the factory's stop, once; and, through the agent, a stripped image's
  factory stopped in as a frame without a line. Of the header stops
  (decision 12): a step into a superclass clause stopping at the
  superclass's header, its factory's parameters shown; a chain of three
  levels stepped into header by header and back, never into `Object`,
  with the shipped filter on and off, and a step over at the lowest
  header; a step over a clause and a step out of a header stop; a step
  over and out from a superclass's last statement taking no header stop;
  an explicit `Object new` clause; a shipped superclass's header, only
  with the filter off, and stepped in once a filter switched on at it
  filters it, the shipped one or a class filter; a breakpoint at a first
  statement at the entry after the header stop, by a continue and by a
  step; a factory whose level runs nothing stopping at its
  superclass's header; `Object` never stopped in from an empty class's
  factory with the filter off; each `<:` clause's level at its mixin's
  header; a clause whose argument sends; the warmed header stop; and,
  through the agent, a stripped image's superclass header stopped at as
  a frame without a line. The agent's own tests step in from a loop's
  line to its body and out to the line after the loop, and reach the
  factory's stop and the header stops as frames at the header pattern,
  their parameters shown
  (`crates/ns-cli/tests/debug_adapter.rs`).
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
  landing in the program's block at its end; a filter changed between
  two steps deciding afresh; and, under the suspend stress mode, a step
  over at a block's last statement whose `do:`'s sender is suspended
  going on from that suspended frame, held as `Holder::Suspended`, to
  the block's next activation, which a debug build's count of steps
  going on from a suspended frame pins
  (`Interpreter::steps_from_suspended_frames`). Of the unwinds a step follows: a step
  from a block whose non-local return unwinds fifty frames flooding only
  the home it stops in, and an exception's stop reporting a method
  breakpoint's pending exit beside the exception. Beside them, a breakpoint at an
  instruction that meets an unresolved future stopping once, its slot
  initialized once: only the optimized lowering puts the instruction
  that meets it first in its statement, since the reference lowering
  begins every slot initializer's statement with a `Move`. A step over a
  statement that raises stops in the handler block a handler below
  takes, the error signalled by a method it sends or raised by its own
  instruction (a `^` whose home has returned), and a step over a
  statement whose `on:do:` the stepping frame sends itself stops at its
  next statement; a step out of the `signal` the VM pushed for an error,
  which returns once the test hook resumed its default action, stops in
  the `ensure:` block the run's end runs. Each
  also passes under every stress mode, the suspend
  stress mode's suspensions included.
- **The exception breakpoints' tests** (`tests/debugger.rs`, each under
  both lowerings with the JIT on): an uncaught error stopping before its
  default action, the frames of the machinery hidden and the method that
  signalled it shown first, with the shipped filter on and off, and
  going on ending the run with its error; an error a shipped method
  signals showing the program's frame first, and with the shipped filter
  off the shipped method; a caught exception not stopping by default,
  and with the caught-exception breakpoint on stopping before its
  handler, which runs once the program goes on, the frame on top the
  handler block's at its first instruction; under the JIT stress mode,
  the search in machine code, an uncaught exception stopping at
  `defaultAction`'s first instruction and a caught one at the handler
  block's; an uncaught error the VM raises showing the raising frame
  first, with the shipped filter on and off; the `ensure:` blocks a
  termination runs stopping at no exception they raise; a class filter taking an
  exception whose class inherits from the class it names and no other;
  an uncaught `halt` stopping and going on resumed with nil, and with
  the `Halt` breakpoint off stopping as an uncaught exception and ending
  the run; an uncaught `MessageNotUnderstood` showing the message's
  sender first; an uncaught exception whose `defaultAction` pushes
  nothing stopping at the next poll; and the golden that catches,
  resumes and leaves uncaught a `Halt` running with a debugger attached
  and every exception breakpoint off as it runs without one.
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
- **Stage 5's tests** (`tests/debugger.rs`, each under both lowerings
  where a lowering can tell): `evaluation::*` -- a local read and written,
  a frame below the top and a stopped block's home read, a private method
  reached by an implicit receiver send and another object's refused, an
  error as the answer with the program's handlers below untouched, a
  program's block whose `^` would return to a home below answering
  `CannotReturn` (an evaluated `^` is refused by
  `compiler/evaluation.rs`'s own tests), breakpoints gone past, a compiled
  loop's iteration and a compiled frame below the top written, a
  collection during an evaluation, a stored block reading the frame after
  it returned, a shipped frame refused, a pause, `AbortEvaluation`, a
  terminate and a detach ending an endless evaluation, the depth reserve
  at a stack overflow and near the limit; setting a variable, the copied
  variable refused after its capture and assigned before it, through a
  fallback block and a vector, before its vector is made, and a read
  leaving the frame to drop its registers; `conditions::*` -- a condition
  counted before the hit condition and evaluated once a hit, a logpoint, a
  failing condition and log expression, a step's and a step into's stop, a
  method breakpoint's entry and exit, an exit by an exception and by a
  non-local return, an endless condition aborted, two conditions at one
  statement, beside an exception's stop and in a compiled loop;
  `restarting::*` -- a method, a block, a factory and an initializer
  restarted and forced to return, `ensure:` blocks run, the vector kept, a
  clause-less initializer and a body without metadata, an exception's
  machinery unwound, compiled frames below the top, a shipped frame
  refused; `smart_stepping::*` -- the targets in source order, an inlined
  conditional's and loop's blocks' sends, a fast-path accessor, a filtered
  send's block, a block never run, an escaped block of another activation,
  an exception's targets and a target in a loop; `marking::*` -- a mark
  held past a continue, named in a later evaluation and a condition, moved
  and removed, the platform's mirror field withheld; and `memory_view::*`
  -- instances counted and listed, referrers from objects and from the
  stack, a shipped instance, the mirror's class and a frame object
  withheld, weak words and an ephemeron's key no references, and returned
  frame objects. The agent's (`crates/ns-cli/tests/debug_adapter.rs`):
  `evaluate`, its error, a hover's bound and `cancel`, `setVariable`, a
  conditional breakpoint, a failing condition, a logpoint, a mark named
  later, a withheld value read-only, `stepInTargets` and `stepIn` with a
  target, a stale target refused, `restartFrame`, `nsNext/forceReturn`
  with and without an expression and during an evaluation, the memory
  view's requests, a shipped frame deemphasized, and a lost connection
  ending the program in the connecting mode. The MCP server's
  (`crates/ns-cli/tests/mcp.rs`): its tools, a start stopping at a
  breakpoint with its summary, a step and an evaluation, an endless loop
  paused, a shipped frame's file, a reference expanded and a mark, an
  endless evaluation aborted, a start without a runtime running the
  server's own executable, stale shipped sources removed, and a killed
  server ending its program; `xtask/src/mcp.rs`'s tests hold the copy
  `cargo xtask mcp` puts, that a copy already there stays and the
  refusal without a build. The plugin's tests cover the stage's actions
  over a scripted agent and the real runtime ("The JetBrains plugin").
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
- **The breakpoint stress mode** (above) patches every statement start
  of every body it finds running at a safepoint -- every body whose first
  entry is interpreted -- goes on at each, and holds every
  golden's outcome under both lowerings, with the JIT and with
  `NS_NEXT_NO_JIT=1`.
- **The step stress mode** (above) takes bursts of steps into, over and
  out, the shipped filter on and off, from the stops its schedule picks,
  reads the stack at every stop, and holds every golden's outcome under
  both lowerings, with the JIT and with `NS_NEXT_NO_JIT=1`.
- **The agent's tests** (`crates/ns-cli/tests/debug_adapter.rs`, through
  the test client `tests/common/dap.rs`, each starting `ns-next` as a
  process, and its unit tests in `crates/ns-cli/src/debug/`): a session
  run to its end; the listening mode's line, its token required, an
  `initialize` counting from 0 and a request before the `attach`
  refused, a peer that never attaches closed at the deadline and an
  attached one silent past it kept, a peer that never reads closed at
  the deadline, an attached client that stops reading dropped, and an
  unattached peer's long frame closing its connection; the connecting
  mode's `process` event returning its launcher's secret; a request after
  the program ended answered, and a client that never disconnects
  keeping the ended program's process no longer than its 2 s; a module
  named through `..` and a program started through a link taking a
  breakpoint by the plain and the real path; `stopOnEntry` stopping at
  `main:args:`; `terminate` running the `ensure:` blocks, also between
  runs, before `main:args:`; `disconnect` with `terminateDebuggee`
  killing a program inside a long leaf, and without it letting a stopped
  program finish in either mode; a second client of the listening mode
  meeting none of the first's breakpoints; a breakpoint's stop showing
  its frame and variables, a frame's line and column after non-ASCII
  text, a shipped frame through a source reference, a loaded image's
  modules through source references and a stripped image's frames
  without lines; breakpoints set before `configurationDone`, in a
  shipped module, while running, as a block variant, with a pass count
  and on a comment line; a stop naming a replaced breakpoint; steps by
  line and by statement, in and out, a line step over a one-line loop,
  one at a one-line block's statement and one at a one-line fallback
  block's stopping in its next activation, and one a breakpoint ends
  mid-line; a continue and a terminate at a
  factory's stop; run to cursor; a function
  breakpoint; an uncaught exception's description; `pause` of a running
  loop and right after a `continue`; a stack trace while running
  refused; and a pending breakpoint reported once its module is built.
  The unit tests hold the parsing of hit conditions, method names and
  class lists, a breakpoint's id kept while the client sends it, the
  unasked events told from answers, a stop naming only the breakpoints
  the agent knows and a pending replacement hiding the ids it replaces,
  a module's source and reference, a canonical and a spelled path of one
  file agreeing and a module whose file is gone still named by its path,
  a message's whole write bounded however the peer reads and the
  writer's `finish` ending within its deadline, the framing's bounds,
  and when a line step goes on.
- **The breakable statements' tests**:
  `crates/ns-cli/tests/debug_metadata.rs` holds `breakable_statements`
  to where `SetBreakpoints` lands for every golden of
  `goldens/language/` under each lowering, and the language
  server's tests (`crates/ns-analyzer/tests/protocol.rs`) its request:
  each statement's line and its block variants, an answer following the
  document's changes, `null` for a document that does not parse, an
  unknown document's error, and a chain past the cap answered `null`
  with the server serving on ([editor-support.md](editor-support.md),
  "Breakable lines").
- **The plugin's tests** debug programs with the real runtime through
  its DAP client (`DapSessionTest`), through the platform's debugger
  (`NewspeakDebugProcessTest`) and through its breakpoint types
  (`NewspeakBreakpointTypesTest`), each a Gradle task of its own
  ([editor-support.md](editor-support.md), "Plugin testing and gates").
- Planned: **debugger goldens**, a `.ns` with a script of commands and
  the stops, stacks and values expected, run by a scripted DAP client in
  `ns-cli`'s tests.
- **Stage 3's end-of-batch gates** passed on its final tree
  (`b3da5ce3`, 2026-11-13): `images`, `caches`, `benchmarks`,
  `release-checks`, `codegen`, `stress` -- the suite under the fifteen
  modes and with `NS_NEXT_NO_JIT=1`, 1,642 s, and `run-corpus` under
  each -- `linux` and `aarch64`, whose suite took 688 s.
- **Stage 4's end-of-batch gates** passed on its final tree
  (`d3786f5f`, 2026-11-14): `images`, `caches`, `benchmarks`,
  `release-checks`, `codegen`, `stress` -- the suite under the fifteen
  modes and with `NS_NEXT_NO_JIT=1`, 3,235 tests each (WIDE 5,009), and
  `run-corpus` "ran 6, failed 0, skipped 205" plain and under each mode
  -- `linux`, `aarch64`, whose suite took 385.7 s, and
  `plugin-verification`.
- **Stage 5's end-of-batch gates** passed on its final tree
  (`904fec56`, 2026-11-17): `images`, `caches`, `benchmarks`,
  `release-checks`, `codegen`, `stress` -- the suite under the fifteen
  modes and with `NS_NEXT_NO_JIT=1`, 3,719 tests each (WIDE 5,514), and
  `run-corpus` "ran 6, failed 0, skipped 205" plain and under each mode
  -- and `plugin-verification`, compatible with each of the six IDEs it
  checks. Their first run found two defects and three tests the pause
  stress mode's evaluations broke, fixed before the final tree: the
  memory view answered a frame the suspend stress mode had suspended as
  its frame object, and the pause stress mode began an evaluation while
  a breakpoint's token waited for its frame. The Docker groups `linux`
  and `aarch64` were not run.
- **The `codegen` gate** holds the run loop with the `Cell` slots and
  the `Breakpoint` arm to its budgets: the arm shares the out-of-line arm
  of the simultaneous slot clauses, since an arm of its own grew
  `vm::run` past its budget ("Stopping at and going past a breakpoint").
  What stage 3 costs a program without a debugger was measured against
  its base, `bench_signal` +0.3% at forty iterations and the geometric
  mean +0.36%, nothing separated ("What stage 3 moved"), and what an
  attached debugger with its defaults costs, against the same binary
  without one, +0.04% on the geometric mean, nothing separated ("What
  stage 4 moved"). The `linux` and `aarch64` gates run the atomics on
  both architectures.

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

### What stage 3 moved

Stage 3 makes every slot of a body a `Cell`, which the run loop's fetch
copies out as before; puts the `Breakpoint` arm into the run loop,
sharing the out-of-line arm of the simultaneous slot clauses; makes
`MixinMethod::fast_path` a `Cell`, read by every hit as before; adds a
leaf machine code calls without exiting to the kernel's handler search,
`exceptionUnhandled:ifFail:` before every `defaultAction`, and reports
a caught exception through the exit that reads the handler, which the
search made before, now `exceptionCaught:at:ifFail:`, so that a caught
exception costs no send it did not cost; and adds to the unwinder one
test per unwind, whether a step is in progress or a method breakpoint
set (`breakpoint::follows_unwinds`). Only then does it make the
per-frame reports (`leaving_by_unwind`, `returning_by_unwind` where it
arrives, and `pop_for_unwind`'s tests whether a step is in progress):
a version that made them for every frame it popped, and sent the report
of a caught exception beside the read of its handler, measured
`bench_signal` +2.3% slower, separated, at forty iterations, two thirds
of it those per-frame tests and a sixth that send, as two probe builds
showed.

Stage 3 was measured against its base on `master` (`e42f896a`) in one
sitting on 2026-11-13, a program without a debugger, by the protocol of
the `measuring-performance` skill: three pinned builds a side with the
zone paddings, built at once, three rotated rounds, CD, Havlak,
Mandelbrot, NBody and DeltaBlue at three iterations and every other
program and microbenchmark at eight, every process held to the eight
performance cores. The geometric mean over the twenty-three sources is
+0.36%; by build pair it is +0.13%, +2.09% and -1.12%, by round +0.63%,
+0.91% and -0.46%. The seconds-long programs are within 1.1%. None
separated at eight iterations; the sources outside 2% swing by build
pair in both directions -- List +1.8% (-24.4%, +17.6%, +18.8% by
build), Towers +4.7% (+20.5%, +27.2%, -25.0%), Queens +3.9%, Sieve
+3.1%, `bench_recursion` +4.4%, Permute -7.5%, `bench_send` -7.9% --
and at forty iterations, three rounds of the six binaries, Queens is
-9.7%, Sieve -5.4%, Towers +2.5% and `bench_recursion` -1.6%, none
separated: noise of sub-millisecond medians, as stage 1 found.

`bench_signal` is the one program that is mostly caught exceptions:
each of its 5,000 iterations signals an `Error` fifty frames deep,
which a handler catches, and `return:` unwinds the fifty frames. At
eight iterations it is +1.7% (+2.2%, +1.7%, +1.3% by build), not
separated. At forty iterations, six rotated rounds of the six binaries
beside the three of the per-frame version, it is +0.29% (+0.5%, +0.3%,
+0.1% by build; 19.48 against 19.42 ms), not separated, while the
per-frame version in the same rounds is +2.6% (+3.0%, +2.6%, +2.2%).
Three more rounds of the six binaries in the same sitting put it at
+1.7%, one run of build a 20.88 ms against no other above 19.97 ms,
not separated either; all twenty-seven runs a side pooled, +0.75%. The
`Cell` slots, the shared `Breakpoint` arm and the fast path's `Cell`
cost nothing measurable.

Not covered: a run with a debugger attached and no breakpoint, which no
command line attaches yet; Linux, AArch64 and Windows on ARM; start-up,
memory and `ns-next load`; and an unwind through frames running machine
code beyond what `bench_signal` exercises.

### What stage 4 moved

Stage 4 adds nothing to a program run without `--debug`: the hit counts
are read only in `interpreter/breakpoint.rs`'s cold decision, and no
instruction, golden, `run-corpus` outcome or image changed. What it
adds to a program run with a debugger attached is the agent's threads
and, at every exception the handler search reports, a clone of the
exception breakpoints' settings. So stage 4 was measured as an attached
debugger against none, on one binary, in one sitting on 2026-11-14, on
the tree `f45b4f0b`, by the protocol of the `measuring-performance`
skill: two builds, a with the pinned `RUSTFLAGS` and b with them plus
another code layout (`-C metadata`, every block aligned, the zone
paddings 208 and 32), each process held to the performance cores before
it ran. One build a layout rather than three a side, since both sides
run the same binary: a layout's noise cancels, but in the debugger's own
cold code. The Are We Fast Yet harness timed each program in process, a
warm-up of 1,000 ms and a measurement of 2,000 ms (CD, DeltaBlue,
Havlak, Mandelbrot and NBody 2,000 and 5,000), its result printed to
four decimals, plain `ns-next run` against `ns-next run --debug-connect`
with a client that initializes, attaches and sends `configurationDone`
and nothing else, three rounds alternating and rotating the order.

The attached run's time over the plain one's, as a geometric mean over
the sixteen programs, is 1.0001 in build a, 1.0008 in build b, 1.0004
pooled (+0.04%); every pooled program lies within 0.9956 (Storage) and
1.0065 (Json), while the plain runs alone moved a median 0.65%, at most
1.84%, between rounds: nothing separates from noise. No Are We Fast Yet
program signals an exception in a passing run, so the clone was timed
apart, 200,000 caught signals fifty frames deep timed in process: 0.9995
in build a and 1.0003 in build b, about 3.8 µs a signal either way. The
fixed cost is the connection and the handshake, 17 to 32 ms a process
over loopback (a program that prints a line: 5.3 ms plain, 29.7 ms
attached).

Not covered: a third build; the listening mode (`--debug`) and
`load --debug-connect`; any debugger state beyond the defaults --
breakpoints, filters, steps and pauses; uncaught exceptions,
`doesNotUnderstand:`, errors the VM raises, and resumed or retried
exceptions; `ns-next bench`, and of the microbenchmarks only
`bench_signal`'s signal; Linux, AArch64 and Windows on ARM; and the
agent as the batch ended, measured at `f45b4f0b` before its writes moved
to the connection's writer thread and were bounded, which adds a thread
and a queued message per answer to an attached run, nothing to a
program's own thread.

### What stage 5 moved

Nothing on the hot path of a program run without a debugger, so stage 5
was not measured, as stage 2 was not. No instruction, wire tag, golden,
`run-corpus` outcome or image changed; the lowerers compile an
evaluation only when the debugger asks for one, and the metadata's
`copied_by` lists only when the debugger compiles a body again. What
the stage adds to the VM's paths runs out of line: the run loop's arms
for an evaluation's end and a condition's answer at an exit by an
unwind (`Continuation::Evaluated`, `Continuation::ExitEvaluated`) and
the unwind a stop starts (`SafepointOutcome::Unwind`) are cold
functions beside `take_safepoint`, itself cold; the depth limit an
evaluation raises is read only on the cold path of a push past L
(`Mutator::ordinary_depth_limit`), the push's own check still comparing
with L; and the unwinder tests whether an unwind is the debugger's
restart or forced return (`arrives_at_any_frame`) only after
`catches_return` answered false, at a target that is no method, and
whether it stands at an evaluation's base only in the copy that follows
unwinds for the debugger. The `codegen` gate held `vm::run`,
`resume_caller`, `finish` and the push and return paths to their
budgets unchanged at the end of the batch ("Testing and gates").

With a debugger attached, what a program pays is what it asks for: a
breakpoint with a condition or a log leaves every reach to the safepoint
and runs an evaluation there, which a reach without them never does
(conditions and logpoints in hot code, "Hit counts and temporary
breakpoints", are unmeasured), and the memory view walks the whole heap
at each request.

## The decisions

Decisions 1 to 9 were decided with the user on 2026-11-10, decisions 10
to 15, stage 2's, on 2026-11-12, and decisions 16 and 17, stage 3's, on
2026-11-12, one at a time, each after the
specification, the prior art (`prior-art/jdk`, `v8`, `opensmalltalk-vm`,
`primordialsoup`, `newspeak`, `somns`) and the repository's invariants.
Each states the choice and the reasons the alternatives were rejected; the
mechanisms each implies are in "Planned: the mechanisms". Decisions 2,
3, 5, 7 to 17 are built; of decision 1 the debugging API and its first
client, the DAP agent ("The debugging API at the safepoint"), without
`ActivationMirror`; of decision 4 the tables, the stops at statement
starts and return points, smart step into's targets and the agent's
stepping by line; and of decision 6 the session over DAP with every
request stages 1 to 5 need, without `breakpointLocations` and the
requests of ns-next's own for hot swap and edit-and-continue. Everything
else they decide is planned, and is written below as the design to
build.

### 1. One debugging API in Rust, two clients over it

**Partial:** the API's first part exists in
`crates/ns-runtime/src/debugger/` -- attaching, pausing, the commands
served while running, the stop, reading the stack and its values,
continuing and terminating ("The debugging API at the safepoint"), the
breakpoints of every kind and the steps with their filters, evaluation,
writing, restarting and forcing a frame to return, marks and the memory
view -- and so does its first client, the DAP agent in `ns-cli`
(`crates/ns-cli/src/debug/`; "Launching, the start and the end"),
beside the tests and the pause, breakpoint and step stress modes;
`ActivationMirror` is planned.

Every mechanism -- stopping and going on, the breakpoint instruction, the
stepping state, reading and writing frames, the metadata,
deoptimization -- sits behind one debugging API of `ns-runtime`. It has
two clients: the remote agent in `ns-cli`, which speaks the protocol to
an IDE and is built, and `ActivationMirror` in `MirrorsFactory.ns`,
through new mirror leaves, which gives a program the access to
activations §8.2 asks of mirrors and is planned. Evaluation runs
Newspeak code through the existing resolver and compiler
("Evaluation"), and edit-and-continue will run it through the install
too.

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

**Built** (`debugger/stepping.rs`; "Step over in recursion and
blocks", "Step into's held request", "The return value of the last
call", "Smart step into a block"): step over, step into, step out and
smart step into, the frame left by a return to its caller's return
point, and the reports of a frame left otherwise, by an unwind or a
continuation other than `Deliver` (decision 17), the handler search's
report ("Exceptions"), the step filters (decision 16), and run to
cursor. The agent speaks smart step into as DAP's `stepInTargets` and
`stepIn`'s `targetId` ("Launching, the start and the end").

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
  reports of a frame left otherwise; taken in a block a send took, it
  leaves that send instead -- an inlined form's for its join
  mid-statement, or else the next statement start after it, a real
  send's for its return point (decision 17).
- **Step into** is a step over plus a safepoint request held until the next
  push, which the debugger's safepoint stops at, at the new body's first
  statement start. A send answered by a primitive or a leaf pushes
  nothing, and the next one-shot breakpoint stops.
- **Smart step into** (`Command::StepInTargets`,
  `Command::StepIntoTarget`; `stepping::targets`,
  `stepping::begin_smart`) offers the top frame's sends from where it
  stands to its statement's end, in the order they run, then the block
  literals those sends take that run as closures. A frame below the top,
  at an exception's stop, waits on the send at its program counter, which
  runs already: its list begins past it. An inlinable form -- `ifTrue:`,
  `and:`, `whileTrue:`, a mirror send's failure block, with literal
  blocks -- offers the same under both lowerings: its own selector is
  never a target (the reference lowering's `SendInlinable` and its
  loop's `value` sends, the optimized lowering's fallback send and
  inlined mirror send; `stepping::forms_own_sends`), nor are its blocks,
  and the sends its blocks hold are, once each, where the form stands,
  in the order the source writes them: `[x foo] whileTrue: [x bar]`
  offers `foo` and `bar`. Under the optimized lowering they are the
  inline copy's, in the frame's own body; under the reference lowering
  they are the block body's, whose closure the frame runs and the step
  floods for it, so that the chosen send's sender is the step's inline
  code. The step stops at no statement start of the statement on the
  way -- of an inlined block of it, its fallback closure or the
  reference lowering's block (`stepping::StatementRegion`) -- so that it
  stops at the chosen push or, as a step over, once the statement is
  done; its own first statement start stops it, since a frame that
  comes back there has done the statement -- the next iteration of a
  loop the optimized lowering inlines, whose condition may have no
  statement start, as `[going] whileTrue: [...]`'s has none, reaches
  the statement again where the reference lowering's next condition
  block stops
  (`a_smart_step_in_a_loop_reading_a_local_stops_at_the_next_iteration`).
  Aimed at a send it is
  a step into whose safepoint stops only at a push whose sender -- or
  the first sender below a run of filtered frames, so that a send of
  the shipped `do:` stops in the program's block it runs -- stands at
  the chosen send (`stepping::stands_at_aim`): the body and the index,
  which within one body names the send as its `CallSite` does. Any other
  push keeps the request, which is taken at every poll as a step into's
  is until the chosen push or a one-shot stops; a send answered without
  a push leaves the step a step over. Aimed at a block, it is a step
  over plus a one-shot ("Smart step into a block"). A target is named
  by its position in the list, valid until the program goes on; the
  stop keeps the list, so that an evaluation that installs between the
  two commands changes no id.
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
line breakpoints and steps stop at their statement starts and return
points, and the agent steps by line (`crates/ns-cli/src/debug/stepping.rs`),
and smart step into's targets read the sends and the blocks they take
(`stepping::targets`).

The metadata of a body lists:

- every **statement start** -- the body's own, every block's and every
  inlined block's, and every initializer, slot and prologue entry
  `statement_program_counter` gives a range -- with its source span. A
  line breakpoint, a step over and a step into stop only there. A
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
- every **send that takes a block literal** as its receiver or an
  argument, the compiler's inlined forms among them: where a frame of the
  body stands while each of its blocks runs -- the instructions an inline
  block lowered to, or the send that runs it as a closure and the
  `MakeClosure` that made it -- whether the form is one the optimized
  lowering inlines, and the index after the whole send: what a step out
  taken in one of its inline blocks leaves, and whether a send a closure
  was taken by is an inlined form (decision 17).

The agent steps by line for IntelliJ, over statement starts until the line
or the activation changes, as the JDWP agent steps by its line table
(`stepControl_handleStep`,
`jdk/src/jdk.jdwp.agent/share/native/libjdwp/stepControl.c:523-697`, the
line comparison at `:640-669`, `findLineNumber` at `:130`): a `next` or
`stepIn` by line -- DAP's default `granularity` -- sends the VM's
`StepOver` or `StepInto` and, while the stop it makes is a step's in the
activation the step began in -- the same stack depth and frame name, the
VM's step never having gone on from another frame -- on the line it began
on, sends it again, reporting only the stop that leaves the line or the
activation; a stop for any other reason -- a breakpoint, an exception, a
pause -- ends the step at once and is reported as it is
(`next_by_line_steps_over_a_whole_line`). A frame without a location ends
it too, since no line can be compared. Depth and name cannot tell a
block's next activation from the one the step left: a `next` at the
statement of a one-line block a `do:` runs stands, after the VM's step
over, in the next activation at the same depth, in a frame of the same
name, on the same line, where decision 16 stops it. So the step's stop
says whether the VM's step went on as a step into from where the program
went on rather than from the frame it was taken in -- after that frame, or
its caller, returned or unwound, past a filtered frame, at a handler, or
into the next run (`Event::Stopped`'s `elsewhere`,
`stepping::continue_as_step_into`) -- or whether the activation it was
taken in left while the step stepped in another frame: a step taken in the
top frame itself -- not at an exception's stop, which steps from the frame
it shows below the machinery, nor for a step out that leaves the send that
took a block -- that is the fallback closure of a block its creator's
inlined form took steps in the creator, and a receiver that runs the block
again makes a new activation at the same offset, at the same depth, in a
frame of the same name, which only the first's return tells apart, noted
by a one-shot that never stops where its caller goes on
(`OneShot::leaves`, `stepping::note_origin`) and by `popped` for an
unwind. A stop that went on elsewhere is another place, whatever its
depth, name and line
(`a_line_step_over_at_a_one_line_block_stops_in_its_next_activation`,
`a_line_step_over_at_a_fallback_block_stops_in_its_next_activation`,
`a_step_s_stop_says_whether_it_went_on_from_another_frame`,
`a_step_over_into_a_fallback_block_s_next_activation_says_it_went_on_elsewhere`).
The agent places a stop by the first frame it shows, below an exception's
hidden machinery, the frame the VM steps from
(`a_line_step_from_a_halt_stop_leaves_the_line`). Statements on one line
of one activation are still stepped over as one line, and so is a one-line
loop whose block a step over passes, since the step goes on in the frame
it was taken in (`a_line_step_over_a_one_line_loop_ends`,
`a_step_over_a_loop_says_it_stayed_in_its_frame`). Each repetition reads
the stop's frames and the top frame's module's text from the VM, the text
by revision, so that a step over a line of several statements costs a
round trip per statement and never a stop the client sees. A client may
ask for `granularity: statement`, one VM step, and `instruction` steps as
`statement` does, a statement being the smallest place a stop is made at.
A source span per instruction, Primordial Soup's `bciSourceMapping`
(`primordialsoup/newspeak/Newspeak2PrimordialSoupCompilation.ns:564-596`),
is rejected: most of ns-next's instructions are register moves with no
place of their own in the source.

### 5. The metadata is compiled again on demand and verified

Built ("Compiling the metadata again", and a breakpoint's resolution, "A
breakpoint names a source position"). Neither table is stored. The first
time the debugger needs a body's -- a stop in it, or a breakpoint the IDE
sets -- it compiles the body's source again, the lowerer collecting the
stop locations and the **variable table** on the side: each named
variable's place (a register, a register of an enclosing frame, a slot of
an indirection vector), over the whole body, since an inlined block
declares no variable of its own (decision 13). The result is compared with
the running body; only an equal body gets the tables, cached by the body.
A body whose source is gone (an image built with `--strip-source`) or
whose recompilation differs (code an older binary compiled, a superseded
version an edit changed) gets none, and its frames show without variable
names and source positions, as Java's do for a class compiled without
`-g`; a frame keeps its name, which its declaration gives.

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

**Partial:** the agent speaks the session, a stop's `stackTrace`,
`scopes`, paged `variables` and `source`, `setBreakpoints` with hit
conditions and block variants, `setFunctionBreakpoints`,
`setExceptionBreakpoints`, `pause`, `continue`, the steps with their
`granularity`, run to cursor, `evaluate`, `setVariable`, breakpoints'
conditions and logpoints, the marks (`nsNext/mark`, `nsNext/unmark`),
smart step into (`stepInTargets`, `stepIn`'s `targetId`),
`restartFrame`, force return (`nsNext/forceReturn`), the memory view
(`nsNext/instanceCounts`, `nsNext/instances`, `nsNext/referrers`),
a stopped frame's code (`nsNext/frameCode`), the shipped frames' hints,
`terminate` and `disconnect`, and the `stopped`, `breakpoint`, `output`
and `exited` events (`crates/ns-cli/src/debug/`, "Launching, the start
and the end"); `breakpointLocations` and the requests of ns-next's own
for hot swap and edit-and-continue are planned.

The agent speaks the Debug Adapter Protocol, JSON in `Content-Length`
frames, over a TCP socket bound to 127.0.0.1, since the program owns its
standard streams; `ns-next run --debug …` listens on it and
`--debug-connect` connects it to a client that listens, as
`-agentlib:jdwp` does for a JVM ("Launching, the start and the end"). The
framing is written by hand on `ns-cli`'s `serde_json`; the analyzer's
server dependencies stay out of `ns-next`, and `ns-runtime` sees only the
debugging API's types.

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

**Built:** an expression is resolved and compiled as a block in a
stopped frame's scope (`resolver::resolve_evaluation`,
`compiler/evaluation.rs`) and run at a stop (`Command::Evaluate`,
`debugger/evaluation.rs`; "Evaluation"), as a variable's new value
(`Command::SetVariable`; "Writing a frame") and as a breakpoint's
condition and a logpoint's message ("Hit counts and temporary
breakpoints"), and the agent serves `evaluate` in every context, a
watch's among them, and the plugin evaluates through it in the Evaluate
dialog, its watches and its hovers ("The JetBrains plugin").

An expression -- `evaluate`, a breakpoint's condition, a log message, a
watch, `setVariable` as `name:: expression` -- is resolved and compiled as
if it were a block standing at the frame's stored `program_counter` -- a
statement start, or a return point mid-statement -- its outer frame the
stopped frame, reified for it. It reads and writes the frame's
variables by reference, through the instructions a closure reads its
creator's registers with (`LoadOuterLocal`, `StoreOuterLocal`, counted in
`outer_reach`), and finds each variable by the variable table: an
assignment reaches the frame, a parameter or immutable local is refused by
the resolver as anywhere else, an assignment to a mutable local a block
may already have copied is refused ("Assigning a captured variable"
below), and a
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

**Built** (`debugger/mod.rs`): the stop is made for a pause, a
breakpoint of every kind and a step ("Serving at the safepoint", "The
stop", "Stopping at and going past a breakpoint"), and serves the stack,
values, breakpoints and evaluations ("Evaluation").

A breakpoint, a step's stop or a pause leaves its stretch for the run
loop's safepoint, as a collection request does, and there calls the
debugging API, which blocks on a channel and serves the agent's commands
-- reading the stack and values, setting breakpoints, evaluating --
answering on another; no handle is live there, and nothing pushes or pops
while it waits. Evaluating pushes the expression's activation and returns
into the loop, its return carried back into the stop by a `Continuation`
variant of its own (`Continuation::Evaluated`): the trampoline every send
uses, never a re-entry of the loop from Rust
([execution.md](execution.md), "No re-entry from Rust"). The debugger
therefore needs no yield path out of `vm::run`; that remains the image's
need, for a snapshot taken mid-execution
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
statement start. A step into at the instantiation stops in it, at its
entry, as in any body it pushes, though it has none: its place is the
pattern, its variables the factory's parameters as the sender passed
them (decided with the user on 2026-11-15, the Java constructor
pattern). A factory a frame sends runs its first two instructions as
part of the send, the instance allocated and the initializer pushed,
with no safepoint between (`Mutator::enter_factory_body`), so the held
request meets the initializer's push first: the debugger undoes that
push -- the initializer has run nothing, and the factory's
`InitializeInstance` decides and pushes it again once the program goes
on -- and stops in the factory, at its `InitializeInstance`, the run
loop reading the frame on top again (`stepping::rewound_into_factory`,
`HeldStep::StopInFactory`, `Served::Rebuilt`). Whatever goes on from
that stop -- a step, a `Continue` with no step in progress, a
`Terminate` -- goes on from the frame read again: a `Continue` runs the
initializer pushed again once, a breakpoint at its first statement hit
once and its hit count counted once, and a `Terminate` ends the run
there
(`a_continue_at_the_rewound_factory_stop_runs_the_initializer_once`,
`a_hit_count_at_the_initializer_s_first_statement_counts_the_rewind_once`,
`a_terminate_at_the_rewound_factory_stop_ends_the_run`; through the agent
`a_continue_at_the_factory_stop_runs_the_program_to_its_end` and
`a_terminate_at_the_factory_stop_ends_the_program`); a factory the host sends,
the first activation of an instantiation's run, which a step the run
before left goes on from (decision 17) or a pause asked before the run
stops, stops at its first instruction through a one-shot
(`stepping::stop_at_factory_entry`). Neither puts a
test on the instantiation's path: the one is decided at the held step's
safepoint, the other by a patch. A step over or into at that stop goes
on, as a step into, to the initializer the factory pushes, and stops at
its first statement -- the superclass clause when one is written, else
the first `<:` clause's check or slot initializer: the clause that
stands for `Object new` where a declaration writes none (§6.1), which
the metadata places at the class's name, is none a step stops at
(`stepping::first_stop`), and an initializer with no other statement
lets the step stop after the `new`, where the factory returns; a step
out stops after the `new` at once. On the way out the factory is passed
over as a frame the step filters filter is (decision 16), whatever the
filters say: a stop the step would make in it past its entry -- the
return point an initializer's return, or a step out of one, reaches --
goes on as a step into from the factory itself
(`stepping::went_on_past_filtered`), which stops at the factory's
return point in its caller. The factory is known by its frame's kind
(`stack::is_factory`, the stack trace's `FrameKind::Factory`), with or
without metadata: a factory of an image without source is stopped in
as a frame without a line, and the next step goes on
(`a_step_into_stops_in_a_stripped_image_s_factory`).

Every other level has a stop of its own at its header, the same
pattern's place one level up (decided with the user on 2026-11-15): a
step into whose push is an initializer entered for another level than
its sender's own -- by a superclass clause's `InitializeSuperclass`, by a
`<:` clause's message's `InitializeAppliedLevel`, or by a factory whose
own levels run nothing, which pushes the first initializer above them
that does (`instantiate::enter_or_pass`) -- stops in it at its entry,
before its first statement, placed at its declaration's factory pattern
or name (`stepping::enters_at_header`, `HeldStep::StopAtHeader`), its
variables the factory's parameters as the clause passed them. An
initializer its own class's factory pushes has none: the factory's stop
is that level's. A clause whose arguments send enters what they send
first, as any statement does; only the clause's own initialization of
the level takes the header stop. A step over or into at a header stop
goes on to that initializer's first statement, as the push would have
stopped it (`stepping::begin_at_header`, `first_stop`), the frame not
let past its entry, since its first statement may start there, and a
step out goes back to the sender after its clause; a step over at the
clause runs the whole chain above it. The stop is told from a stop at
the same index -- a breakpoint at a first statement that starts at
index 0 -- by the stop itself, which places its frame at the header
(`Standing::at_header` in `debugger/mod.rs`, the metadata's
`header_pattern`), not by the frame, and it lets nothing past, so a
breakpoint at the slot the frame fetches next is the next stop's: a
`Continue` from the header stop stops there once, and a step over or
into from it makes one stop there, reported as the breakpoint's, as the
step from a factory's stop to a breakpoint at the initializer's first
statement is
(`a_breakpoint_at_a_first_statement_at_the_entry_follows_the_header_stop`).
The step from a header stop is begun in the initializer, though it
steps from its sender: a filter switched on at the stop that filters the
initializer -- a shipped superclass's with the shipped filter off, a
class filter naming it -- leaves the step stepping in it, as in any
frame a step is begun in (decision 16; `Stepping::begun_in`,
`a_step_from_a_header_stop_steps_in_its_frame_once_it_is_filtered`). The
step names the initializer so only while the one-shot at its first
statement waits, which it reaches before it can return: a return to its
caller's send is reported to no step, so a name kept past it would name
the next activation at its offset. An initializer that runs nothing a
step stops at leaves the step to its sender, begun in no filtered frame
(`a_step_from_an_empty_header_stop_never_stops_in_a_filtered_call`). An
initializer without metadata, an image's without source, is stopped at
its header as a frame without a line, and a step goes on from it as
from any push without metadata
(`a_step_into_stops_at_a_stripped_image_s_superclass_header`).
`Object`'s initializer and `Top`'s never stop, whatever the step
filters say, decided by the level they initialize, not by its being
shipped: they keep the held request as a filtered frame does, and a
stop in them goes on as one in a filtered frame
(`stepping::passed_by_every_step`), so the header stops of a chain end
at the last class that inherits from `Object` directly and an explicit
`Object new` clause steps to the next statement. The rule is a guard no
program reaches today: `Object`'s and `Top`'s levels need no
initializer code and take no arguments, so the instantiation passes
them without pushing their initializers (`instantiate::enter_or_pass`),
and no step meets one; it keeps the decision should either level come
to run code. A shipped superclass
other than `Object` is filtered as any shipped code: its header stop
comes only with the shipped filter off. Nothing of it is on the
instantiation's path: the decision is the held step's, at the safepoint
the push takes while a step into holds its request.

So every initializer's statements are stepped, in the order the program
runs them -- a `<:` clause's check, the superclass clause, the
superclass's header and initializer, each `<:` clause's message and its
level's header and initializer, the slot initializers and the body's
statements -- a step over or out at a level's last statement stops at
the next level's first after the clause that entered it, with no header
stop on the way down, and from the last level's the step stops at the
factory's return point in its caller, after the `new`, or, for a
factory the host sent, at the next host run's first statement (decision
17;
`a_step_into_an_instantiation_stops_in_the_factory`,
`a_step_from_the_factory_stops_at_the_superclass_clause`,
`a_step_from_the_factory_without_a_clause_stops_at_the_first_slot`,
`a_step_from_the_factory_of_an_empty_initializer_stops_after_the_new`,
`a_step_out_of_the_factory_stops_after_the_new`,
`a_step_from_a_body_less_declaration_s_factory_stops_in_its_initializer`,
`a_step_carried_into_an_instantiation_stops_in_its_factory`,
`a_step_over_goes_through_an_initializer_and_past_the_factory`,
`a_step_over_from_a_superclass_s_initializer_stops_in_the_next_level_s`,
`a_step_out_of_an_initializer_stops_in_the_next_level_s_then_after_the_new`,
`a_step_into_goes_through_a_mixin_application_s_initializers_in_order`,
`a_step_into_enters_a_method_a_superclass_clause_sends`,
`a_step_into_a_superclass_clause_stops_at_the_superclass_s_header`,
`a_step_into_goes_through_every_header_of_a_superclass_chain`,
`a_step_over_at_the_lowest_header_stops_at_its_first_statement`,
`a_step_over_a_clause_and_out_of_a_header_stop_in_the_subclass`,
`a_step_from_a_superclass_s_last_statement_takes_no_header_stop`,
`a_step_into_an_explicit_object_clause_stops_at_the_next_statement`,
`a_shipped_superclass_s_header_stop_comes_with_the_filter_off`,
`a_step_from_a_factory_into_a_superclass_s_initializer_stops_at_its_header`,
`a_step_from_an_empty_factory_never_stops_in_object`). A factory's body is
equal by construction, `factory_body` of the same arity. Primordial
Soup's `computeDebugInfo:` takes a factory's from the class header the
same way (`primordialsoup/newspeak/Mirrors.ns:2756-2761`). Showing it without
names or position is rejected, since it would be the one frame of an
ordinary program without a place; hiding it contradicts the
specification's "accurate and complete stack traces" (§3.6,
`newspeak-spec.pdf` p. 17) and would need rules of its own for step out,
restart and force return. Passing over the factory at its push is
rejected with the user: the factory is where the
sender's arguments become the class's parameters, which the stop shows.
Making `enter_factory_body` leave the factory to the run loop while a
step holds its request, so that its push meets the safepoint, is
rejected: it puts a test of the request word on every instantiation, a
branch a program run without a debugger takes.

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
  variables; a block's outer chain, copied, in a vector, by reference, two
  frames out, after its creator returned, and from a creator's frame an
  image decoded; a factory's frame; a superseded version an install into
  its declaration left alone, and one the edit changed; and an image
  without its source. Unit tests in `debugger/metadata/tests.rs`: a body
  after an edit of another declaration
  (`an_install_moves_the_spans_of_an_unedited_body`) and an image whose
  code the reference lowering compiled
  (`a_reference_lowered_image_body_pairs_with_the_reference_lowering`).
- An oracle over every body: a test compiles every body of every golden
  again under the lowering it runs under, both of them, and requires it
  equal, and holds the tables to their invariants -- every statement start
  within the body and at the start of every `statement_program_counter`
  range, every send entry naming a send of exactly its `CallSite`, every
  block send going on within the body, its inline blocks before that and
  holding their statement starts, and its called ones run by a send of the
  body and made by a `MakeClosure` before it whose body's source is the
  block's span, every variable's register below `local_count` and its
  range within the body, every span within the module's text; the shipped
  modules' bodies are checked once by a test of their own, not per golden;
  the `images` gate runs it through `build` and `load`, which checks the
  trial of the optimized lowering and then the reference lowering.
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
`continue_as_step_into`, `held_step`; `Command::SetStepFilters`). A frame
is filtered when its body is a member of a declaration, or a block of one,
whose module's root is shipped while the shipped filter is on, or whose
path from its module's root begins with that of a `ClassName` the client
sent (`again::declaration_path`); the declaration is the one the frame's
metadata is looked up by (`stack::origin`), a class side's its instance
side's, and a frame of no declaration, the top level, is never filtered.
The decision is the body's, since the declaration is the version holding
the frame's body, and is kept by body (`stepping::FilterDecisions`, held
weakly as the metadata cache holds its bodies), so that a step into
through shipped code, which asks at every poll, builds no declaration path
after the first; it is forgotten when the filters change and at every
install, as the metadata is. A stop the step's own one-shot makes, where
no line breakpoint stands, is decided at the safepoint (`serve_debugger`):
in a filtered frame the step was not begun in, nor in a block lowered
inline in that one, it is not made, and the step goes on as a step into
from the first unfiltered frame below (`went_on_past_filtered`), the frame
past its slot once; every other way a step goes on as a step into -- a
frame an unwind or a continuation other than `Deliver` left (decision 17)
-- likewise starts from the first unfiltered frame at or below the one the
program goes on in, through suspended senders. A step into's held request
counts as its push an unfiltered frame whose sender is the stepping frame,
a block the step flooded for it, or a filtered frame whose sender is one
in turn, and so on down; a filtered frame on top keeps the request. The
filters live in the breakpoint table and go back to their default with its
other contents at an attachment and a detach.

A step never lands in a filtered frame: by default the shipped modules'
bodies -- a body is shipped when its module's root is, as for its
metadata ("Compiling the metadata again") -- and, beside them, the
bodies of the class declarations a list the client sends names,
IntelliJ's "Do not step into the classes"; the filter of the
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
in it as in any other. Whether it did is decided as the step begins: a
`SetStepFilters` served while the step runs changes where it lands, not
that. An uncaught exception's and a `Halt`'s stop likewise shows first
the first unfiltered frame below the exception's machinery
("Exceptions"), and the rule of a step that starts in a filtered frame
does not reach the machinery frames such a stop hides above it, which
the debugger chose, not the user: a step taken at that stop starts from
the frame shown first, as if it had stopped there, the hidden frames
ones it was taken above, so that neither they nor what they push are
the step's. A caught exception's stop stands in the handler block about
to run, which it shows first on top, and a step there is taken in the
handler block as in any frame it stood in; one whose handler pushed no
frame stands in the machinery, and is shown and stepped from below it
as an uncaught exception's is ("Exceptions").

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

**Built** for every way a frame leaves: the unwinder's pops and the arms of
`Continuation::Unwind`, `NestedClass` (a step pushed in the clause's
place, or an error), `VmErrorSignalled`, `Reexecute` and
`ReturnToSuspended` report to the stepping state
(`Mutator::pop_for_unwind`, `interpreter::breakpoint::frame_left`,
`stepping::frame_left`; an unwind's stop once, `stepping::unwind_stopped`),
and so does the handler search, once it found
a handler (`stepping::handler_found`, "Exceptions"); the step goes on
past the shipped frames -- `ensure:`'s, `on:do:`'s, `resolve`'s, the
exception's machinery -- by decision 16's filter. The unwinder's pops
and arrivals also report the exit of a method a method breakpoint names,
which stops by the same kind of step ("Method breakpoints").

A step out, and a step over at a body's last statement, stops at the
stepping frame's return point when its caller goes on there --
`Continuation::Deliver`, and a superclass clause's
`Continuation::NestedClass` whose class `after_clause` hands to the
accessor's send. Every other way a frame returns goes on as a step into
(with decision 16's filter) until the next statement start of the program,
the frame the program goes on in flooded with one-shot breakpoints: an
`ensure:` or `ifCurtailed:` block the unwinder ran
(`Continuation::Unwind`) stops in the next such block or in the frame the
unwind lands in; a future's `resolve` (`Continuation::Reexecute`) stops in
its caller after the instruction run again, or in what that instruction
pushes; a caller standing at an instruction that raised stops in the
handler block that takes the error, which the search reports to the
stepping state ("Exceptions"), or the program ends with the error
(`Continuation::VmErrorSignalled`); and a frame that returns to a
suspended sender (`Continuation::ReturnToSuspended`) stops in the sender,
resumed. The pop is reported to the stepping state only by the unwinder as
it pops (`Mutator::pop_for_unwind`) and in `apply_continuation`'s arms
other than `Deliver`'s, which call one cold function
(`interpreter::breakpoint::frame_left`), so the return's hot path is
unchanged: `finish` keeps no handle of the popped frame for them. The
unwinder asks once per unwind whether a step is in progress or a method
breakpoint set (`breakpoint::follows_unwinds`), and only then reports each
frame it pops: no pop can start a step that neither already holds; made
for every frame, those tests cost `bench_signal`, which unwinds fifty
frames per caught exception, about 1.5%, two thirds of the +2.3% that
version measured ("What stage 3 moved"). A pop only forgets the names the
step knew the popped frame by (`stepping::popped`); where the step goes on
is decided once, where the unwind stops popping -- it arrives at its
target, runs an `ensure:` or `ifCurtailed:` block, or leaves the run -- by
one cold call (`breakpoint::unwind_stopped`, `stepping::unwind_stopped`),
so that only the frame the program goes on in is flooded, never one the
unwind passes, which a deep recursion's unwind would otherwise patch, its
machine code freed, at every frame. The report finds the popped frame as
the one whose offset lies above the stack's top
(`Heap::activation_lies_above_the_top`), since nothing is pushed between
the pop and the report, and every name the step knew it by becomes
`Holder::Left`, which no activation is (`stepping::popped`) -- for a frame
returning to a suspended sender before the sender is resumed, which may
take its offset, all a release build compares
(`breakpoint::returning_to_suspended`). When the step's frame, or the
caller its return point is in, has left, the step goes on as a step into
from the frame on top (`continue_as_step_into`); any other frame the step
named above it is only forgotten. An error the popped frame's continuation
raises -- a superclass clause's answer refused as no class -- makes no
frame the step goes on in yet: stepping into the `signal` pushed for it
would stop in the exception's machinery, so the step waits, its frame
`Holder::Left`, for the handler search's report of the handler it found,
which goes on into the handler block, or for the unwind its default action
starts, which goes on where it stops popping. A step out of a superclass
clause whose class is handed to the accessor's send, the `NestedClass`
arm's value, reports nothing and stops at the accessor's return point.

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

**Built** for a step whose frame leaves its host run: the frame returns to
the host -- the run's first activation -- or an unwind pops it there, and
the run ends with the step not stopped. `ns-next run` is several host
runs of one program -- a project's manifest instantiated, the root
module, slot initializers included, then `main:args:` -- so the next
place the program reaches is the next run's first statement: a step
over or out at the last statement of the root's initializer returns to
the factory, which is passed over as a step into (decision 12) and
returns to the host, and the step stops at `main:args:`'s first
statement, one step. The run's end carries the step
(`stepping::run_ended`), and the next run's start goes on with it as a
step into from its first activation (`stepping::carry_into`,
`Interpreter::serve_before_run`), the way a pause pending between runs
stops that activation ("Commands while the program runs"), but a step's
stop, with decision 16's filter; step over, into and out alike, between
every pair of runs. A `Terminate`, a detach, a pause asked between the
runs -- whose stop reports the step's method exits -- and the program's
end, which starts no next run, end it; its value
of the last call is not carried, since it names a call of the run that
ended. The carried state is read only at a run's start and end, so the
run loop is unchanged
(`a_step_that_leaves_its_run_stops_at_the_next_run_s_first_statement`,
`a_terminate_between_runs_drops_the_step_its_run_left`, and
`next_from_a_slot_initializer_reaches_main` through the DAP agent).

**Built** for a step out taken in a block a send took as its receiver or
an argument -- one the compiler lowered inline, `whileTrue:`'s,
`ifTrue:`'s, `and:`'s, a mirror send's failure block, and a real closure
alike, `do:`'s, `collect:`'s, `on:do:`'s protected and handler blocks,
`ensure:`'s, a lexical send's (decided with the user on 2026-11-14): it
leaves the whole send that took the block, not only the block, and stops
where the block's home goes on once that send has answered, as a step
over of that send would. A debugger steps through what the source shows,
and the source shows a loop's body as part of the loop: stepping out of
`whileTrue:`'s body stops after the loop, never at its condition's next
evaluation, and stepping out of `do:`'s block stops after the `do:`,
never at the next element. Leaving only the block, as a frame would --
the next round's condition, the next element's block -- is rejected: it
makes the user hold the control structures' implementation in mind.
IntelliJ's Kotlin debugger steps out of an inline lambda past its call in
the same way (not under `prior-art/`).

The send is found in the frame the step is taken in, before the step
starts (`stepping::send_left`): first the innermost inlined form one of
whose inline blocks holds the instruction the frame stands at, in its own
body, from the metadata's block sends ("Compiling the metadata again") --
the smallest such block, since inlined forms nest, so that one step out
leaves one block level; else, for a frame that runs a closure, the send
its creator -- the frame its captured frame names, running or suspended --
stands at, when that send took this very closure, which is decided at the
stop from the frames themselves: walking down the closure's senders meets
the creator, and the frame just above it, the send's callee, is the
closure's own activation -- a `value` the creator sent -- or holds the
closure object the stopped frame runs (`Heap::activation_closure`,
`Heap::frame_closure`) as its receiver or in one of its parameters, the
registers below its body's argument count, which no body writes; or the
callee is a `doesNotUnderstand:` handler, to which the send delivered its
arguments as its `Message`'s, and the closure is one of the `Message`'s
`arguments`, read through the kernel's `Message` layout as `sendTo:`'s
leaf reads it (`Mutator::kernel_slot`), no Newspeak code run
(`stepping::took_closure`) -- only when the creator's send is of another
selector, so that the `Message` is the lookup's delivery: a `Message` a
program sends `doesNotUnderstand:` itself is an argument like any other,
whose arguments no send took
(`a_step_out_of_a_block_in_a_message_sent_explicitly_leaves_its_frame`).
What the send delivered decides, so that a proxy's `do:` leaves the `do:`
(`a_step_out_of_a_block_a_does_not_understand_takes_stops_after_the_send`),
and the creator's registers never do: the run loop reads a single
argument, or consecutive ones, where they stand and copies them into the
send's slots only for a callee laid on them, so a slot the send did not
read still holds what an earlier send of that slot took, and a named local
the send read from lives in a register an inlined form's fallback closure
assigns, which a callee can run while the send runs
(`a_step_out_of_a_block_a_does_not_understand_takes_in_a_variable_stops_after_the_send`,
`a_step_out_of_a_kept_block_a_does_not_understand_runs_stops_in_its_caller`,
`a_step_out_of_a_block_assigned_to_a_sent_local_meanwhile_stops_in_its_caller`,
each with the JIT off too, since machine code writes every argument into
its slot). The one thing the decision does not see is a handler that
stores into its own `Message`'s `arguments` before the closure runs: it
then answers what the array holds, not what the send delivered. A block
literal, a parenthesized one, a closure handed on in a variable or
computed by an expression are all taken alike; the creator's literal
entry, when the closure's block is the literal the send took, only says
whether the send is an inlined form. A frame of a method, a closure whose
creator returned, and one whose creator's send did not take it -- a
sibling closure that runs it, a closure kept and run by another send --
step out as any frame does. A frame at a loop's own statement start,
before its first round, stands at no instruction of its blocks yet, and a
step out there leaves the frame. So does a frame of a body without
metadata, as an image built with `--strip-source` has: no inline block of
it is known, so a step out inside one falls back to a frame step out,
while a closure's creator's send is still found and left for the index
after it, since that needs no metadata -- though without it no send is
known to be an inlined form's, so a fallback send or a `SendInlinable` is
left as a real send is.

An inlined form -- and its reference-lowered counterpart, a
`SendInlinable` or the loop of `value` sends, and the optimized
lowering's fallback send, which the form's entry names -- is left for
where the form has answered: the stepping frame is flooded as for a step
over, but for the slots of the form's inline blocks and the bodies of its
closures and of every block made inside its inline ones
(`stepping::LeftBlocks`), and a one-shot stops at the form's join,
`after`, when that lies mid-statement -- the form an operand, an argument
or an assignment's value, the rest of its statement still to run, as a
real send's return point is (`stepping::join_stop`). So the step stops at
the join mid-statement, else at the first statement start the frame
reaches once the form is done -- the statement after a loop, a loop's next
condition when the form ended its body, whose join is the loop's back
jump -- or at the stepping frame's return point when the frame returns
first, the same under both lowerings. A real send is left for its return
point in the creator, the index after it: a one-shot that stops and
records there for the creator (`Stepping::after_send`), the send's answer
reported, beside the stepping frame's return point; the step's frame is
the creator's, so that it holds no handle of the closure's activations,
which return to the send and are replaced by the next element's at the
same offset. The creator leaving otherwise than by a return is reported
as the step's frame is (`Stepping::has_left`): a non-local return out of
the closure, an exception a handler below takes, an unwind, each going on
where the program goes on. A real send that is its body's last
statement, a method's without a `^` or a block's, is left for the
return the body ends with, which no statement holds: that stop stands at
the statement it follows, as every program counter no statement holds
but one ends at does (`stack::location`;
`a_step_out_of_a_block_the_last_statement_took_stands_at_that_statement`),
so that the IDE shows the send just left rather than the whole body.

The two kinds of leaving meet decision 12's and the next run's: a step
out of an inlined form that is an initializer's last statement leaves
the initializer, passes over the factory and stops after the `new` in
the caller, or, in the root's instantiation run, goes on in the next
run's `main:args:`; a block handed to a factory and run by a slot
initializer is left with the factory's send, after the `new:`; and an
inlined form's fallback send, a user class answering `ifTrue:`, is left
as its form is, at the next statement or at its join mid-statement
(`a_step_out_of_an_initializer_s_last_form_stops_after_the_new`,
`a_step_out_of_an_initializer_s_last_form_goes_on_in_the_next_run`,
`a_step_out_of_a_block_a_slot_initializer_runs_stops_after_the_factory_s_send`,
`a_step_out_of_a_fallback_send_s_block_leaves_the_form`).

## Planned: the mechanisms

What building the decisions changes, found by checking them against the
code and the invariants in `CLAUDE.md` in three reviews; each is part of the
design, not an option, unless it names an open question ("Open
questions"). The mechanisms stages 1 and 2 built -- the poll words as
atomics, the commands served while the program runs, reading the stack at
a stop, compiling the metadata again and the values a stop holds -- are
described in "The debugging API at the safepoint", with what of them is
still planned; a mechanism below that stage 3, 4 or 5 built says so,
and what of it is still planned, in its first line; one that says
nothing is planned whole.

### Where the original lives

**Built** (`bytecode/compact.rs`, `EncodedBody::patch`, `restore`,
`original`).

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
`JitStatistics::breakpoint_exits` and against no exit budget
(`jit::exit_counts`) -- it is the debugger's, not the code's, and
counted it would abandon a body stopped at often, to be revived late
after the breakpoint is gone -- and a body whose patch it reaches before
any transfer exits at once and is not installed. The optimizing
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

**Built** (`mixin.rs`, `MixinMethod::switch_fast_path`;
`debugger/breakpoints.rs`).

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
`recognized_fast_path` once the last patch of the body goes, but for
the breakpoint stress mode's, which never stops and so keeps no send
from the fast path ("The breakpoint stress mode") --
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

**Partial:** the stop and the going past are built for every patched
slot -- a line breakpoint's, a method breakpoint's, a step's and run to
cursor's one-shots, and the breakpoint stress mode's -- and so are hit
counts, conditions and logpoints, an evaluation goes past every slot
uncounted, and a restart and a forced return leave no token; what
edit-and-continue does with the token is planned.

A `Breakpoint` slot shares the run loop's out-of-line arm of the
simultaneous slot clauses (`vm::simultaneous_clause_instruction`): an arm
of its own, one more target of the `match`'s jump table, grew `vm::run`
past the `codegen` gate's budget, where the shared one costs nothing. That
call hands the decision to `interpreter/` (`vm.rs` holds no dispatch
logic): `interpreter::breakpoint::reached`, `#[cold]`, which answers
`BreakpointDecision::Execute` with the slot's original or
`BreakpointDecision::Stop`, having counted a hit of each breakpoint
standing there ("Hit counts and temporary breakpoints"). A breakpoint
with a condition or a log it neither counts nor decides, since no code
runs in the arm: it records the stop with those breakpoints left to the
safepoint (`PendingStop::conditions`), which evaluates each in the
reaching frame and then counts, logs, and stops or lets the frame past
the slot (`debugger/conditions.rs`, "Hit counts and temporary
breakpoints"). A slot it does not stop at it goes past as
a resumed stop's token lets it past (`gone_past`), so that what a stop
there would owe is owed all the same. Going on, the arm replaces the
fetched slot by the original and re-enters the loop's `match` with it --
the `match` sits inside a dispatch loop that runs once per instruction
unless the arm continues it with the original, which costs nothing until
the arm is reached, since breaking the loop's outer labelled blocks from
inside it is legal; a check before the `match` would cost every
instruction a compare -- so every kind of instruction, sends included,
runs through its ordinary arm and the original's behaviour is the
interpreter's own; `run_wide`'s way, the effect functions called out of
line, is closed to a send (`unreachable!("internal: a send is never
encoded through Wide")`), and statement starts and return points are often
sends. Stopping, it records the stop on the `Mutator`
(`Breakpoints::request_stop`) and the arm leaves the stretch as
`Leave::Safepoint`, the frame standing at the slot: the stop is the
safepoint's (`serve_debugger`, "Serving at the safepoint"), never the
arm's.

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
A restart and a forced return leave none: the stop that starts one goes
on without paying the token it owes (`Served::Unwind`), since a restart
does not pop its activation and would otherwise go past the one-shot at
its first statement, and the step out a forced return begins drops the
token it sets for the frame on top, which the unwind pops. Planned:
edit-and-continue drops it explicitly too. Breakpoints an
evaluation goes past neither consume nor drop it: the arm decides so
before it takes the tokens (`Breakpoints::evaluating`), and the stop
names the top frame afresh once the evaluation is over, the suspend
stress mode having perhaps resumed it elsewhere meanwhile. A
`Continuation::Reexecute` runs a caller's instruction again at the same
index once a future it met is resolved (`vm::reexecute`); when that
instruction is a patched one, `interpreter::breakpoint::reexecuting` sets
the token again, so the breakpoint neither stops, counts nor logs twice.
The `codegen` gate's budgets of `vm::run`, `resume_caller` and `finish`
hold what the `Cell` slots and the arm cost the hot path.

### Breakpoints by body, and reclaimed code ids

**Built** (`debugger/breakpoints.rs`, `jit::free_for_patch`).

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

**Built** in the VM (`Command::SetBreakpoints`,
`debugger/breakpoints.rs`), the runtime's answer of the breakable places
before a session (`Interpreter::breakable_statements`,
`debugger/metadata/breakable.rs`), the agent's line to offset
(`crates/ns-cli/src/debug/breakpoints.rs`), and the language server's
request `newspeak/breakableLines` (`crates/ns-analyzer/src/breakable.rs`;
[editor-support.md](editor-support.md), "Breakable lines").

The IDE sets a breakpoint at a file and line; the agent turns the line
into a byte offset into the module's current text -- the start of the
line, or, for a breakpoint with a `column`, the place of that column, a
block variant -- and sends `SetBreakpoints` with the module's name and the
offsets. The VM resolves each offset (`debugger::breakpoints`) against
every current body of the module -- methods, blocks (a block's body is a
literal of its home, so a block never yet created is patched in place all
the same), slot initializers, superclass clauses and initializer
expressions, each a body of its own, of every level of the module, both
sides -- through their metadata, compiled again for the purpose
(`metadata::Cache::level`): to the first statement that compiled to an
instruction and starts at or after the offset, and patches the first
instruction of every statement with that same span, in every body. A body
without metadata is not patched. A shipped module's text is its shipped
text. A breakpoint whose module is not built yet -- a program module not
loaded, a shipped module built on first demand through the mirror -- is
pending (`BreakpointState::statement` is `None`) and is resolved when the
module's root class is built (`Mutator::load_root_class`,
`core_library::module_class`, before its factory and initializer run),
reported unasked by `Event::Breakpoints`. An image's load builds its
modules in an interpreter of its own, which no debugger is attached to
yet. An install replaces bodies: the edited module's breakpoints are
resolved again by their offsets against its current bodies, a patch on a
current body a breakpoint no longer stands at is removed, a moved one is
reported unasked, and frames still running the old bodies keep theirs,
whose patches stay until the breakpoint is removed -- or until the
module's set is sent again, which an IDE does at every change of it: a
`SetBreakpoints` removes the module's breakpoints everywhere, old bodies
included, and resolves the new set, under new ids, against the current
bodies only, so a frame still running an old body runs past them.

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
places come from `ns-next-analyzer` (decided with the user on 2026-11-14),
through a request of the language server of its own: it parses and
resolves the document's module and asks
`Interpreter::breakable_statements`, and answers the lines a statement
starts on and, for each, the block bodies that begin on it, the line
breakpoint's variants. `breakable_statements` compiles every level of the
module, both sides, through the metadata's collector by the function a
loaded level is compiled again with (`again::compile_unloaded_level` over
`Mutator::mixin_level`), which registers nothing -- no mixin, code id,
module or class -- and runs nothing, and answers each statement that
compiled to an instruction, as `SetBreakpoints` resolves an offset: of two
starting together only the outer, an inlined block's two copies once,
sorted by start, each with the block literal it is the first breakable
statement of, a closure's and an inlined block's alike. It compiles under
the optimized lowering, whatever the interpreter's, since the answer comes
before any session and cannot depend on the lowering one chooses: the
reference lowering compiles a few statements to an instruction the
optimized one compiles to none -- a block's last statement that only names
a variable -- which the answer leaves out, and a breakpoint a session
under it resolves to one is reported moved. `ns-cli`'s `debug_metadata`
holds the answer, for every golden of `goldens/language/`, to where
`SetBreakpoints` lands under each lowering. The plugin keeps the answer
per document version and allows every line of a document that does not
parse. The session verifies what was set before it: the VM resolves each
breakpoint as above and the agent reports one it moved through DAP's
`breakpoint` event. A Kotlin rule of where statements start is rejected:
the plugin has no parser of its own, only a lexer held to `ns-syntax`'s,
and the compiler alone knows where a statement compiled to an instruction.

### Hit counts and temporary breakpoints

**Built** (`HitCondition`, `Breakpoints::count_hit`,
`interpreter/breakpoint.rs`; the agent's parsing,
`crates/ns-cli/src/debug/breakpoints.rs`), and so are conditions and
logpoints in the VM (`LineBreakpoint::condition` and `log`,
`debugger/conditions.rs`; tests `conditions::*` in
`crates/ns-runtime/tests/debugger.rs`); the agent passes neither yet.

Decided with the user on 2026-11-14 for stage 4. A breakpoint's hit
condition -- DAP's `hitCondition`, IntelliJ's pass count -- is counted by
the VM: `SetBreakpoints` carries it for each line breakpoint
(`LineBreakpoint::hit_condition`) and `SetMethodBreakpoints` for each
method breakpoint (`MethodBreakpoint::hit_condition`), a `HitCondition` --
`Equal(n)`, `Greater(n)`, `GreaterOrEqual(n)` or `Multiple(n)`, the last
holding at no hit for 0 -- or `None`, which stops at every hit. The agent
parses it from DAP's string (`parse_hit_condition`), in the forms `n`,
`=n`, `==n`, `>n`, `>=n` and `%n`, `n` a decimal `u32`, a plain `n` the
hit numbered so, as IntelliJ's pass count and JDWP's `Count` mean it; a
malformed one is never sent, its breakpoint answered unverified with the
message saying so (`a_pass_count_stops_at_that_iteration`).

The count lives on the breakpoint's record in the table
(`debugger/breakpoints.rs`), starts at none when the breakpoint is set
-- every `SetBreakpoints` and `SetMethodBreakpoints` sets its whole set
afresh, under new ids, so sending a set again restarts every count of
it -- and is counted only in the cold decisions of
`interpreter/breakpoint.rs`, never on the run loop's path, so a run
without breakpoints takes no new branch and a hit that does not stop
costs a counter, not a stop and a round trip to the agent's thread. A
hit is a reach of the breakpoint's slot by an activation: the
`Breakpoint` arm's decision counts each line breakpoint, method entry
and method exit standing there, the token a resumed stop set letting
the frame past its slot once uncounted, as it does a re-executed
instruction (`reexecuting`); a method's exit by an unwind is counted
where the unwinder reports it (`left_by_unwind`, `returning_by_unwind`),
and an exit at a non-local return counted at the return is not counted
again where the return arrives, whether or not it stopped. For a slot
that is both a method's entry and its exit, the entry is counted first
and the exit, if the entry stopped, as the frame goes past the entry's
stop. The arm stops only for the breakpoints whose condition holds,
and only those are named by the stop's reason; a reach for which none
holds and no step or run to cursor stops goes past the slot as a
resumed stop's token lets it. A step's, a step into's or a run to
cursor's stop at the same slot is made whatever the conditions say,
and names only the breakpoints whose condition holds there: none makes
it the step's (`StopReason::Step`).

Each reach of a slot counts each breakpoint standing there once. A stop
the arm decided and the safepoint does not make -- the commands served
there removed every breakpoint it was for (`PendingStop::still_standing`),
or an exception's stop is made instead -- leaves the frame to fetch the
slot again with no token, so the table remembers the reach
(`CountedReach`: the activation, the body and the index, the breakpoints
counted that did not hold and those that held), consumed or dropped by the
next `Breakpoint` arm and moved by the suspend stress mode as the token
is: that fetch counts none of them again, stops for the held ones that
still stand, and counts only a breakpoint set there since. Without it, a
breakpoint counted at the reach but not holding, beside one a
`SetBreakpoints` replaced, would count the same reach twice
(`a_stop_not_made_counts_no_hit_twice`).

JDWP counts in-process the same way, its `Count` modifier decremented at
each event and the event reported at zero
(`jdk/src/jdk.jdwp.agent/share/native/libjdwp/eventFilter.c:482-488`).

A breakpoint's condition -- DAP's `condition`, IntelliJ's -- is decided by
the VM too, before the count, as JDWP applies `Count` after the other
filters, and evaluated exactly once per hit, never again by the IDE, so
that a condition with a side effect sees every hit once
(`a_condition_is_evaluated_once_per_hit`). `LineBreakpoint::condition` and
`MethodBreakpoint::condition` carry its text, `log` a logpoint's message,
parts of text and expressions (`LogPart`). The `Breakpoint` arm, where no
code may run, leaves every breakpoint with a condition or a log standing
at the slot to the safepoint, uncounted (`PendingStop::conditions`), and
records the stop even when nothing else stops there. The safepoint
evaluates each condition in the frame that reached the slot, an evaluation
as `Command::Evaluate` runs one ("Evaluation"), one at a time: it begins
the evaluation and keeps the decision in the `Debugger`
(`conditions::Deciding`) as it keeps a stop, the run loop runs it, and the
safepoint its end asks for goes on with the next (`Interpreter::decide`).
A breakpoint whose condition answers `true`, and only `true`, is counted
and decided by its hit condition; one whose condition answers anything
else is neither counted nor stopped for
(`a_condition_counts_before_the_hit_condition`). A logpoint that holds has
its message's expressions evaluated the same way, each as `(expression)
printString`, whose `String` joins the text, and sends `Event::Logged`
with the message instead of stopping (`a_logpoint_logs_without_stopping`).
A condition or log expression that does not compile -- a `^` among its
causes, as in every evaluation -- or raises an error stops the program at
the slot with `StopReason::ConditionFailed`, naming every breakpoint that
failed there and the first failure's description, as IntelliJ stops when a
condition fails (`a_failing_condition_stops_and_says_why`,
`a_failing_log_expression_stops_and_says_why`), and, after them, every
breakpoint that held there and every method exit by an unwind the stop
reports, so that IntelliJ removes a temporary breakpoint hit there all the
same (`a_failed_condition_s_stop_names_the_breakpoints_that_held`). A
reach none of whose breakpoints holds and no step stops at goes past the
slot as a resumed stop's token lets it (`PastOnce`); otherwise the stop is
made as the arm's would be, every condition decided first, and the
commands served while they ran -- a `SetBreakpoints` -- may have removed
some of what it is for. A stop not made remembers what the reach decided,
the conditions that did not hold and the logpoints that logged among the
passed (`CountedReach`), so that the fetch of the slot again evaluates
none of them twice; a condition not yet evaluated when an exception's stop
is made instead is evaluated by that fetch, once. A pause asked while a
condition runs aborts it ("Evaluation", "Aborting") and stops the program
at the slot, for the breakpoints that held there and the exits it reports,
or else as the pause's stop. Each expression is compiled once per
breakpoint, body and slot and kept on the breakpoint's record
(`CompiledConditions`), dropped with the breakpoint and at every install,
which changes the text it was resolved against. A step into's stop at a
pushed frame's first statement, which is the step's whatever the
breakpoints there say, decides their conditions in that frame first, as
the safepoint decides a reach (`conditions::Deciding::at_push`): it is the
breakpoint's where one holds, and the step's otherwise, and a logpoint
there logs (`a_step_into_decides_a_condition_at_the_first_statement`). A
method's exit by an unwind -- an exception a caller handles, a non-local
return past it or arriving at it -- is decided in the frame the unwind
leaves, before the unwinder pops it or returns from it, as it runs an
`ensure:` block there, as JVMTI's `MethodExit` fires for an exit by an
exception in the frame still on the stack: the unwinder pushes the
condition's evaluation above that frame with the unwind it interrupts
(`Continuation::ExitEvaluated`), whose return decides the exit and raises
the unwind again, and an error that ends it does the same at its base
(`interpreter/unwind.rs`, `end_at_base`); the exit is then counted and
decided as an ordinary one, logged, or reported where the program goes on,
as every exit by an unwind is (`conditions::decide_unwound_exits`;
`a_condition_decides_an_exit_by_an_exception`,
`a_condition_decides_an_exit_by_a_non_local_return`,
`a_method_logpoint_logs_an_exit_by_an_exception`). A failure there is
named by the stop that reports the exit (`conditions::failure_named`,
`a_condition_failing_at_an_exit_by_an_unwind_is_named`), the attached
debugger's next stop, never a stop a stress mode's reader serves, which
would take the failure from the debugger
(`a_stress_pause_leaves_an_exit_s_failure_to_the_debugger`); a pause that
aborts it makes that report the pause's stop. An exit whose unwind ends
the run is reported by the step the next run carries (decision 17), and
the failure goes with that step: named by its stop at the next run's first
statement, or by a pause asked between the runs, which reports the step's
exits; dropped with it by a `Terminate` or a detach between the runs; and
dropped at a run's end that carries no step (`evaluation::run_ended`,
`stepping::end`;
`a_failure_owed_at_a_run_s_end_is_named_by_the_stop_that_reports_its_exit`,
`a_failure_owed_at_a_run_s_end_goes_with_a_terminate_between_runs`). A
temporary breakpoint needs nothing of the agent or the VM: IntelliJ's
debugger session removes a breakpoint marked to be removed once hit when
it reports the stop, which the `stopped` event's `hitBreakpointIds` tell
it.

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

The flood covers the stepping body's own statement starts and those of the
inlined blocks in it, from its metadata, and the statement starts of each
block literal the compiler lowered inline (`stepping::inlined_blocks`):
under the optimized lowering, the fallback closure a receiver that is not
a Boolean runs, recognized as a block literal whose `source` encloses one
of the body's statements, its inline copy's; under the reference lowering,
which inlines nothing, a block literal a `SendInlinable` takes as its
receiver or an argument, the blocks the optimized lowering inlines; and so
on into those blocks' own. A simultaneous slot's computation, the closure
a `MakeFuture` takes, is none of them, though its source is the very span
of the initializer's statement for the slot: its future's `resolve` runs
it, a frame of its own. So a step over a non-Boolean receiver's `ifTrue:`
stops in its fallback block as in the inline copy, and a step under the
reference lowering stops where it stops under the optimized one. A step
taken in such a block steps in its creator, the frame whose inline code
the block stands for (`stepping_frame`), so that a step over the block's
last statement goes on at the creator's next statement start rather than
in the receiver's `ifTrue:` or in a loop's `value` send; a creator that
returned leaves the block a frame of its own. A step out taken in such a
block leaves the form that took it, as one taken in its inline copy does
(decision 17). A block the body creates otherwise is a body of its own and
not flooded, so a step over a `do:` whose argument block contains a
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
request is taken at every poll until then, not only at pushes: a backward
jump in the stepping frame, or a caller resumed, also reaches the
safepoint, which goes on without stopping when nothing was pushed. A loop
with no send, or a statement whose sends are all answered by primitives,
so takes the safepoint at every iteration until a push or a one-shot
breakpoint ends the step, and machine code's loop-head poll exits at each.
The bit is one of `LOWERS_DEPTH_LIMIT`, as the pause bit is ("The poll
words as atomics"), so that a compiled prologue refuses the push and the
interpreter's push takes the safepoint before the callee runs. The
safepoint recognizes a push by a frame on top whose sender is the stepping
frame or a block the step flooded for it -- not by a `program_counter` of
0, which a backward jump to the stepping body's first instruction, or a
frame `retry` restarted, also shows, nor by a frame merely above the
stepping one: a step taken in an inlined block steps in the block's
creator, and the frames between the two, the receiver's `ifTrue:` among
them, and what they send to are not the step's. A frame that runs a
flooded block stands for the stepping frame's inline code and is no push,
nor is a frame that lay between the one the step was taken in and the
stepping frame when the step began, which goes from that list once the top
lies below it, so that a frame the stepping frame pushes later at its
offset is a push. The push drops the bit and stops at the new body's first
statement start: at once when its frame stands there, at index 0, and
through a one-shot for the pushed frame at index 1 for a body -- a
method's or a block's -- whose first instruction makes its indirection
vector (`MakeVector`, emitted before the local initializers and outside
every statement's range, `compiler/lower.rs`), or, for an initializer of a
declaration that writes no superclass clause, at the first statement start
after the clause that stands for `Object new`, the step stopping where its
frame goes on when there is none (`stepping::first_stop`). A factory's
push stops in the factory, and the push of an initializer a clause entered
stops at its entry, at its header, the step from there going on to its
first statement; `Object`'s and `Top`'s initializers keep the request
(decision 12). A send answered by a primitive or a leaf pushes nothing,
and the next one-shot stops.

### Writing a frame

**Built:** a frame an evaluation reads and writes, setting a
variable (`debugger::evaluation::begin`, `Command::SetVariable`), and a
frame restarted or forced to return (`Command::RestartFrame`,
`Command::ForceReturn`; "Restart frame and force return").

A variable is set by an evaluation: `Command::SetVariable { frame, name,
expression }` compiles the assignment `name:: expression` in the frame
(`debugger::evaluation::compile_assignment`) and runs it as `Evaluate`
runs an expression. The expression is parsed on its own and must be one
expression without temporaries, and the assignment is built around it as a
syntax tree node, never spliced around it as text: text such as `3). (9`
would close a spliced `x:: (…)` early, assigning one value while the
evaluation answered another. The name must be the variable the frame shows
under it -- the innermost of the name, which the assignment's resolution
names too -- and assignable, or the command is refused as not assignable,
the name never read as source; an assignment to a variable a block may
already hold a copy of is refused as an evaluated one is ("Assigning a
captured variable"). Its answer, `Event::VariableSet`, is the variable
holding the value the assignment answered, which is what it wrote; an
expression that ends with an error answers `Event::Evaluated` with it, the
variable unwritten.

Reading a compiled frame needs nothing; a frame whose variable is set, or
that is restarted or forced to return, goes on in the interpreter -- a stop
at an optimized poll having been deoptimized first ("The stop"): its
compiled mark is cleared (`Heap::clear_compiled`, as the run loop does
where it gives a frame to the interpreter), as
[liveness-and-reflection.md](liveness-and-reflection.md), "The unifying
principle", requires. A frame an expression is evaluated in is one such:
the evaluation may write it, so its mark is cleared before the
expression runs, whether or not it writes.

### Evaluation

**Built:** evaluating an expression at a stop
(`Command::Evaluate`, answered by `Event::Evaluated`;
`debugger/evaluation.rs`; tests `evaluation::*` in
`crates/ns-runtime/tests/debugger.rs`): where it lies, how it returns,
the frame it reads, its errors, its depth reserve and aborting it,
below; and a breakpoint's condition and log, evaluated the same way at
the safepoint a reach leaves for ("Hot conditions"; "Hit counts and
temporary breakpoints"). `compiler/evaluation.rs`
parses it as a code body standing alone (`ns_syntax::parse_code_body`),
`resolver::resolve_evaluation` resolves it -- a marked label is the
block's parameter, a name of the frame's variable table
`Binding::FrameVariable`, every other name resolved against the stopped
body's class chain (`metadata::again::declaration_chain`, read from the
module's current text as the frame's metadata is), an assignment to a
parameter, an immutable local or a label refused as not assignable
(`VariableEntry::assignable`, which both lowerings' collectors record),
and every `^` refused, nested blocks included -- and both lowerings
compile it as a block whose frame variables are reached one hop past its
own frames, where the stopped frame's variable table places them: a
register by `LoadOuterLocal` and `StoreOuterLocal`, a vector variable
through its vector. It is refused, the program still stopped, in a frame
of the core library ("Evaluation never reaches the VM mirror"), in a
frame whose source is not known -- a module without text, a body its text
no longer compiles to, a top-level frame -- and when it does not compile:
`"the expression does not compile: <message>"`, `"<name> is not
assignable"` or `"a ^ cannot leave an evaluation"`.

- **A vector not made yet.** A frame stopped at its push stands before
  its `MakeVector`, its vector's register still nil. A vector variable is
  in scope from its body's start and nil until written, so the variables
  view shows it nil, and so does the evaluation: `LoadVector` answers nil
  for a nil holder, in the cold path its error took
  (`interpreter::locals::load_vector`; machine code exits for every holder
  that is no `Array`), a case only a block an evaluation made can reach.
  An assignment to it there -- evaluated or through `SetVariable` -- is
  refused, `"<name> does not exist yet at this point"`, decided where a
  copy's refusal is (`Assignability::NotMade`, `stack::vector_not_made`):
  the value would have nowhere to live, and a vector the debugger made
  would be replaced by the frame's own `MakeVector`
  (`a_vector_variable_before_its_vector_reads_nil_and_refuses_assignment`).

- **A variable not available.** A variable the variables view shows as
  not available -- a home's variable its frame dropped as it returned,
  or a register not live at the stop -- lives nowhere: a read would
  answer a nil it never held, and a write would go nowhere. Both are
  refused, `"<name> is not available here"`, decided per frame variable
  by the reader the view reads with (`FrameVariable::available`,
  `stack::available`), for the variable a read names -- the innermost of
  its name, the one the view shows -- and for the one an assignment
  writes (`an_unavailable_variable_is_refused_for_reads_and_writes`).

- **Where it lies.** The expression's activation is pushed above every
  live frame's whole register window (`Mutator::push_step`, which lays
  every activation that does not take a send's own arguments above the
  top frame's window), whichever frame it evaluates in: IntelliJ
  evaluates in any selected frame, and a frame below the top has its
  callees above its window; at a statement start inside an inlined
  block, scratch registers of the enclosing expression may still be live
  in the stopped frame too.
- **How it returns.** Its return does not go through `resume_caller`: the
  stopped frame's slot is not a send, and `send_destination` would reach
  its `unreachable!`. `Continuation::Evaluated`, the base activation's,
  records the result (`evaluation::returned`), asks for a stop
  (`Heap::request_debugger_stop`, under the arming lock, as an exception
  breakpoint asks), and switches to the frame below at the program counter
  it stopped at, delivering nothing (`vm::evaluated`), so that the
  safepoint the request makes is taken before that frame runs another
  instruction, interpreted. The stop is left for the evaluation with
  `Served::Evaluate`, kept in the `Debugger` ("The stop"), which
  `Interpreter::safepoint` answers as `SafepointOutcome::Rebuilt`, in
  every build, so that the run loop reads the evaluation's frame, now on
  top; the safepoint the end asks for re-enters the kept stop
  (`Interpreter::serve_evaluation`), which answers `Evaluated` and serves
  on. `Event::Stopped` is sent once per stop, `Event::Evaluated` once per
  evaluation. While the evaluation runs a safepoint serves the commands
  queued meanwhile, and a pause aborts it ("Aborting" below). The variant
  is named in [image-format.md](image-format.md), "Planned: suspend and
  resume", for completeness; no image is written while an evaluation runs,
  so it is never encoded, and the encoder, which writes dead frames only,
  has no match over continuations.
- **The frame it reads.** The stopped frame loses the registers the
  leftover analysis finds tainted at the program counter it stores
  (`Heap::nil_tainted_registers`, below the window of the frame above
  it), so that a variable not live at the stop reads nil, never a word a
  callee or a popped frame left; it is reified and its compiled mark
  cleared, so that it goes on interpreted -- a frame the debugger may
  write needs no machine code; the run loop could enter a reified frame
  in its body's reifying twin (`jit::entry_of_frame`), so this is a
  choice, not a necessity; a stop at an optimized poll is deoptimized
  before ("The stop"). The debugger makes the expression's closure
  itself, as a `MakeClosure` in the frame would, the stopped frame its
  creator, so that `LoadOuterLocal` and `StoreOuterLocal` at depth 1
  reach the frame's registers. A block the expression creates can escape
  (`[count]` stored away), so when the expression creates a block that
  reaches the frame -- an `outer_reach` of 2 or more, one hop past the
  expression's own frame -- the frame is marked as `MakeClosure` marks a
  creator whose block has an `outer_reach` above 0
  (`Heap::frame_retain_registers`, which marks a suspended frame's
  record too), or it would drop its registers when it returns and the
  block would read `nil` at exit status 0
  (`a_block_an_evaluation_stored_reads_the_frame_after_it_returned`).
  So is every frame further along the frame's captured chain that the
  block reaches -- an `outer_reach` of 3 marks the frame's captured
  frame too, and so on (`evaluation::retain_reached_frames`): a block of
  the program marked its creator only as far as its own reach, so a
  stopped block that never read its home's `unused` left the home
  unmarked, and the expression's `[unused]` would read `nil` once the
  home returned
  (`a_block_an_evaluation_stored_reads_an_enclosing_frame_after_it_returned`).
  An expression that only reads or writes the frame's variables reaches
  them while the frame is stopped, so its frame is not marked: retaining
  its registers would keep alive what the frame drops at its return and
  show it to every later stop
  (`a_read_at_a_stop_leaves_the_frame_to_drop_its_registers`).
  While it runs, every `Breakpoint` arm goes past its slot uncounted
  (`Breakpoints::evaluating`), as IntelliJ skips breakpoints in an
  evaluation, no method's exit by an unwind is counted, no exception
  breakpoint stops, and no step is in progress, the stop having ended it.
- **Its errors.** The handler search walks below the expression into the
  program's own `on:do:` frames, and a program's handler would catch the
  expression's error and unwind the program's frames. So the evaluation's
  base frame ends the handler search (decided with the user on
  2026-11-16; `next_active_handler` in `builtins/activation.rs`), and a
  search that starts below it -- an exception the program signalled
  before the stop, which the expression passes on -- finds no handler:
  an exception no handler within the expression takes runs
  its `defaultAction` as it would in the program, and where that ends
  the run -- `Exception>>defaultAction` does, through the `terminate:`
  leaf -- the unwind ends at the base, and the evaluation answers the
  error, the exception object held at the leaf so that it can be
  inspected (`Evaluation::Error`, its description the exception's class
  and text, `an instance of MessageNotUnderstood: an object does not
  understand #secret`); `retry` and `pass` cannot cross the base. A
  handler for every `Exception` at the base, which answered the exception
  itself, is the way not taken: a program class may override
  `defaultAction` and have `signal` answer its value -- a resumable
  `Missing` whose `defaultAction` answers 0, or an `Error` whose does --
  and the expression must answer what the same code answers in the
  program, where a handler at the base would answer the exception
  (`an_overridden_default_action_answers_as_in_the_program`); the
  Newspeak IDE's `on: Error do:` at its base
  (`newspeak/ActivationMirrors.ns:368`) cuts off the same for an
  `Error`. The `^` is refused by the expression's compiler, and the
  evaluation's base frame is an unwind barrier the unwinder stops at
  (`interpreter/unwind.rs`, `at_evaluation_base`, reached only while the
  debugger follows unwinds, which it does while an evaluation runs): a
  program block the expression calls whose `^` would return to a home
  below the evaluation -- `b value` for a `b` holding `[^1]` -- is
  stopped there, and the evaluation answers the error a return to a home
  it cannot reach signals (`CannotReturn`, signalled from the base), the
  program's frames untouched. Only the debugger's own termination goes
  past the base, to the run's floor. Machine code's cut already exits for
  a home below its stretch's entry frame (`jit/cut.rs`), so the
  interpreter's unwinder is the one place that changes.
- **At the depth limit.** An evaluation has a depth reserve of its own
  (decided with the user on 2026-11-16), since the stop that needs one
  most is an exception breakpoint on the stack overflow itself, where
  every watch, condition and renderer would otherwise fail. While it
  runs, the depth limit is `max(L, base + E)`, the base the evaluation's
  base frame's counted depth and E 1,000 counted activations
  (`EVALUATION_DEPTH_RESERVE` in `interpreter/mod.rs`): far below L
  nothing changes, at L the evaluation has E. `evaluation::begin`
  raises it before the base's push, which a stop at L would otherwise
  refuse, and its end gives it back. A VM error within it takes the
  signalling reserve above that limit as it would above L
  (`unwind::start_signalling` and the kernel's `doesNotUnderstand:` read
  it through `Mutator::ordinary_depth_limit`; "The depth limit",
  [execution.md](execution.md)); a reserve the program held at the stop
  -- its own overflow's `signal`, still searching -- stays held and
  raises the evaluation's limit the same way. The raise is a term of the
  heap's beside the limit `set_depth_limit` sets
  (`Heap::raise_depth_limit_for_evaluation`), which
  `synchronize_depth_limit` adds into `DepthLimit`, so the push's check
  stays its one compare against L and machine code's against the word;
  the cold `push_past_the_limit` alone reads the evaluation's limit.
  Whatever the evaluation overruns -- its limit and reserve, or the
  stack reservation's room -- never ends the run: where a program's
  overrun answers `Signal::Terminate`, that termination inside an
  evaluation is unwound to the evaluation's base frame, which ends every
  termination but the debugger's own, its `ensure:` blocks run, and
  answered as the evaluation's error, the program's frames untouched;
  the error is picked by `overrun_of_the_reserve`'s walk, which stops at
  the base, so that the program's own error below it is never the
  evaluation's (`an_evaluation_near_the_depth_limit_that_overruns_is_its_error`,
  `an_evaluation_at_a_stack_overflow_has_room`). Evaluations do not
  nest -- an evaluation skips breakpoints, so no stop arises within one
  -- so the stack holds at most L + `DEPTH_RESERVE` + E counted
  activations, plus the reserve of an error signalled within the
  evaluation, and the frame bound of "The depth limit" grows by that
  term. The native stack's need does not: the stop is a safepoint,
  where no compiled frame holds a return address, so machine code
  within the evaluation nests at most E + `DEPTH_RESERVE` frames above
  its base, or as far as L and its reserve allow when the base lies far
  below L. JDI and pdb give an evaluation no room beyond the program's
  limit; their limit's headroom, like ours, is for the error's handler.
- **Hot conditions.** A breakpoint's condition and a logpoint's message
  are compiled once per breakpoint, body and slot and kept on the
  breakpoint's record (`CompiledConditions`); still, every hit leaves the
  stretch for the safepoint and runs an evaluation, whose frame goes on
  interpreted, so a conditional breakpoint in a hot loop slows that loop:
  the body exits at its patched slot at every pass, as for any
  breakpoint (`a_condition_in_a_compiled_loop_is_decided`, ten thousand
  passes). A condition that raises an error stops at the breakpoint and
  reports the error, as IntelliJ does; a condition evaluated at the
  depth limit has the evaluation's reserve, as every evaluation does.
  While a condition runs, the stop is not made yet: the `Debugger` keeps
  the decision (`conditions::Deciding`) where it keeps a stop for an
  evaluation, the safepoints within it serve the commands queued
  meanwhile, a pause aborts it, and a pause asked after it ended stops
  the program at the next safepoint, the program not being stopped.
- **Aborting.** An evaluation the user cancels is aborted by
  `Command::AbortEvaluation`, or by the agent's pause bit
  (`DebugControl::pause`): the safepoint that serves the command --
  queued, it arms the commands bit as every command does -- or the pause,
  finding an evaluation running at the kept stop
  (`Interpreter::serve_evaluation`), marks it aborted
  (`evaluation::abort`), answers the command `Event::AbortTaken`, and
  answers `Served::Abort`, which the run loop takes as a termination --
  the debugger's termination signal unwound from the top activation
  (`vm::unwind_at_safepoint`) -- but for the termination flag, which it
  leaves clear, so that the evaluation's base ends the unwind: every
  `ensure:` block above the base runs, and the stop answers
  `Event::Evaluated(Evaluation::Aborted)`, whatever an `ensure:` block
  raised on the way, and serves on, the program still stopped. The unwind
  leaves the frame below at the slot it stood at, and the run loop takes
  the safepoint the evaluation's end asked for at once
  (`vm::take_safepoint`), as the switch to that frame does when the
  evaluation returns or an error ends it: were the frame to fetch its
  patched slot first, with no token to go past it, it would request a stop
  of its own, which the kept stop's serving leaves pending for a later
  safepoint -- a stop for the same breakpoint in whatever frame runs then
  (`a_continue_after_an_aborted_evaluation_runs_to_the_end`). That second
  safepoint is not at a loop head, whatever the first was: the backward
  jump that made the first was the evaluation's, and counted against the
  stopped frame's body it could compile the body and switch the frame into
  machine code mid-body
  (`an_evaluation_aborted_at_a_loop_head_counts_no_jump_of_the_stopped_frame`).
  A condition's evaluation is aborted the same way, and its stop is what
  the conditions' rules make of an aborted condition
  (`an_abort_evaluation_command_aborts_an_endless_condition`). The command
  is sequenced with the others, so that it never outlives the evaluation
  it was meant for: where none runs -- at a stop, the evaluation already
  answered, while the program runs, or after another `AbortEvaluation`
  drained at the same safepoint took the abort -- it is moot, refused with
  `"no evaluation runs"`, and leaves nothing armed
  (`an_abort_evaluation_command_with_none_running_is_refused`). A pause is
  a bit, unordered with the commands: one that comes after the evaluation
  ended is cleared by the safepoint that serves the kept stop, but one
  asked while that stop serves again is a pause asked while stopped, and
  stops the program at the next safepoint once it goes on ("The stop").
  The DAP agent therefore aborts an evaluation past its bound by the
  command, never by a pause. The remote's departure aborts it too: the
  last handle of a `DebugControl` closes the command channel and arms the
  commands bit as it drops, so that even an evaluation that never ends
  reaches a safepoint that finds the channel closed, detaches the debugger
  and aborts the evaluation, and the program runs on. A
  `Command::Terminate` while it runs ends the run as at any safepoint, the
  evaluation unanswered, its `ensure:` blocks and the program's run once
  on the way to the floor, the depth limit keeping the evaluation's raise
  until the run ended (`evaluation::terminated`), since those blocks are
  pushed where the evaluation's frames stood, which may lie past L
  (`a_terminate_of_an_evaluation_past_the_limit_runs_its_ensure_block`).
  Pinned by `an_endless_evaluation_is_aborted_by_a_pause`,
  `an_abort_evaluation_command_aborts_an_endless_evaluation`,
  `a_detach_during_an_evaluation_aborts_it` and
  `a_terminate_during_an_evaluation_ends_the_program`. A renderer past its
  limit will be stopped the same way.

### Assigning a captured variable

**Built** (`VariableEntry::copied_by`, `bytecode::may_have_run`,
`debugger::evaluation::assignabilities`).

Closure conversion copies a captured mutable local into the closure
unless something can write it once a block holds it
(`compiler/capture.rs`, `needs_vector`): a local a block writes, one
written conditionally or in a loop enclosing the capture, goes into the
indirection vector instead. Such a copy is read-only by
construction ([execution.md](execution.md), "Closure conversion"). An
assignment through evaluation -- in the home frame or in the block's --
would write one copy and leave the others stale, and the reference
lowering, which does not copy, would behave differently. A copy exists
only once its `MakeClosure` has run, though, and an assignment before
it reaches every copy the body will make. So the variable table records,
for each mutable local, the `MakeClosure`s that copy it, and at a stop a
forward search over the body's control flow from its entry answers
whether one of them may have run before the stop's location -- a loop's
back edge included, since a capture later in a loop's body may have run
in an earlier iteration; it reads the instructions as emitted, which a
breakpoint's patch leaves as they were. An assignment where one may have
run is refused with a message naming
the capture, as IntelliJ refuses a local a lambda captured; in a block's
own frame the variable is a copy, and is always refused (decided with
the user on 2026-11-16). A variable in the indirection vector, a register
no block captures, and a copied variable before its capture are
assigned.

The variable table (`debugger::metadata::VariableEntry::copied_by`,
held to the code by the metadata check) names, for each of a body's own
assignable locals, the `MakeClosure`s that copy it, and
`bytecode::may_have_run` answers whether one of them may have run
before a program counter. Where an evaluation is compiled, each variable
of the stopped frame's table is decided by its place
(`debugger::evaluation::assignabilities`) and handed to the resolver as
`resolver::Assignability`: a `Captured` one is what an assignment of its
name names -- it declares a setter, unlike a parameter, which the
assignment passes by -- and the assignment is refused with "<name> is
captured by a block made before this point", in an `Evaluate` and a
`SetVariable` alike. A register of the frame's own body is refused once
one of its `MakeClosure`s may have run before the frame's stored program
counter; a register of the frame that holds an enclosing variable is a
copy the frame was handed, and refused, as is an enclosing frame's
register short of the declaring frame (`Outer` with fewer hops than the
variable's depth), a copy a block between was handed; the declaring
frame's own register (`Outer` as far as the depth) is decided by that
frame's own entry for it at its program counter in its body
(`debugger::stack::outer_frame`), refused whenever a block copies it once
that frame returned, and refused, too, where neither the frame nor its
entry is found; a vector variable is assigned. Pinned by
`a_copied_variable_is_refused_after_its_capture`,
`a_copied_variable_is_assigned_before_its_capture`,
`a_copy_in_a_block_s_frame_is_refused`,
`an_evaluated_assignment_to_a_captured_variable_is_refused`, and, for
the declaring frame's own register a conditional's fallback block reads,
`a_method_s_copied_variable_is_refused_from_its_fallback_block`,
`a_method_s_variable_is_set_from_its_fallback_block_before_its_capture`
and `a_returned_method_s_copied_variable_is_refused_from_its_fallback_block`.

The reference lowering copies nothing and refuses nothing, so
the lowerings differ in what the debugger allows, never in what an
assignment it allows makes the program do. Primordial Soup and the
Newspeak IDE assign the most local copy and leave the others stale
(`Newspeak2PrimordialSoupCompilation.ns:2084`,
`newspeak/ActivationMirrors.ns:344`), which would make the lowerings'
outputs differ. Moving the frame into a layout where the variable lives
in a vector, through edit-and-continue, would not reach a copy a block
already made, which names no frame when its `outer_reach` is 0.

### Evaluation never reaches the VM mirror

**Built** for the debugger's evaluation (`debugger::evaluation::compile`,
`Mutator::is_withheld`); `ActivationMirror`'s part is planned with it.

The VM mirror is a capability (`CLAUDE.md`, "The VM mirror is a
capability"). An expression evaluated in a shipped module's frame -- a
frame of `Kernel.ns` a step into reached -- would resolve `vmMirror` as
the frame's slot and could hand the mirror to the program, as its result
stored into a program object. So evaluation in a frame whose body belongs
to a shipped module is refused, before the body's metadata is compiled
again (`debugger::evaluation::compile`, "the frame is the core
library's"; `an_evaluation_in_a_shipped_frame_is_refused`); the IDE
shows such a frame's variables,
read by the agent and held as `Root`s, which never enter the program. A
value the IDE shows enters the program only through an expression the user
typed in a program frame, and an object reached through a shipped
activation or a shipped mixin's field can be neither marked nor named in
one ("Marking objects"). `ActivationMirror`, planned, withholds every
value of a shipped activation, as the mirrors withhold what they reach
through a shipped level.

An expression obeys access control as the frame's method does (decided
with the user on 2026-11-16): resolved as a block in the frame's scope,
its sends to `self`, `super`, `outer` and the implicit receiver reach
what the method's would, and an ordinary send to any other object finds
public methods only (§5.5, *lookupPublic*). No send ignores access:
access control is what keeps a program's sends from the private members
of the shipped objects it holds or reaches. The platform object every
program holds is a `Platform`, whose only fields are its public lazy
facets -- `kernel`, `transcript` and the others, which the variables view
shows withheld, a shipped mixin declaring them -- so `platform vmMirror`
is not understood; but each facet is built by the platform module's
instance, whose private slots hold the mirror (`vmMirror`) and the
kernel's instance (`kernelModule`), and the kernel's and the other
modules' classes keep private methods and slots of their own
(`PlatformFactory.ns`; `the_platform_s_mirror_field_is_withheld` stops in
the time facet's lazy slot and finds `vmMirror` a field of that frame's
receiver, the module instance). A send that ignored access would reach
every one of them that an expression could name, and a send
that ignored access for program classes alone would need a send kind of
its own in the instructions, the lookup, the caches and the JIT, and
would let an evaluation tell a proxy from its object, which §5.5's
protected `doesNotUnderstand:` rules out. A private slot of another
object is still seen: the variables view reads it through the agent,
never through the program. Primordial Soup and the Newspeak IDE compile
the expression in the frame's method's mixin for the same reason
(`primordialsoup/newspeak/Mirrors.ns:49-64`,
`newspeak/ActivationMirrors.ns:343-372`), and Soup's VM checks a public
lookup on an ordinary send (`vm/interpreter.cc:246`); IntelliJ ignores
access in Java, which has no capability to keep.

### Marking objects

**Built** in the VM (`Command::Mark`, `Command::Unmark`,
`debugger/values.rs`; `ValueDescription::withheld`,
`ValueDescription::mark`), in the agent (`nsNext/mark`,
`nsNext/unmark`; "Launching, the start and the end") and in the plugin
(Mark Object…; "The JetBrains plugin").

An object the IDE shows can be marked with a label and named in every
later evaluation of the session as `<label>_DebugLabel`, as IntelliJ's
Mark Object does (decided with the user on 2026-11-16). The names an
expression uses that are marked labels become parameters of the
expression's block, and the debugger passes the marked objects as its
arguments -- the way Primordial Soup's `evaluate:with:` binds the names
of its scope map (`primordialsoup/newspeak/Mirrors.ns:81-95`,
`Newspeak2PrimordialSoupCompilation.ns:2128-2134`); no instruction is
added. The suffix keeps a label from shadowing anything: a parameter
named by the bare label would hide a frame variable, a slot or an
implicit receiver send of the same name, while `label_DebugLabel` is an
identifier (§3, `id`) no program is expected to spell. A marked object
is held as a `Root` across stops, until it is unmarked or the attachment
ends, as a pinned value is ("Values the IDE holds"), and its label shows
wherever a stop describes it (`ValueDescription::mark`;
`a_mark_is_shown_on_the_value`,
`a_mark_survives_a_continue_and_is_named_in_an_evaluation`).

`Command::Mark` names a value the stop handed out and a label, an
identifier of letters, digits and `_` not starting with a digit that
does not end in `_DebugLabel`; any other is refused with "`<label>` is
not a label". Marking an object marked before renames it, and a label
another object had moves to the object marked now, so that a label
names one object and an object has one label; objects are told apart as
`==` tells them, so that a number's label shows on every value of that
number. `Command::Unmark` takes a label off, a configuration command
served while the program runs too. Both answer `Event::Marked`. The
marks are held on the `Mutator`, beside the breakpoints
(`Breakpoints::marks`), so that a condition decided while the program
runs reaches them, and go when the debugger detaches; each stop's value
table starts from them, describes by them and hands back what its
commands made of them (`values::StopValues::mark`,
`Breakpoints::set_marks`). An expression's identifier tokens that end
in `_DebugLabel` and whose label is marked, each once in the order the
text first names it, are its block's parameters
(`values::used_labels`, `EvaluationScope::labels`), but for a name its
own temporaries declare; one whose label no mark holds is an ordinary
name, resolved as any other. The block's arguments are the marked
objects (`evaluation::Compiled::marked`). A breakpoint's condition and
log expression name them as every evaluation does: each is compiled with
the marks as they stand and kept compiled with the objects it names, and
every change of the marks -- a label marked, unmarked or moved -- forgets
every compiled condition, so that the next hit compiles it again
(`a_condition_names_a_marked_object`,
`a_condition_follows_a_mark_that_changed`). A label inside a string
literal or a comment is no identifier and stays text
(`a_label_in_a_string_or_a_comment_is_text`).

A value whose way into the IDE passed through shipped code cannot be
marked, nor forced as a frame's return ("Restart frame and force
return"): both are refused with "the value is withheld". Every value a
stop hands out carries whether it is withheld (`ValueDescription::withheld`):
it is when it was reached through a shipped activation -- a shipped
frame's receiver and variables, and the value a call in a shipped frame
answered at a step's stop -- or through a field a shipped mixin declares
-- every field of the platform object, and the platform module's private
`vmMirror`, which a shipped frame of that module shows among its
receiver's fields, among them -- or is a value the shipped world
withholds from every program (`Mutator::is_withheld`: the VM mirror and
every recorded shipped module instance, the kernel's among them); a child
inherits its parent's withholding (`the_platform_s_mirror_field_is_withheld`).
An evaluation runs only in a program's frame, so what it answers is
withheld only where `Mutator::is_withheld` holds. A `String`, an
`Array` or a number a program frame shows is marked like any value: the
rule is the way a value came, not its class
(`a_string_of_a_program_frame_is_marked`). The mirrors withhold what they
reach through a shipped level by the same rule. An object refused on one
way can be marked where the program's own frames or fields show it. The
Newspeak IDE's workspace scope, which binds `_` in every evaluation to
an object with the platform and the IDE behind it
(`newspeak/squeak/WorkspaceManagerForSqueak.ns`, `workspaceScope`), is
the way not taken: it would put what the VM mirror guards within an
expression's reach.

### The memory view

**Built** in the VM (`Command::InstanceCounts`, `Command::Instances`,
`Command::Referrers`, `debugger/memory.rs`, `Heap::each_object`,
`Heap::refers_to`; tests `memory_view` in `tests/debugger.rs`) and in
the agent (`nsNext/instanceCounts`, `nsNext/instances`,
`nsNext/referrers`; "Launching, the start and the end") and in the
plugin, a panel of the session's tab and Show Referring Objects ("The
JetBrains plugin").

At a stop the IDE can ask for the instances of a class and for the
objects that refer to one (decided with the user on 2026-11-16, in stage
5), as IntelliJ's memory view asks JDWP's `InstanceCounts`, `Instances`
and `ReferringObjects`, and as the Newspeak IDE's workspace offers
`instancesOf:`, `pointersTo:` and `pathTo:`
(`newspeak/squeak/WorkspaceManagerForSqueak.ns`). Each is a walk of the
object space at the stop (`Heap::each_object`, over the past survivor
space, eden and the old space, forwarders and gaps skipped), where no
Newspeak code runs and so nothing collects: the walk finds every object
the space holds, those nothing reaches any more too, which only a
collection would tell apart. A class object exists per enclosing
instance in Newspeak ([object-model.md](object-model.md), "Three layers,
not one"), so the counts are grouped by mixin -- by declaration
(`Heap::class_declaration`), named as a step filter names one
(`ClassName`), a builtin class such as `String` under its declaration in
the kernel -- never by class object, and `Instances` lists a
declaration's instances grouped by application, in the order the walk
first meets each. A class side's instances, the class objects, are not
counted, since a `ClassName` names the instance side; nor is the VM
mirror, whose class has no declaration and so no name ("The VM mirror is
the whole Rust boundary" in [core-library.md](core-library.md)).

What refers to a value (`Command::Referrers`, answered by
`Event::Referrers`, each a `Referrer`) is, first, every variable of a
frame of the stop's stack trace that holds it -- a running one, or one
suspended into its frame object, which is a frame of the stack all the
same and never an object of its own
(`a_suspended_frame_refers_from_the_stack`) -- its receiver as
`self`, innermost frame first (`Referrer::Stack`, the frame's index and
the variable's name), and then every object a word of which that keeps
what it names alive names it (`Referrer::Object`), in the space's
order; the answer pages over the two together. A frame's variables are
read by the variables view's own reader (`stack::frame_values`;
"Reading the stack at a stop"), so the two views never disagree, no
frame is reified to be answered, and the answer is the same under both
lowerings: whether a block copied a value or captured the frame that
holds it changes only whether the closure is an object referrer. A
scratch register counts for none, nor does a register the leftover
analysis finds tainted: no variable names either, and either may hold
a word a callee or a popped frame left -- a stale handle, or a debug
build's poison -- which would answer a referrer the program no longer
has, or none at all ("What a callee leaves" in
[compiler.md](compiler.md)). An object refers through the strong
pointer words the collectors trace (`Heap::refers_to`), but neither
through its class word -- an instance names its class, which
`Instances` answers -- nor through a `WeakArray`'s slots or an
`Ephemeron`'s key, which keep nothing alive; an `Ephemeron`'s value
counts, held while its key lives. A closure refers to what it copied
and to the frame it captured, and a suspended or returned frame object
to its receiver, its sender and home, and to what its registers hold. A
frame object married to a running frame of the stack is that frame's
variables, never an object of its own; one married to an activation
below the run, which the stack does not show, is an object whose live
registers count, read as the variables view reads them. A returned
frame that kept its registers still refers to what they held, as the
garbage it may be: the stop collects nothing. But a returned frame
object that nothing living refers to is left out
(`unreferred_returned_frames`): a frame object is the
implementation's artefact -- reified for a program's block or for the
debugger's evaluation alike -- not the program's, and one nothing refers
to keeps nothing alive. Its referrers are decided by further walks of
the space, which allocate nothing on the heap: a closure or a returned
frame object that refers to it is followed in turn, as the evaluation's
own closure over the frame is, garbage once the evaluation ended, and
every other referrer -- an object of any other kind, a frame object
married to an activation, or a closure or frame object the stack
keeps: a running block's closure, held in its activation's header and
perhaps nowhere else, a suspended frame's object, and what a running
frame's variable holds -- counts as living, its liveness carried along
the references followed. So a frame an evaluation reified is no
referrer of its receiver once it returned
(`a_returned_frame_object_nothing_refers_to_is_no_referrer`), while one
a block stored in a slot keeps is listed, withheld as every frame
object is (`a_returned_frame_object_a_stored_block_keeps_is_a_referrer`).
Every other object nothing reaches is answered as the walk finds it.

The objects found are values a stop holds, as the variables view's are
("Values the IDE holds"), each named `#<n>` by its place in the answer,
and withheld ("Marking objects") -- shown, but never marked, so that no
walk puts a private kernel object or the mirror within an expression's
reach: an instance, when a shipped mixin declares its class, a class
object of a shipped declaration's class side among them, and an object
whose class has no declaration, which the view cannot place; a frame
object -- Kernel's private `Activation` -- always, whatever body it
runs, in every view (`StopValues::describe`), since no program ever
holds one ("a program sees the Error, never the activation", `Kernel.ns`)
and a program's way to activations is a mirror; a closure, when its body
belongs to a shipped module, decided by its code -- the registered mixin
whose member's family holds it (`stack::code_is_shipped`) -- rather than
by Kernel's `Closure` or the frame it captured. The VM mirror's class
and its metaclass, nameless by lock 4, have a placeholder mixin and a
class side for declarations, so the fail-closed rule does not reach
them: `Mutator::is_withheld` withholds both, in every view, as it
withholds the mirror.

### Folding the shipped frames

**Built:** every frame of the stack trace says whether it runs a body of
a shipped module (`FrameDescription::shipped`, `stack::is_shipped`;
`a_shipped_frame_says_so`); so do the agent's hints
(`stopped::stack_frame`; `a_shipped_frame_is_deemphasized`), and the
plugin folds them as library frames ("The JetBrains plugin").

The agent marks a frame of a shipped module's body with DAP's
`presentationHint` `subtle`, and its source with `deemphasize`, and the
plugin folds each run of such frames in the stack, as IntelliJ hides
library frames (decided with the user on 2026-11-16, in stage 5). Which
frames are shipped is the test the step filter already makes (decision
16). The agent also writes `nsNext/shipped: true` on such a frame, so
that a client tells it from a hidden frame, which is `subtle` too -- a
frame both hidden and shipped stays `subtle` -- and `canRestart:
false`, since the VM refuses to restart a frame of the core library.

### Exceptions

**Built** (`debugger/exceptions.rs`; `Command::SetExceptionBreakpoints`;
the mirror rows `exceptionUnhandled:ifFail:`, a leaf, and
`exceptionCaught:at:ifFail:`, an exit, `builtins/activation.rs`), but for
"Create Method" and the declined `MessageNotUnderstood`'s forced `nil`,
which are planned; the forced return they need is built ("Restart frame
and force return"). The handler search of `Kernel.ns` reports its outcome
through two mirror rows: `invokeNextHandler`, having found no handler,
sends `exceptionUnhandled:ifFail:` just before `defaultAction`, and
`invokeHandlerAt:disabling:` reads the handler it is about to send `cull:
self` through `exceptionCaught:at:ifFail:`, which answers the `on:do:`
activation's handler, the block its marker's register holds; only the
shipped modules reach the mirror ([core-library.md](core-library.md), "The
VM mirror is the whole Rust boundary"). With no debugger attached
`exceptionUnhandled:ifFail:` does nothing and answers what its failure
block would, false, and `exceptionCaught:at:ifFail:` answers the handler
and records nothing; with one attached the caught row answers the handler
all the same and records the catch beside it. A handler it cannot read --
the activation not running, or no `on:do:` marker's -- makes it record
nothing and decline, so that its failure block answers nil. So an
exception behaves as it does without the debugger; and
`Interpreter::resume_unhandled_exceptions`, the test hook that resumes an
unhandled exception, is served first, so that an exception it resumes
stops nothing; nor does an exception an `ensure:` block of the debugger's
termination raises, from the `Terminate` to the run's end, as the
termination ends a step. `exceptionUnhandled:ifFail:` is a leaf machine
code calls without exiting (`MachineCall::Leaf`), and
`exceptionCaught:at:ifFail:` the exit the search made already to read the
handler, which reads another activation and so is no leaf: a program
without a debugger pays no send of its own for either report. Beyond the
handler the caught row answers, each only decides and records, and only
with a debugger attached -- it reads the exception and its class chain,
holds the `on:do:` activation and the handler as `Root`s, and sets a bit
-- runs no Newspeak code and collects nothing. What it decides is recorded
on the `Mutator` (`Breakpoints::request_exception_stop`) and asked of the
next poll through the request bit `DEBUGGER_STOP` ("The poll words as
atomics"), which lowers the depth limit, so that the next push is the
interpreter's, or machine code's call of it refused, and its safepoint
serves it (`exceptions::serve`): there, where flooding is allowed, the
handler found is reported to a step in progress; and the stop waits,
recorded again, while the frame on top is still the search's -- the caller
of a refused compiled call stands at the send, before the push -- so that
an uncaught exception stops at `defaultAction`'s first instruction, a
`Halt` at `resume:`'s, and a caught one past the kernel's `cull:`, at the
handler block's first instruction, before it ran, or at the first frame of
a handler that is no block. The stop ends a step in progress and serves a
pause asked meanwhile, as every stop does; a termination and the run's end
drop a stop no poll served (`exceptions::drop_pending`). A stop of another
kind -- a step's, a breakpoint's, a pause's -- served while a caught
exception's stop waits for the handler's first instruction leaves that
stop waiting: once the program goes on, the exception's stop follows as a
second one, so that neither is lost.

A signal searches its handler before anything unwinds, so an uncaught
exception stops with its whole stack, where the search, having found no
handler, is about to send `defaultAction` (`invokeNextHandler`,
`Kernel.ns`), before any subclass's override of it runs:
`StopReason::Exception { class_name, description, caught: false, exits }`,
the description the exception's `messageText` as text, or its class's name
when it has none -- `Exception>>description`'s answer unless a class
overrides it, which only running code could ask. The frame shown first is
the first frame below the exception's own machinery -- below the kernel's
`signal` or `pass` that runs the search, then the instance side's and the
class side's `signal` and `signal:`, and `Object>>doesNotUnderstand:` for
a `MessageNotUnderstood` -- which is the raising frame itself only for a
VM error, whose `signal` is pushed right above it; with step filters
(decision 16) that is the first frame below it they do not filter, the
first frame of program code (`exceptions::first_shown`). The frames above
it are marked `FrameDescription::hidden`, never dropped, so that a frame's
index is the same at every stop, and a step taken at the stop steps from
the frame shown first (decision 16; `stepping::begin`). Going on lets
`defaultAction` run, which ends the program for every exception whose
class does not override it, as its `terminate…` leaf does without a
debugger, resumable ones included; a `Halt` and a declined
`MessageNotUnderstood` are handled as below. Whether a handler takes an
exception is decided by Newspeak code (`handles:`), so "caught" is known
only once the search found one: a caught-exception breakpoint, off by
default, stops where the search invokes the handler it found
(`invokeHandlerAt:disabling:`), before the handler runs, `caught: true`.
Execution stands in the handler, about to run it, so that stop shows the
handler block on top first, the exception's machinery between it and the
raising frame hidden -- the kernel's `cull:`,
`invokeHandlerAt:disabling:`, `invokeNextHandler`, `signal`, `signal:` or
`pass` -- and the frame `exceptions::first_shown` answers below the
machinery shown next (`exceptions::hidden`); a step taken there steps in
the handler block, as from any frame it stood in. Only an uncaught
exception's and a `Halt`'s stop step from the frame below the machinery --
and a caught one's whose handler pushes no frame: a handler whose `cull:`
is a slot's setter, or a method a fast path answers, runs without an
activation, so that its stop, made at the next poll, stands in the
kernel's machinery frame the program goes on in. That stop hides the
machinery as an uncaught one's does, showing first the frame below it, and
a step taken there steps from that frame (`exceptions::serve` answers
whether the frame on top is one the handler pushed: the handler block's
activation, or a method sent to the handler by the kernel's block in
`invokeHandlerAt:disabling:` that sends it `cull:` -- not one the search
sends it later, as the `return:` an exception that is its own handler
receives). A handler that does not understand `cull:` pushes
`Object>>doesNotUnderstand:` sent to it, a real call it made, which the
stop shows on top and a step steps in as in any kernel code. A class
filter -- `ClassName`s, as the step filters name a declaration -- is
decided in Rust by walking the exception's class chain by declaration,
each level's applied mixin and body-less declaration as `Class>>handles:`
reads them (`activation::inherits_any`), running no code; with no class
named, every exception is taken. `halt` signals `Halt`, whose breakpoint
is on by default under the debugger: an uncaught `Halt` stops
(`StopReason::Halt { exits }`), and going on resumes it with `nil`, which
`halt` answers -- the leaf answers true and `invokeNextHandler` sends
`resume: nil` instead of `defaultAction`. A `Halt` subclass therefore
behaves otherwise under that default breakpoint than without a debugger:
its `defaultAction` override is skipped, the `Halt` resumed with nil
instead, and one whose `isResumable` answers false makes that `resume:`
signal the kernel's "cannot resume a non-resumable exception" `Error`,
which, uncaught, stops in turn. With the `Halt` breakpoint off, a `Halt`
is an exception as any other. An uncaught `MessageNotUnderstood` will
offer "Create Method": the method is created public, since the send again
is a public send, and the exception is resumed, sending the message again
([core-library.md](core-library.md), "Exceptions"). Declined, the failing
send will answer `nil`: the debugger forces `Object>>doesNotUnderstand:`'s
frame to return `nil` -- the one forced return in a shipped frame the
refusal below allows. Resuming the exception instead, as
`resume_unhandled_exceptions` does, would send the message again
(`doesNotUnderstand:` sends it again once the exception reached its
default handler, `Kernel.ns`), signal the same `MessageNotUnderstood`
again, and stop again, without end. Until then, going on from it ends the
run.

A stop at the push of `defaultAction` needs a push: an override of
`defaultAction` that a method's fast path answers without an activation
-- a constant, a slot -- pushes nothing, and the program stops at the
next poll instead, the return to `signal`, the override's answer
already given; so does a caught exception whose handler's `cull:`
pushes no frame -- a slot's setter, a fast path. A leaf cannot leave its stretch, and a
check after every mirror send would cost the run loop.

A handler block runs on top of the stack before anything unwinds, so it is
never flooded by a step over. As Java's step over stops in the `catch`
that takes an exception thrown below, a step over -- or a step out, or a
step into -- whose exception a handler at or below the stepping frame
takes stops at the handler block's first statement: the search's
invocation of the handler is reported to the stepping state
(`stepping::handler_found`), as an unwind is, and the step goes on as a
step into from the frame on top, through the machinery the step filters
pass. So does a step whose frame left already, waiting for where the
error's handling takes the program (decision 17). A step whose frame
lies below the handler's `on:do:` -- the frame sent that `on:do:`
itself -- goes on as it was, past the handler, to its next statement.

### Restart frame and force return

**Built** (`Command::RestartFrame`, `Command::ForceReturn`;
`debugger/unwinding.rs`; `UnwindAction::RestartFrame`,
`UnwindAction::ForceReturn` in `interpreter/unwind.rs`), but for the
forced `nil` of a declined "Create Method" ("Shipped frames" below).

A stop serves either command by naming the frame -- by the frame object
of its activation, reified, a `FrameReference`, since the `ensure:`
blocks the unwind runs may reuse a popped activation's offset -- and,
for a forced return, the value, both rooted
(`unwinding::StartedUnwind`, held by the `Debugger`); it begins the step
that makes the next stop and answers `Continued`, and the safepoint
answers `SafepointOutcome::Unwind` for its `Served::Unwind`, the rest of
the safepoint undone, as for a termination. The run loop
(`vm::unwind_at_safepoint`) raises the unwind in the top activation as a
`Signal::Unwind`, the pending unwind itself, and hands it to the one
unwinder, which runs every `ensure:` and `ifCurtailed:` block above the
frame, each raising it again as it was once it returns
(`unwind::signal_of`), and arrives at the frame whatever it runs
(`arrives_at_any_frame`, tested after `catches_return`, so that a
program's `^` and `retry` arrive where they did): a block, an
initializer, a factory, a superclass clause, the top level. The frame
the loop goes on with runs interpreted -- the restarted frame, and a
caller a forced return resumed, whose machine code it leaves as a caller
resumed for a safepoint does (`Heap::clear_compiled`). A command that is
refused leaves the program stopped where it was, nothing begun.

- **The next stop.** A restart patches a one-shot for the frame, before
  the unwind (`stepping::stop_at_restart`), where a step into stops a
  pushed frame (`stepping::first_stop`): its first statement start --
  index 1 past the vector it keeps -- a factory's first instruction, its
  entry, the first slot of an initializer whose declaration writes no
  superclass clause, past the implicit `Object new`, and index 0 or 1 of
  a body without metadata, an image's without source; the stop is a
  step's (`StopReason::Step`), whatever the step filters say. A flood
  would patch only the statement starts the metadata names -- none for a
  body without it -- and the implicit clause too. A forced return
  begins a step out of the frame (`stepping::begin`), so that the stop
  is where its sender goes on, reporting the value it returned, or where
  a superclass clause's error is handled.
- **Restart** arrives at the frame and resets it (`restart_frame`): its
  `program_counter` goes back to 0, as `retry`'s `Restarted` outcome sets
  it, and the registers it keeps are the arguments, the header and a
  block's copied values, while it nils the named temporaries between the
  copied values and `local_count`; the scratch registers are tainted at
  index 0 anyway. A block's copied values stay (Primordial Soup's restart,
  `primordialsoup/newspeak/Mirrors.ns:2325-2353`). Under the optimized
  lowering a body with vector variables makes its indirection vector at
  index 0, outside every statement's range; a restart there would make a
  fresh vector, and blocks made in the first run would keep the old one
  and see neither the reset nor any later assignment, while under the
  reference lowering, whose blocks read the frame's registers, they see
  both. So a restart reuses the frame's vector (decided with the user on
  2026-11-16): it nils the vector's elements -- a vector variable's own
  register is always `nil`, so nilling the named temporaries alone would
  not reset it -- and restarts at index 1, past `MakeVector`, and the
  first run's blocks see the reset and every later assignment, as the
  reference lowering's do; a frame standing at index 0, stopped at its
  push, has made no vector yet, and makes it as it starts again. What
  remains between the lowerings is a variable closure conversion copied,
  one nothing writes once a block holds it: a first-run block keeps its
  copy, where the reference lowering's reads the register the restart nils
  and the second run assigns again, so the two differ only for such a
  block run between the restart and that assignment, or where the second
  run assigns another value. Pharo's `Context>>restart` and Primordial
  Soup's restart (`Mirrors.ns:2341-2353`) start before the bytecode that
  makes the vector, and so disconnect the first run's blocks from every
  variable; neither has a second lowering to agree with. Updating the
  copies instead is not possible: a block of `outer_reach` 0 holds nothing
  that names the frame it came from. Every program frame can be restarted
  (decided with the user on 2026-11-16): an initializer's, a slot
  initializer's within it included, runs its prologue, its superclass's
  initialization and its slot initializers again, overwriting the slots; a
  superclass clause's evaluates the clause again, its `NestedClass`
  continuation waiting for the value as before; a factory's allocates a
  fresh instance, an instance of the first run that escaped staying as far
  initialized as it got. Each runs its side effects again, as every
  restart does.
- **Force return** arrives at the frame and returns the value to its
  sender through the ordinary return path (`UnwindOutcome::Returned`),
  defined for every program frame (decided with the user on
  2026-11-16), by its continuation: `Deliver` hands the value to the send,
  a factory's included, so the value becomes the factory send's; a
  superclass clause's `NestedClass` takes the value as the superclass,
  and `after_clause` signals for a value that is no class what it
  signals for a clause that answered one; an `ensure:` or `ifCurtailed:`
  block the unwinder started (`Unwind`) has its value dropped and the
  unwind carried on, as when it ends. A `ReturnToSuspended` wraps
  another continuation, so force return looks through it to the
  continuation it wraps and decides by that one. An initializer's frame
  takes no value: it answers `self`, as its end does, and the slots it
  has not initialized stay `nil` -- the state an initializer that raised
  an error leaves an instance in, should its `self` have escaped. The
  other continuations are shipped code's: `VmErrorSignalled` is the
  kernel's `signal`, `Reexecute` its `resolve`, and the unwind markers
  `on:do:`, `ensure:` and `ifCurtailed:` are kernel methods, so the
  shipped frames' rule below refuses them. Primordial Soup restarts and
  returns from every activation, initializers included, refusing only to
  cross an unwind-protect activation
  (`primordialsoup/newspeak/Mirrors.ns:2323-2340`, `:2373-2387`,
  `:2467-2473`), which the unwinder here runs the blocks of; JDWP's
  `PopFrames` and `ForceEarlyReturn` accept a constructor's frame.
- **Shipped frames.** Setting a variable, restarting and forcing a return
  are refused in a shipped module's frame, as evaluation is: forcing
  `signal` or an `ensure:` marker to return would break the kernel's own
  invariants. A frame whose body belongs to a shipped module is refused
  with "the frame is the core library's", decided from its declaration's
  module before anything is begun. The one exception, planned with
  "Create Method", is the forced `nil` from
  `Object>>doesNotUnderstand:` when "Create Method" is declined
  ("Exceptions"), which the debugger will issue itself.
- **The value** is one the stop handed out -- an evaluation's answer, a
  variable's or a child's value -- named by its id (`ValueId`), and an
  id the stop did not hand out is refused with "no value of this stop
  has that id". A withheld value ("Marking objects"), which the frame's
  sender would hand the program, is refused with "the value is withheld"
  (`a_withheld_value_is_not_forced_as_a_return`).

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
  a step into from the frame the unwind stops in -- the step waits, its
  frame `Holder::Left`, while the unwind pops on -- or a step out from
  the method's frame the unwind arrives at, which stops at its caller's return point.
  That step replaces any step in progress, carrying its value of the
  last call, and its stop reports the exit; so does an exception's stop
  served before it, the place the program reached first, in its
  `exits` beside the exception (`StopReason::Exception`,
  `StopReason::Halt`), whose reason stays the exception's, where a
  pause's gives way to `MethodExit`. A stop at an exit at a
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
  reported unasked by `Event::MethodBreakpoints`. It is set on the
  method in every built module of its module's name, as a line
  breakpoint resolves in each (`breakpoints::find_methods`); an install
  sets it on the method's new body, and the body it replaced keeps its
  patches for the frames still running it. A method with a fast path
  loses it while the breakpoint is set ("A breakpoint in a method with a
  fast path").

### Smart step into a block

**Built** (`stepping::begin_smart`). IntelliJ offers a lambda on the
line as a target. Here a block argument's body is pushed from inside its
callee -- `do:`'s loop, not the chosen `CallSite` -- so a block target
is a one-shot breakpoint at the block body's first statement start
(index 0, or 1 past a `MakeVector`), held for the stepping frame, which
`stepping::takes` takes in every activation of a closure whose captured
frames lead to it -- every activation of that block created by the
stepping frame -- rather than a push from a call site. The step is
otherwise a step over, so that a block never run stops at the next
statement start or the return point, and its stop in the block reports
no value of a call, as a step into's push does
(`stepping::in_aimed_block`). A block the compiler lowers inline is no
target -- every block of a form the optimized lowering inlines, under
both lowerings, so that both offer the same: it runs in the frame
itself, where a step over stops in it.

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
that skipped the call -- is not read. No jump joins after a send with a
value another way wrote: an inlined conditional's ways each end in a
`Move` to its value register, so their join follows that `Move`, and the
only send a jump joins after is an arithmetic or comparison send's
fallback, whose inline fast path jumps past it with that same send's
answer in its destination. The step's stop reports the last
value recorded as `Stopped::returned`, the stop's first value; a stop in
a frame a step into pushed, and every stop that is not a step's, report
none.

The value is named by the call that answered it. Where IntelliJ names
Java's by the method's class and name (`MethodReturnValueDescriptorImpl`),
this names it by the send in the caller, because a call is always the
instruction before the one-shot's index in the frame it answered into,
so that a step over's value and a step out's -- the callee's answer, at
its return point in the caller -- are named alike, by the expression the
user reads on the line the step stopped at, which tells two sends of one
method on that line apart where the callee's `Class>>selector` would
not. A setter assignment's send records nothing: its answer is the
receiver, never the value the assignment answers, so the flood leaves
out the index after it (`SendEntry::setter`) and the assigned value's
call stays the last -- `count:: count + 1` reports `count + 1`, and an
assignment of a literal reports none
(`a_setter_s_answer_is_not_the_returned_value`). A step out that returns
into one -- out of a setter method the program declares, or out of a
block such an assignment handed its method -- records nothing either,
its return point a one-shot that stops without recording
(`stepping::answers_a_setter`), so that the value before stays: none,
for a step out, which records nothing in the callee
(`a_step_out_into_a_setter_assignment_records_nothing`).

The one-shot notes only where the call stands, beside the value: its
body, its index, the kind of call the instruction is (`stepping::Call`,
decided by the one match that decides what a call is, `call_of`) and the
declaration the body belongs to, taken again from the previous call of
the same body (`stepping::RecordedCall`) -- no handle, so that it
outlives a frame a step over its last statement leaves. The stop names
it, once, rather than every call a step over a loop runs past
(`stack::call_name`): through the body's metadata, whose send entry
records the call's span (`SendEntry::call`, from the receiver -- or the
selector, where the receiver is implicit -- to the last argument; a
setter assignment's whole assignment), sliced from the text that
metadata was compiled from, which the metadata cache keeps beside it,
one text per module that every level of it compiled again shares
(`Cache::module_text`), and clears with it at every install
(`Cache::metadata_with_text`;
`the_levels_of_a_module_share_its_text`), so
that no text is assembled at a stop and a module is never looked up by
a name two loaded modules may share
(`a_call_is_named_from_its_own_module_s_text`). A cascade's later
message, which the source spells apart from its receiver, is named by
the receiver's text and its own, the receiver in parentheses where it is
a binary or keyword send written without them
(`SendEntry::parenthesize_receiver`): `platform transcript newLine`,
`(3 + 4) min: 9`
(`a_cascade_s_message_is_named_by_its_receiver_and_itself`). Its
comments are left out -- the text of a send starts and ends at a token,
so it lexes alone as in its module
(`a_comment_inside_a_call_is_not_in_its_name`) -- and the name is put on
one line, each run of whitespace one space; a call longer than 60
characters keeps its start and its end, each cut at a space, its middle
left out as ` … `: `(kinds at: which) value`. A body without metadata
-- an image built with `--strip-source` -- names it by the instruction
(`Call::name`): a send's selector, `super` before a super send's, a
mirror send's mirror selector, and `future`, `initializer`, `superclass
initializer` or `mixin initializer` for the other calls; so does a body
with metadata for a call the source spells as no send, the superclass
clause's initializer a step out of a superclass's initializer returns to
(`a_stripped_image_names_the_return_value_by_its_selector`,
`a_stripped_image_names_a_super_send_and_a_superclass_initializer`,
`a_step_out_of_a_superclass_initializer_names_its_kind`).

### Launching, the start and the end

**Built** (`crates/ns-cli/src/debug/`).

Decided with the user on 2026-11-14: the agent opens its connection in
either direction, as JDWP's `server=y` and `server=n` do
(`jdk/src/jdk.jdwp.agent/share/native/libjdwp/debugInit.c:1197`, `:1246`).
`ns-next run --debug` listens: it binds 127.0.0.1 on a port the system
chooses and prints the port and a token on the standard error stream
before the program writes anything, for a program started in a terminal
and for any DAP client. `ns-next run --debug-connect=<address>` connects
to a client that listens; the plugin always starts its programs this way,
as IntelliJ starts a JVM with `server=n` and an address it listens at, so
no port of the program is open to another process, nothing is read out of
the program's standard error, and the plugin checks that the `process`
event names the process it started: its `systemProcessId`, and
`nsNextSecret`, a field of ns-next's own, in which the connecting mode
returns the one-time secret its launcher handed it in the variable
`NS_NEXT_DEBUG_SECRET` -- a process id is no secret, and another process
that connected to the plugin's port first could name it. The connecting
mode sends none when it was given none, and the listening mode, whose
client presents a token, never sends one. `ns-next load` takes both
options too: a program loaded from an image shows its modules through
`source` requests on the image's text, and an image built with
`--strip-source` shows frames without lines, a line breakpoint in it
unverified.

The token guards the listening socket, since a client that connects can
run code in the program through evaluation, as the user who started it,
and on a machine with several users 127.0.0.1 reaches every one of them:
JDWP checks nothing but a fixed handshake (`socketTransport.c:186`) and
an `allow` list of addresses. `--debug` takes 128 bits from the
operating system's random source (`ns-memory`, through the `windows-sys`
and `libc` it already depends on) and the client presents them as the
`token` argument of `attach`; before it the agent answers `initialize`
only, and a missing or wrong token is answered by an error, the
connection closed and the next one awaited. The connecting mode needs no
token.

Either way the agent waits for the IDE's `configurationDone` before
`main:args:` runs, so breakpoints are set first, as JDWP's `suspend=y`
does; `stopOnEntry` stops at the first statement of `main:args:`. The
agent accepts one connection at a time. `terminate` ends the program as
its own `terminate…` would, its `ensure:` blocks run, served at the next
safepoint; `disconnect` removes every breakpoint and lets the program run
on, and `disconnect` with `terminateDebuggee` ends the process at once
from the agent's thread, with a terminated program's exit status and no
`ensure:` block run, for a program that reaches no safepoint -- unless the
program ended already, whose process then ends with its own status. The
plugin's Stop sends `terminate`, and a second Stop kills the process, as
IntelliJ's `KillableProcessHandler` does for a JVM; no timer in `ns-next`
decides it (decided with the user on 2026-11-14). A program that ends --
while running, while stopped, or in the middle of a step -- sends `exited`
with its exit status. The program's standard streams stay its own: when
the IDE launched the process it shows them itself, and the agent sends
`output` events only for logpoints and messages of its own, so that no
line is shown twice.

Built (`crates/ns-cli/src/debug/`; its tests
`crates/ns-cli/tests/debug_adapter.rs`, through the test client
`crates/ns-cli/tests/common/dap.rs`): the framing (`wire.rs`, bounding a
header and a message's length before it reads them, since the listening
socket answers whoever connects: a message of at most 64 KiB from a peer
that has not attached, which is far above an `initialize` or an `attach`,
and of at most 64 MiB once it has, the bound decided as each header is
read; `an_unattached_peer_s_long_frame_closes_its_connection`), the
connection in either direction with the token compared in constant time
(`connection.rs`), the agent's thread (`agent.rs`), which every input
reaches on one channel -- a connection, a request a reader thread read, a
connection's end, an event a pump thread took off `DebugEvents`, a word of
the host -- so that it alone decides what the client is sent and in which
order, and the host's link (`host.rs`). The session: `initialize` (its
capabilities in `protocol.rs`), `attach` (the token, `stopOnEntry`), the
`process` and `initialized` events, the client's configuration, then
`configurationDone`, which starts the host; `pause`, `continue`,
`terminate` and `disconnect`. A stop (`stopped.rs`, `sources.rs`) shows
one thread, `main`; its frames, each `id` its index plus 1, placed at the
selector of the send it waits on or else its statement, in lines and
UTF-16 columns of its module's current text, which the agent asks of the
VM (`Command::ModuleText`) only for the frames a `stackTrace` page shows
and keeps by revision, so that a later stop asks only whether it changed
-- a shipped module's text is the binary's own
(`ns_runtime::shipped_module_source`), never asked; a frame without a
location shows no source and line 0. A program module whose current text
is its file's as loaded is shown by the file's path, a shipped module by
the path of its file in the core library the client handed over at
`attach`, and every other module -- a shipped one without that library,
an image's, one an install changed -- by a `sourceReference`, stable per
module for the session, whose `source` request answers the text.

**The agent's sources.** The debugging API names every module by its id
and, where only one version is meant, its version (`ModuleName`:
`FrameLocation::module`, `SetBreakpoints`, `RunToPosition`,
`ModuleText`, `Event::Breakpoints`, `ClassName`;
`docs/design/modules.md`, decision 8): a root's identity
(`Mixin::module_identity`), or its class's name for a root a host built
without one. Within a program an id and a version name one module, so a
name with a version names that module and one without names every
version of its id, the VM resolving a method breakpoint, an exception
filter and a step filter in the roots the name covers only
(`ModuleName::names`, `ModuleName::covers`, `breakpoints::module_roots`;
`a_breakpoint_with_a_version_stands_in_that_version_only`,
`a_breakpoint_without_a_version_stands_in_every_version`,
`a_versioned_method_breakpoint_stands_across_an_install`). A line
breakpoint and a module's text are one module's, named exactly as a
frame's location names it: a module's line breakpoints are held and
resolved again at its build under its exact name, and `ModuleText`
answers no text for a name without a version that covers built
modules of several versions (`a_text_is_asked_for_by_one_version`). A shipped
module is a module of its own, whatever the program's names: a program
may declare a `Kernel` of its own, which is `Kernel`, while the binary's
is `newspeak.core.Kernel`, an id no program module may have (decision
9). Whether a module is shipped -- a frame folded, its values withheld,
an evaluation, a restart or a forced return refused in it, the shipped
step filter -- is never read from its id: the VM decides it by the
module's root (`Mixin::shipped`, `metadata::again::module_is_shipped`),
since it is a capability, and tells the agent (`FrameDescription::shipped`,
`InstanceCount::shipped`). The agent names
a shipped module by its file's absolute path, never by a name or a marker
(decided with the user on 2026-11-15): a client hands over the directory
of shipped sources it shows as `attach`'s `coreLibrary`, absolute, which
the agent takes only when the directory's `fingerprint` file is its own
`shipped_sources_fingerprint()` -- else it says so in an `output` event
and shows its shipped modules by reference -- and then maps
`<directory>/<path>`, each path `shipped_source_files` records, to the
shipped module, both ways, paths compared as a program file's are: a
`setBreakpoints` source of that path names the shipped module only, and a
shipped frame's `source` carries that path and no `sourceReference`. The
library's `Manifest.ns` names the program's embedded manifest,
`newspeak.core.Manifest`, a program module. A program module is named by
its own file's path; a name, `<Class>.ns`, names only a program module
that has no file -- an image's, the embedded manifest -- or one a stop
showed by reference that is not shipped, and only while no other such
module has that name, since two versions of a module share it and are
told apart by their references; never a shipped module, so that a
program module named `Kernel` never takes the library's breakpoints and a
shipped frame never shows the program's text
(`a_program_module_named_kernel_never_takes_the_shipped_kernel_s_breakpoints`,
`a_shipped_module_is_never_named_by_its_name`,
`a_core_library_of_another_fingerprint_is_ignored`,
`a_program_root_named_kernel_is_another_module_than_the_shipped_kernel`,
`two_versions_of_a_file_less_module_are_not_named_by_their_name`). A
shipped module's text the agent takes from the binary by its id
(`shipped_module_named_by_id`).

A class name a client types -- a function breakpoint's, an exception
filter's condition, the memory view's -- is written
`<id>[@<version>][/<Class>(.<Class>)*]`, a method
`<class>>><selector>` or `<class> class>><selector>`
(`breakpoints::parse_class_name`, `parse_method_name`):
`com.acme.json.Json@2.1.0/Parser>>parse:`, `Demo/Account>>deposit:`,
`Calls>>>>` (selector `>>`), `newspeak.core.Kernel/Error`. Each names
one module, or every version of one id, so a filter sends the VM one
`ClassName` and a function breakpoint one method breakpoint behind its
DAP id. Every module of the program and of the binary is known before
the program starts, so an id none of them has is refused, ``no module
`org.example.Tool` ``, rather than left to wait, unverified, for a module
that never comes, and so is a version no module of the id has, naming
the versions there are, ``no module `com.acme.json.Json@9.9.9`; its
versions are 2.1.0, 2.1.0-beta.1 `` (`a_version_no_module_of_the_id_has_is_refused`).
A module the binary has but builds only on first demand is no such
module: a breakpoint in it stays pending until the build
(`a_function_breakpoint_in_a_module_not_built_yet_stays_pending`).
The refusal says the form meant where it can
(`Sources::module_of`): a dotted module a prefix of which names one --
by its id, or, undotted, by the class's name of the one module it ends
in -- ``no module `Demo.Account`; a class inside a module is written
`Demo/Account` ``, since it is how a nested class was written before
ids; and an id that is the class's name of one module, ``no module
`Kernel`; a class inside a module is written `newspeak.core.Kernel/Array` ``
in a program without a module `Kernel`
(`a_dotted_nested_class_is_refused_with_the_new_form`,
`an_id_no_module_has_is_refused`,
`a_function_breakpoint_names_the_program_s_kernel_and_the_shipped_one_apart`,
`an_exception_filter_reaches_the_shipped_class_of_a_shadowed_module_name`,
`the_stack_trace_names_each_version_of_a_module`).
A step filter types no name: it names its module by a file, as a
breakpoint does (`nsNext/setStepFilters`, under "The steps" below).
`scopes` gives a frame's `Locals` and, at a
step's stop, the top frame's `Return value`, `presentationHint`
`returnValue`, whose one variable is named by the call that returned it
(`ReturnedValue::call`); `variables` pages a frame's
variables and a value's children, as indexed ones where the VM says so
(`ValueDescription::indexed`, by the object's shape); every
`variablesReference` is valid until the program goes on, and the ids
continue across stops, so that a stale one is refused rather than naming
another value.

The breakpoints (`breakpoints.rs`): `setBreakpoints` sets each at the
start of its line or, with a `column`, at that place of it, a block
variant, in UTF-16 columns of the module's text as the agent knows it -- a
program module's file as loaded until a stop read a newer current text, a
shipped module's own, before any stop showed it too -- the module named by
a source's path, its reference, or, for a program module without a file,
its name `<Class>.ns` ("The agent's sources"), and a source naming none
answered unverified, "not a module of this program". A path
names a module's file when both resolve to one canonical path, every `..`
and symbolic link resolved, since the two are spelled independently: a
project file names `../shared/Tree.ns` as written, a program may be
started through a link, and an IDE names the file it resolved
(`a_module_named_through_dot_dot_takes_a_breakpoint_by_its_plain_path`,
`a_program_started_through_a_link_takes_a_breakpoint_by_its_real_path`); a
path that does not resolve is compared with its `.` and `..` resolved by
their spelling, and on Windows every path without the verbatim prefix
`\\?\` a canonical one has (`\\?\UNC\` read as `\\`), so that a module's
file deleted after the
program read it is still named by its path. A breakpoint is verified once
the VM resolved it to a statement, whose line and column the response
gives, so that one on a comment line shows on the next statement.
`setFunctionBreakpoints` names a method
`<id>[@<version>][/<Class>(.<Class>)*]>>selector`, or `... class>>selector`
for the class side's ("The agent's sources"); a line or function
breakpoint takes a hit condition ("Hit counts and temporary
breakpoints"). `setExceptionBreakpoints` turns `uncaught`, `caught` and
`halt` on by its `filters` or `filterOptions`, and the conditions of its
filter options, each a comma-separated list of classes
`<id>[@<version>][/<Class>(.<Class>)*]`, each parsed as a function
breakpoint's class is (`parse_class_name`), together name the
classes both exception filters stop for, since the VM keeps one list
("Exceptions"). A breakpoint's DAP id is the agent's own, kept for as long
as the client sends the breakpoint again -- a line breakpoint by its
module and offset, a function breakpoint by its name -- since the VM sets
a set afresh under new ids at every replacement, which an IDE sends at
every change; the VM's ids of each set's latest answer map to them, and a
stop's `hitBreakpointIds` name only ids the client's latest request for
their set holds, never one it replaced since the VM decided the stop:
before it reports a stop, the agent serves every `setBreakpoints` and
`setFunctionBreakpoints` of the attached client that has reached it, the
client having sent it before it could see the stop, and keeps every other
input's order -- a peer's that has not attached among them, which would
otherwise overtake its own `attach`; one still on the wire then crosses
the stop, as DAP allows
(`a_stop_naming_a_replaced_breakpoint_is_reported_without_it`,
`a_pending_replacement_hides_the_ids_it_replaces`). An
`Event::Breakpoints` or `Event::MethodBreakpoints` the VM sends unasked --
a pending breakpoint resolved as its module was built, an install that
moved one -- is told from an answer by its ids: the VM numbers every
breakpoint from one counter and an answer sets its whole set afresh, so an
answer's ids are all new and an unasked event's all named by an earlier
answer, which offsets could not tell apart from a replacement of the same
offsets. It is reported as `breakpoint` events, `reason: "changed"`, and
dropped while a replacement of its set is on its way, which removes what
it names (`a_pending_breakpoint_is_reported_once_its_module_is_built`). A
module an install changed is counted in its new text only once a stop read
it: a `setBreakpoints` sent between the install and that stop counts its
lines in the older text, and the VM resolves those offsets in the current
one. A line breakpoint takes a `condition` and a `logMessage`, a function
breakpoint a `condition`, passed to the VM as written; a blank one is
none. A log message's text is logged as it is, `{{` and `}}` a brace of
it, and each expression in braces evaluated at the hit, its own braces
-- a tuple's -- nesting and a brace in its strings its own, so that one
beginning with a tuple is written after a blank, `{ {1. 2} size}`; a
brace that never closes, or an empty `{}`, leaves the breakpoint
unverified, its message saying so (`parse_log_message`); a comment's
quotes and braces are its own. Each line a logpoint logs
(`Event::Logged`) is an `output` event, category `console`, cut at
`LOG_TEXT_LIMIT`, 10,000 characters, saying so. Those lines are the one
message the agent drops: a logpoint in a hot loop logs faster than a
client reads, and a full write queue gives the client up as one that
stopped reading, so past `DROPPABLE_LIMIT`, three quarters of the queue,
a line is counted instead, and once there is room one `output` line says
`<n> log lines dropped`, the rest of the queue kept for the responses
and the events of the session's state
(`a_logpoint_flood_drops_lines_and_keeps_the_session`). A condition or
log expression that failed stops as the
breakpoint, its error the `stopped` event's `text`
(`a_conditional_breakpoint_stops_only_where_it_holds`,
`a_failing_condition_stops_with_its_error`,
`a_logpoint_writes_an_output_event_and_does_not_stop`).

Evaluation (`evaluation.rs`): `evaluate` runs its expression in the frame
its `frameId` names, the top frame without one, as `Command::Evaluate`,
and answers its value as a variable shows one; an exception that ended it
is a failed response whose message describes it, an abort one of the
message "evaluation aborted". Every `context` is evaluated alike, but a
hover's, asked unbidden as the pointer rests on a name, is aborted when it
has not answered within `HOVER_BOUND`, two seconds, which the agent's loop
keeps beside the attach deadline, by `Command::AbortEvaluation`, never a
pause: the command is served in sequence with the evaluation, so one that
arrives after the answer is refused as moot and dropped, where a pause bit
would stay armed at the stop and stop the program again once it goes on
(`a_hover_evaluation_that_does_not_answer_is_aborted`,
`a_hover_answered_around_its_bound_stops_nothing_later`). The agent sends
the VM one evaluation at a time -- an `evaluate`'s, a `setVariable`'s or
an `nsNext/forceReturn`'s -- and holds every such request that arrives
meanwhile until it answered, then serves the held ones in order, each as
if it arrived then: the VM refuses a command that arrives while an
evaluation runs, and its refusal, named by the command alone, would answer
the running evaluation's request, whose value would then answer the
other's -- a forced return would force an unrelated value
(`a_forced_return_sent_during_an_evaluation_forces_its_own_value`). DAP's
`cancel` (`supportsCancelRequest`) naming the `evaluate` the VM runs
aborts it the same way, by `Command::AbortEvaluation`, the abort the MCP
server sends past its wait ("The MCP server"); one naming a request still
held is dropped and answered with the message "cancelled", never sent,
since the abort would stop the running one; a cancel of another request,
or of one answered already, is answered with success and does nothing
(`a_cancelled_evaluation_is_aborted`,
`a_cancel_aborts_only_the_running_evaluation`). `setVariable` sets a
variable of the frame whose `Locals` scope its reference names
(`Command::SetVariable`), refusing a value's child and the return value:
"only a frame's variables are set". Every value the agent shows that is
not withheld carries, beside its `variablesReference`, a value reference
of ns-next's own, `nsNext/valueReference`, numbered from the same count,
one per value the stop handed out and valid until the program goes on, by
which `nsNext/mark { valueReference, label }` marks it and
`nsNext/referrers` asks what refers to it; a withheld value has none and
is shown read-only (`presentationHint.attributes` `readOnly`), and a
marked one's text ends in ` [label]`. `nsNext/unmark { label }` takes a
mark off (`a_marked_object_is_named_in_a_later_evaluate`,
`a_withheld_value_is_read_only`).

Smart step into, restart and force return (`stepping.rs`,
`evaluation.rs`): `stepInTargets { frameId }` answers the targets of
the frame a step steps from -- the top frame, or at an exception's stop
the first one shown below its hidden machinery -- and refuses any other
frame: "smart step into takes the top frame". It waits for the stop's
frames, which say which frame that is, and for that frame's module's
text, in which each target's span becomes its `line`, `column`,
`endLine` and `endColumn`, 1-based, in UTF-16 columns
(`Command::StepInTargets`); a target's id is the VM's plus 1, so that
none is 0. `stepIn` with a `targetId` sends `Command::StepIntoTarget`
whatever its `granularity`, but only for a target `stepInTargets`
handed out at this stop: the VM's ids are positions, by which an
earlier stop's id would name another target, so any other id is
refused, "no such target", and nothing is sent
(`step_in_targets_list_the_line_s_sends`,
`step_in_with_a_target_stops_in_it`,
`a_step_in_target_of_an_earlier_stop_is_refused`). `restartFrame { frameId }` sends
`Command::RestartFrame`, and the stop the restarted frame makes at its
start is reported with DAP's reason `restart`
(`a_restart_frame_runs_the_method_again`). `nsNext/forceReturn {
frameId, expression? }`, a request of ns-next's own, makes the frame
return at once: `nil` without an expression, or with a blank one;
otherwise the expression is evaluated in that frame first, as
`evaluate` evaluates it, and its value forced (`Command::ForceReturn`).
An evaluation that ends with an error or is aborted fails the request
and forces nothing, the program still stopped, and so does a withheld
value, which the agent sends and the VM refuses in its own words, "the
value is withheld"; while the expression runs the stop counts as left,
as while a step is on its way, so that a `continue` or a step the
client sends meanwhile is refused rather than cross the forced return
(`a_forced_return_answers_the_expression_s_value`,
`a_forced_return_without_an_expression_answers_nil`,
`a_forced_return_passes_the_vm_s_refusal_of_a_withheld_value_on`). The
stop that follows is a step's. A frame of the core library is neither
restarted nor forced, the VM's refusal "the frame is the core
library's" the request's error (`a_shipped_frame_is_deemphasized`).

The memory view (`evaluation.rs`): `nsNext/instanceCounts {}` answers
`classes`, each `{ name, count }` in the VM's order, the largest count
first, its `name` `<id>[@<version>][/<Class>(.<Class>)*]`, as a function
breakpoint names a class, and `nsNext/shipped: true` for a shipped
module's class, as the VM says (`InstanceCount::shipped`).
`nsNext/instances { name, start, count }` answers a page of the
instances of the class `name` names, as `variables` answers, each named
`#<n>`: `Kernel/Array` a program module `Kernel`'s,
`newspeak.core.Kernel/Array` the binary's
(`instances_of_a_shipped_class_are_asked_for_by_its_module`).
`nsNext/referrers { valueReference, start, count }` answers a page of
what refers to the value, as `variables` answers too: an object as a
variable shows a value, and a frame's variable that holds the value as a variable named
`frame <id> <name>`, the frame's DAP id, its text the frame's name,
with `presentationHint.kind` `virtual` and neither children nor a value
reference, since what it holds is the value asked of. A missing or 0
`count` answers at most 1,000 from `start`, so that a class of millions
of instances is never described whole in one response
(`instance_counts_name_the_program_s_classes`).

A frame's code (`agent.rs`): `nsNext/frameCode { frameId }`, a request
of ns-next's own, answers `{ body, bytecode, machineCode }` for a frame
of the stop, `Command::FrameCode`'s answer as text: the frame's name,
its bytecode and the machine code it goes on in -- `null` for a frame
that goes on interpreted, the top frame among them -- each listing as
`ns-next run --print-bytecode` and `--print-code` print it, marked
where the frame stands ([developer-tool.md](developer-tool.md),
decision 15, "The listings and their text"). An unknown frame id is
refused with "no frame of this stop has that id", and an answer the VM
gives after the program went on with "the program went on"
(`frame_code_marks_the_stopped_instruction`,
`frame_code_marks_where_a_compiled_caller_returns`). DAP's
`disassemble` is not answered: it is built on memory addresses, in
which bytecode has no place.

The steps (`stepping.rs`): `continue`; `next` and `stepIn` by line, by
default, or by statement (decision 4); `stepOut`, one `StepOut`; and
`nsNext/runToPosition`, a request of ns-next's own, which runs to the
statement at or after a line, or a line and column, counted as a
breakpoint's offset is, the VM's refusal its error response. A
`continue`, a step or a run to cursor leaves the stop only when the VM
answers it with `Event::Continued`: a refused one leaves the agent at
its stop, which the client keeps reading, and answers the request with
the refusal; a `pause` sent meanwhile is asked of the program it
resumes, and one sent during a step by line reports the step's next
stop. Requests only a stop serves -- `stackTrace`, `scopes`,
`variables`, `continue`, the steps, run to cursor -- are answered by an
error while the program runs, never sent to the VM; `pause` sends the
remote's pause. `nsNext/setStepFilters` and `nsNext/statistics` (the
number of texts the VM sent, for tests) are requests of ns-next's own.
`nsNext/setStepFilters` takes `shipped`, whether the shipped modules
are filtered (on when left out), and `entries`, each a `source` naming a
module as a breakpoint's does -- a program module's file, a file of the
core library taken, or a reference -- and a `classPath` in it from the
module's root down, the root's own name left out, empty for the whole
module (`StepFilterEntry`); the agent sends `Command::SetStepFilters`
with a `ClassName` for each entry it can take, and the others apply. An
entry whose source names no module of the program is left out silently,
since a client's list may name other programs' files -- a project's
does. One whose path holds an empty or blank segment, or whose class the
module's current text -- the VM's answer, else the text the program was
loaded with, else the binary's -- does not declare
(`Sources::declares_class`, parsed by `ns-syntax`), is left out and said
in an `output` event, once for as long as the client sends the same
filters, so that a client resending them at every change says nothing
new, and again once what is left out changes; a text the agent does not
have or cannot parse leaves the VM to decide, since the filter matches
only the declarations it runs. A request that still names `classes`,
the typed form `entries` replaced, is refused by an error that names the
new form (`a_step_filter_request_with_typed_classes_is_refused`). A
file names its module only, so
a program module named like a shipped one -- a `Kernel` of its own --
and the shipped module are filtered apart (`debug_adapter.rs`'s
`a_step_filter_by_the_library_file_filters_only_the_shipped_module`,
`a_step_filter_by_the_program_file_filters_only_the_program_module`,
`an_unmappable_step_filter_entry_is_left_out_and_the_others_apply`).

The host waits for the configuration before its first run, serving
meanwhile every configuration command the agent queues
(`Interpreter::serve_debugger_between_runs`, woken by the agent), so that
a client has the answer to its `setBreakpoints` before it sends
`configurationDone`, as DAP's order has it; with `stopOnEntry` the agent
pauses the remote when the host announces `main:args:`, which stops
`main:args:` at its first statement ("Commands while the program runs"),
and reports that pause's stop as the entry's. At the program's end the
agent has the pump hand over every event the VM sent before it, answers
each request they answer, and answers every request still waiting with an
error, before it sends `exited` and `terminated`: the host's word comes on
another path than the VM's events. It then answers every request the
client sends until the client disconnects or closes the connection, or 2 s
passed since `terminated`, a deadline rather than a bound on the client's
silence, which bounds how long the process outlives its program however
busy the client is
(`a_client_that_never_disconnects_cannot_keep_the_ended_program_s_process`):
a request sent before the client heard of the end -- a `terminate` racing
it -- is answered like any other, `terminate` and `disconnect` with
success, the program having ended, and every other request with an error
(`a_request_after_the_program_ended_is_answered`). A listening agent gives
a connection 10 s from its acceptance to present the token in a good
`attach`, a deadline for the whole exchange rather than a read timeout, so
that neither a silent nor a trickling peer holds the one connection it
serves, while an attached client may stay silent as long as it likes; the
agent's loop keeps the deadline, waiting for its next input no longer than
the deadline is away and checking it before every input, so that no thread
waits for it and a peer that sends without a pause cannot hold it off. The
agent never writes to the socket itself: it queues each message for the
connection's writer thread (`connection::Writer`), so that a client that
does not read never blocks it, and with it the VM's events, the host's
word, a pause or a terminate. The writer bounds each message's whole
write, however the client reads -- by the attach deadline until the client
attached, by 30 s from the message's start after -- and its queue holds
1,024 messages; the agent's wait for what is still queued as a session
ends -- at the program's end, or before `terminateDebuggee`'s kill -- is
bounded by one 30 s deadline as a whole, past which what is left is
dropped and the connection shut down, since the process's end waits for it
(`finish_ends_within_its_deadline_whatever_is_queued`); a write past its
bound, or a full queue, ends the session as a disconnect does: the
connecting mode detaches, the listening mode resets the VM and accepts the
next client (`a_peer_that_never_reads_is_closed_at_the_deadline`,
`an_attached_client_that_stops_reading_is_dropped`). A debug build reads
both bounds and the 2 s after `terminated` from variables, so that their
tests take a fraction of a second. The agent refuses a connection it
cannot start a reader or a writer for, which ends its session as a client
that left before it attached: the connecting mode's program starts without
its debugger. A client of the connecting mode that leaves by `disconnect`
detaches the debugger: both halves of the remote are dropped and the
program runs on, a host still waiting for the configuration starting it.
One whose connection closes before a `disconnect` -- its launcher, which
owns the process, gone: the plugin and the MCP server both start their
child -- ends the program as `terminate` does, its pending `ensure:`
blocks run and every stop on the way going on with no client there, with a
terminated program's exit status, a host still waiting for the
configuration starting no run (decided on 2026-11-17;
`a_lost_connection_ends_the_program_in_the_connecting_mode`). A client of
the listening mode that leaves, once attached, has the VM reset -- no line
breakpoint in any module the client set one in, no method or exception
breakpoint, since nobody would be there to go on from a stop, the default
step filters, none of its marks, a stopped program going on -- and the
next client is accepted. An `Unmark` is a configuration command, served
at a safepoint while the program runs as well as at a stop, so a client
that leaves while the program runs has its marks taken off at once: no
condition or log expression of the next client's, which the VM decides
before any stop, names a label of the departed client's, and the marks
keep their objects alive no longer
(`a_client_leaving_while_the_program_runs_leaves_no_mark`,
`a_departed_client_s_label_is_unknown_to_the_next_client_s_condition`);
a mark the VM makes for the departed client after the leave, its
request already queued, is taken off as soon as it is answered. The VM
serves a `Continue` only at the stop an evaluation's end re-enters,
refusing one that arrives while the evaluation runs, so a client that
leaves with an evaluation in flight has it aborted
(`Command::AbortEvaluation`) and the stop left once the evaluation
answered (`a_client_leaving_during_an_evaluation_lets_the_program_go_on`);
a host still waiting for the configuration waits for that client's, as
it waited for the first. A stop that arrives while no client is attached goes on at
once.

### Threads

**Built** for the one thread there is (`stopped.rs`).

The VM is single-threaded: the protocol shows one thread. A pull future's
forcing runs on the same stack and shows as frames of it. Each actor will
be a thread of its own ([liveness-and-reflection.md](liveness-and-reflection.md),
"Concurrency: actors, not threads").

### The time a step took

The agent measures the wall time from the resume to the stop, minus the
time it spent serving commands meanwhile, and sends it with the `stopped`
event, which the plugin shows inline as IntelliJ does. It is planned in
no stage yet (decided with the user on 2026-11-15): neither stage 5 nor
stage 6 builds it ("Planned: the order of building it").

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

## The MCP server

**Built** (`crates/ns-cli/src/mcp/`; its tests `crates/ns-cli/tests/mcp.rs`).

An agent such as Claude debugs a program through `ns-next mcp`, a Model
Context Protocol server over standard input and output, whose tools
drive a debug session (decided with the user on 2026-11-17, in stage 5,
before the plugin's stage-5 work). A debugging session is a dialogue --
stop, look, choose the next step in the same state -- which an agent
whose every command is a separate call cannot hold with a process of its
own; the client keeps an MCP server alive for its whole session. A
command-line client of the debugger, driven by a file of commands, is
the way not taken: every question would run the program from its start.

- **The protocol** (`mod.rs`): JSON-RPC 2.0, one message a line, the
  specification's revision 2025-06-18. `initialize` answers the
  client's revision when it is 2025-06-18, 2025-03-26 or 2024-11-05, and
  2025-06-18 otherwise, with the capability `tools` and the server's name
  and version; then `ping`, `tools/list` and `tools/call`. A
  notification is not answered, an unknown method is error -32601 and an
  unknown tool -32602, and a batch is answered as one. Written by hand
  on `serde_json`, as the agent's framing is, so that no protocol crate
  reaches `ns-next`. Nothing but protocol messages goes to the standard
  output; the server's own words go to the standard error -- the
  process each program runs as, and every request the agent refused.
  The server ends when the client closes its standard input, and every
  session's program with it. The thread that reads the input answers
  `initialize`, `ping`, `tools/list` and a refusal at once and hands a
  tool call to its session's worker (`worker.rs`), a thread of its own
  that serves the session's calls in their order of arrival -- but for a
  `debug_pause` or a `debug_stop` made while a call waits for the
  program, which acts on that wait at once, and a call the client
  cancelled (`notifications/cancelled`), which ends its wait and answers
  nothing; one writer
  thread owns the standard output (`responses.rs`), and a batch is
  answered once each of its members is. So a wait in one session holds
  neither another session's calls nor a `ping`
  (`a_wait_holds_no_other_session`).
- **The program runs in a child** (`session.rs`, `dap_client.rs`). Each
  `debug_start` starts `ns-next run --debug-connect` against a socket the
  server listens on, as the plugin does, hands the child a one-time secret
  in `NS_NEXT_DEBUG_SECRET`, and goes on only when the `process` event
  names the child's process id and that secret. The server is a DAP client
  of the agent (`crates/ns-cli/src/debug/`) on the agent's framing
  (`wire.rs`), with the requests the plugin uses, and asks it to stop at
  uncaught exceptions and at `halt`. The program writes to its own
  standard streams, which the server reads; a crash ends the child, never
  the server; and the child is killed when its session ends -- at
  `debug_stop` past its wait, and when the server ends.
  A server killed outright drops the child's connection, and the child's
  agent, whose launcher is gone, ends the program as a `terminate` does
  ("Launching, the start and the end";
  `killing_the_server_ends_its_program`). Running the program inside the
  server would put the program's output on the stream the protocol owns
  and take the server down with a crash.
- **The tools** (`tools.rs`). Each `debug_start` opens a session of its
  own, numbered from 1, a number never given twice, and answers
  `session: <n>` on its first line; every other tool takes `session`,
  which may be left out while one session is open, and is refused naming
  the open sessions, each with its program and state, when it names none
  open or is left out among several (`registry.rs`). A session lasts
  until `debug_stop` closes it or the server ends, after its program
  ended too. `debug_start(program, breakpoints?, stop_on_entry?,
  arguments?, runtime?, trace?, trace_filter?, listings?, listing_filter?,
  wait?)` (`trace` is `ns-next run --trace`'s value, `trace_filter` an
  array of patterns, each passed as one `--trace-filter=<pattern>`, a
  prefix ending in `...`; `listings` any of `bytecode`, `code` and
  `inlining`, each passed as `--print-<kind>`, and `listing_filter` one
  `--print-filter=<pattern>` each, refused without `listings`; all go
  before the program, and with `trace` or `listings` the child writes
  them through `--trace-file` into a file of the server's,
  `traces/<server>/session-<n>.trace` in the user's own directory,
  deleted at `debug_stop`, so that nothing of them reaches the
  program's output), `debug_breakpoints(set?, clear?)`,
  `debug_continue(wait?)`, `debug_step(kind: over | into | out, target?,
  wait?)`, `debug_step_targets`, `debug_run_to(location, wait?)`,
  `debug_pause`, `debug_wait(seconds)`, `debug_stack`, `debug_code(frame?)`
  -- the frame's bytecode and the machine code it goes on in, marked
  where its call returns, or `the frame goes on in the interpreter`,
  through `nsNext/frameCode` ([developer-tool.md](developer-tool.md), "Stage 2:
  printing code") -- `debug_variables(frame? | reference?, start?,
  count?)`, `debug_evaluate(expression, frame?)`,
  `debug_set_variable(name, value, frame?)`, `debug_frame(action:
  restart | return, frame, value?)`, `debug_memory(class? |
  referrers_of?)`, `debug_mark(reference?, label)` -- without a
  reference it takes the label off -- and `debug_stop`. A breakpoint is
  a location, `<absolute path>:<line>`, or a method,
  `<id>[@<version>][/<Class>(.<Class>)*]>>selector` (` class>>selector`
  for the class side) as a function breakpoint names it, each with an
  optional `condition`, `log` message or `hit_condition`. The server
  holds a session's breakpoints and sends a file's whole set, or the
  methods', at each change, since DAP sets them so; a breakpoint at a
  place already set replaces it, and `clear` names a place. Frames are
  counted from 0, the top. Every answer is one text; a refused or failed
  call is an error with its reason. No tool reads source: the agent
  reads files itself.
- **A stop is a summary in text** (`summary.rs`). A tool that lets the
  program go on answers, at the next stop, one block, each line only
  when it has something to say: `stopped: <reason> at
  <path>:<line>:<column> in <frame> (frame 0)` (or `ended: exit status
  <n>`, or `running for <s> s`); two source lines before and after,
  numbered, `>` at the stop's; `exception: <class>: <text>` at an
  exception's stop, and `condition failed: <error>` at a breakpoint
  whose condition or log message failed; `locals:` the top frame's
  variables, each value cut at 80 characters, at most 20, then how to see
  the rest; `stack:` at most 8 frames on one line, each with its file's
  name and line, then how to see the rest; `output:` the program's
  standard output and error since the last answer, quoted as a JSON
  string, its last 2,000 characters and how many came before; and on
  `debug_start`'s answer `runtime:` the binary and the fingerprint of its
  shipped sources. A value with children carries a reference (`[ref
  4]`), the agent's value reference for it, which `debug_variables`
  expands, `debug_mark` marks and `debug_memory` asks the referrers of,
  valid until the program goes on. `debug_stack` and `debug_variables`
  answer one entry a line in the same forms, every location an absolute
  path, at most 100 unless asked for a page; an evaluation and a set
  variable answer the value and the output since the last answer. Text,
  not JSON: it is what an agent reads, in fewer tokens, and as precise.
  The output a program wrote before it stopped is still on its way
  through the pipe as the stop arrives, so the summary takes the output
  once it has been quiet for 20 ms, waiting at most 200 ms.
- **Every location is an absolute path** with its line and column, in
  answers and in arguments alike, so that the agent opens it as it is
  and two modules of one name never meet; a relative path is refused.
  The shipped modules are files too: `debug_start` writes the started
  binary's embedded sources with `ns-next shipped-sources` into
  `ns-next/core-library/<fingerprint>/` in the user's own directory
  (below), once per fingerprint, and hands the directory to the agent at the attach as
  `coreLibrary`, as the plugin hands its library, so that a frame in
  `Kernel.ns` names a file whose text is the running binary's. A
  location that names no module of the program or of the shipped ones is
  refused, saying so, and not held.
- **Waiting.** A tool that lets the program go on waits for the next
  stop at most `wait` seconds (30 by default, 600 at most) and otherwise
  answers that the program runs, with its output so far; `debug_pause`
  stops it where it is and answers the stop, `debug_wait` waits on. An
  evaluation that does not end within 30 s is aborted by DAP's `cancel`,
  which the agent turns into the abort a hover's bound takes
  (`Command::AbortEvaluation`), and answered so, the program still
  stopped; an evaluation that ended otherwise meanwhile is answered in
  its own words. `debug_stop` ends the program as its own `terminate…` would,
  its `ensure:` blocks run, and kills it when it has not ended within
  5 s.
- **The shipped sources** (`shipped_sources.rs`) of the binary a session
  runs lie in the user's own directory -- on Windows the local
  application data directory (`%LOCALAPPDATA%`), elsewhere the cache
  directory (`$XDG_CACHE_HOME`, or `~/.cache`) -- never in the shared
  temporary directory, where another user could put a file of their own
  at the path first, which the server would show (decided on
  2026-11-17). A directory is written under a name of its process's and
  its session's and renamed into place, so that no session reads a half written one. A
  server that starts removes a directory of shipped sources untouched
  for a day -- its `fingerprint` file's time, which a `debug_start` that
  reuses the directory touches, or the directory's own without one --
  but the one of its own binary's fingerprint; no process runs from it,
  and a session of a binary whose sources were removed writes them
  again, a removal that fails leaving the directory
  (`a_starting_server_removes_stale_shipped_sources_but_its_own`).
- **Starting it.** `ns-next mcp` is a plain server and knows nothing of
  the build that made it: an installed `ns-next` is never rebuilt
  (decided with the user on 2026-11-17). `debug_start` runs the program
  with the server's own executable (`std::env::current_exe`) unless
  `runtime` names another binary
  (`a_start_without_a_runtime_runs_the_server_s_own_executable`). In
  this repository `.mcp.json` starts the server from a copy of the debug
  build, `target/mcp/ns-next mcp`, which the client starts directly, so
  that no process of the server's locks what cargo writes: on Windows a
  running executable can be neither replaced nor removed, and a server
  run from `target/debug/ns-next` would keep `cargo build` from writing
  it. A long-lived `cargo xtask` launcher, which the client would start
  and which would start the server, is the way not taken: it would run
  for the whole connection and keep cargo from relinking `xtask` after a
  change to it or to `ns-runtime`. The client starts the path as given,
  without a shell; Node's `child_process`, on which the MCP SDKs' stdio
  transports are built, starts `target/mcp/ns-next` on Windows without
  the `.exe`, which the system adds (checked 2026-11-17). `cargo xtask
  mcp` (`xtask/src/mcp.rs`) puts the copy: it builds nothing and starts
  nothing, refuses, naming `cargo build -p ns-cli`, when there is no
  build, and copies `target/debug/ns-next` to `target/mcp/ns-next`,
  written under a name of its process's and renamed into place, only
  when nothing is there, after removing the partial copies a task that
  died midway left; a copy already there stays, since a server may
  run from it, and the task says so. Until the task has run, the client
  fails to start the server. The server and the programs it debugs run
  from that copy until it is refreshed -- `target/mcp/` deleted while no
  server runs from it, `cargo xtask mcp` run again and the client
  reconnected (`/mcp` in Claude Code) -- and a session started with
  `runtime` naming `target/debug/ns-next` runs the newest build, which
  then locks that file for as long as the session's program runs.

## The JetBrains plugin

The plugin speaks DAP through a client of its own
(`editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/debugger/dap/`),
on the Gson the IntelliJ Platform bundles, so it takes no dependency:
`DapConnection` frames each message with its `Content-Length` in UTF-8
bytes, bounds a frame before it reads it as the agent does, and reads on a
thread of its own; `DapConnection.accept` takes the connection of an
`ns-next run --debug-connect` from the socket the plugin listens on.
`DapClient` numbers the requests, answers each with a `CompletableFuture`
of its response's body, failed with the response's `message` when it
reports a failure, and hands each event's body to the listeners of its
name; a close, by either side, fails every request still unanswered and
tells every close listener, one that fails keeping none of the others from
it. Whatever ends the reader -- a malformed frame, the stream's end,
anything the message handler throws -- closes the client, so no request
waits for ever. Every future completes and every listener runs on the
reader thread, in the order the agent sent them, so the debug process hops
to the thread its work needs. `DapTypes.kt` reads the bodies the plugin
uses. A test debugs a program with the real runtime through it
(`DapSessionTest`, [editor-support.md](editor-support.md), "Plugin testing
and gates").

Debug on a Newspeak run configuration that runs a program
(`debugger/NewspeakDebugRunner.kt`; a benchmark is not debugged) binds
127.0.0.1 on a port the system chooses and starts `ns-next run
--debug-connect=127.0.0.1:<port>`, the runtime connecting to the plugin
("Launching, the start and the end"), through
`NewspeakDebuggeeProcessHandler`, a `KillableProcessHandler`. The debug
process (`NewspeakDebugProcess.kt`) takes the connection within 30 s,
waits, at most 60 s, for the look at the project's core library the
session started with -- which moves the line breakpoints to a rebuilt
binary's library before any is sent ([editor-support.md](editor-support.md),
"The core library") -- initializes and attaches, and checks that the `process` event's
`systemProcessId` is the PID of the process it started and its
`nsNextSecret` the secret the runner made for that process -- 128 bits of
`SecureRandom`, handed to it in `NS_NEXT_DEBUG_SECRET`, in its environment
rather than on its command line, which other processes can read --
compared in constant time; an event without the secret fails the check,
since the plugin's own process always has it. On a mismatch -- another
process connected to the port first -- it reports what it compared and
what the event named, kills the process and the session ends. Nothing else
the connection says is acted on before a matching `process` event: an
`initialized`, `stopped`, `breakpoint`, `output` or `terminated` event
ahead of it, or the connection's close, ends the session the same way,
since the agent sends that event first and only another peer would not. At
the `initialized` event it sends every breakpoint the session registered
-- the line breakpoints a file's set at a time, the method breakpoints and
the exception breakpoints, the last two even when empty -- and the step
filters, then `configurationDone` once the agent answered them. The platform's Stop
destroys the process: while the agent is attached the handler asks it to
`terminate` instead, so the program ends as its own `terminate…` would,
and the platform's second Stop, on a process still terminating, kills it
(`KillableProcess.killProcess`), which ends a program inside one long leaf
at once. `XDebugProcess.stop()`, which ends a session otherwise, sends
`terminate` to a program still running too, and keeps the connection until
the agent answered or the process ended, at most 10 s, since a connection
closed at once would lose the request; a process whose agent did not
answer is killed. The process's console is the program's own, the agent's
`output` events printed into it.

A `stopped` event reads the first page of the stack (50 frames) before the
stop is shown, so that the top frame is at hand: a
`NewspeakSuspendContext` holds one `NewspeakExecutionStack`, the thread
`main`, which reads the rest a page at a time when the frames view asks. A
stop with `hitBreakpointIds` is shown through `breakpointReached` for the
first line or method breakpoint they name, every other through
`positionReached`. An exception's stop (`reason: "exception"`) also tells
the user which exception stopped it, the event's `description` and `text`
-- `ZeroDivide: division by zero` -- or that a halt did, as the debugger's
balloon (`XDebugSession.reportMessage`, a warning), which the platform
shows whichever of the session's tabs is in front, where the console holds
the program's own streams and would mix the message into them
(`testAnExceptionStopSaysWhichException`). A `NewspeakStackFrame` is
placed through `NewspeakSources`: a file the agent names by its path -- a
program module's, and a file of the core library the session handed the
agent at `attach` (`coreLibrary`, the directory the project shows), where
a shipped module's breakpoints stand ("The agent's sources";
[editor-support.md](editor-support.md), "The core library") -- opened
directly, and any other module -- a shipped one without the library, an
image's, one an install changed -- by the `source` request's answer, read
once per reference for the session, as a read-only `LightVirtualFile` of
the name `<Module>.ns` holding the answer, one per reference for the
session; a hidden frame (`presentationHint: "subtle"`) stays listed, drawn
greyed. A frame's equality object (`XStackFrame.getEqualityObject`), by
which the platform keeps the nodes the user expanded in the variables view
and restores them at the next stop, is the body it runs -- the agent's
name of it -- its module's file or name and its depth, the frames below it
counted, since the DAP frame's `id` is numbered afresh at every stop: a
step within one body, or a loop's block activated again at the same depth,
keeps what the user opened, as Java's frames do by their method, while a
recursive activation, deeper, is another frame; the object is a data
class, since the platform's front end compares it by its text. Its
children are the `Return value` scope's value, at a step's stop, above the
`Locals`, named by its call and drawn with the platform's icon of a
returned value (`AllIcons.Debugger.WatchLastReturnValue`), as Java's is,
whose name carries no other mark; a `NewspeakValue` shows the agent's text
and class of a value and pages its children by 100, the tree offering the
next page (`tooManyChildren`) -- an indexed value's count known, a named
value's until a page comes back short. Resume is `continue`; Step Over,
Step Into and Step Out are `next`, `stepIn` and `stepOut` with
`granularity: "line"`; Run to Cursor is `nsNext/runToPosition`, Pause
`pause`; a Run to Cursor in a file the agent cannot name -- a module's
text an earlier session showed -- is reported and leaves the program at
its stop, and one before the runtime connected says the session has not
started. A step or a resume the agent refuses leaves it at its stop, which
the debug process shows again with the refusal.

Every event reaches the client's reader thread, which must never wait for
an answer; the debug process hands each on at once to a thread of the
session's own, which takes them one at a time in the agent's order and
waits, bounded, for the answers a stop needs before it tells the platform,
which allows that from any thread. The value tree's and the frames view's
requests complete their nodes from the reader thread, as the nodes allow;
nothing waits for an answer on the event dispatch thread. Nor does any
caller wait for the socket: `DapConnection.send` hands each frame to a
writer thread of the connection's own, which writes them in order. An
event or close listener that throws, an `Error` included, is logged and
keeps no other listener from being called.

Three breakpoint types, each with its `XBreakpointHandler`
(`NewspeakBreakpointHandlers.kt`), which sends its kind's whole set at
every change, as DAP sets each kind -- line breakpoints a file's set at a
time:

- **Line breakpoints** (`NewspeakLineBreakpointType`) stand on the lines
  the language server's `newspeak/breakableLines` answers a statement
  starts on, before any session runs, kept per document stamp by
  `BreakableLinesCache`; `canPutAt` never waits, and allows every line
  while there is no answer for the document's current text and for a
  document that does not parse ([editor-support.md](editor-support.md),
  "Breakable lines"). A line on which a block's first statement starts
  offers the line and each such block as variants; a block variant's
  breakpoint is sent with the `column` of that statement, the line's
  without one. A file on disk is named by its `path` -- a program
  module's, and a file of the core library, which the agent takes for the
  shipped module, or for the program's embedded `Manifest`, only in the
  library the session handed it ("The agent's sources"), so that a file of
  any other fingerprint directory -- an older binary's text, which a look
  that failed left breakpoints in -- names no module and its breakpoints
  stay unverified; in a run of one module, which loads no manifest, the
  agent answers a `Manifest.ns` breakpoint unverified -- and a module the
  session shows by reference by its name. The breakpoints in the library's
  files are kept with the project and follow it to a new fingerprint
  ([editor-support.md](editor-support.md), "The core library"). Run to
  Cursor names a library file the same way. The pass count
  (`NewspeakLineBreakpointProperties`, edited in the breakpoint's
  properties) is sent as the `hitCondition`, in the agent's forms ("Hit
  counts and temporary breakpoints"). The type answers
  `createProperties`, into which the platform loads the saved
  properties of a breakpoint it restores from the workspace, so that the
  pass count and a block variant's column outlive the IDE's restart
  (`testARestoredLineBreakpointKeepsItsProperties`). The handler marks each breakpoint
  verified, or, where the agent set it at another line, verified with the
  line it stops at (`updateBreakpointPresentation`) -- the breakpoint
  stays where the user put it -- or invalid with the agent's message, as
  its answer and every later `breakpoint` event say. "Remove once hit"
  needs nothing of the plugin: the platform removes the breakpoint as the
  stop `breakpointReached` reported is left, and the removal resends the
  file's set before the `continue`.
- **Method breakpoints** (`NewspeakMethodBreakpointType`), added from the
  Breakpoints dialog by the method's name in the agent's form,
  `<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>` --
  `Demo/Account>>deposit:`, `com.acme.Json@2.1.0/Parser class>>on:`
  ([modules.md](modules.md), decision 8) -- with a pass count, sent by
  `setFunctionBreakpoints` as written. One the agent cannot set -- its
  name refused, or no built module declaring the method yet -- is
  reported with the agent's reason, since the platform shows whether a
  breakpoint is verified for line breakpoints only; a breakpoint kept in
  the dotted form of before, `Demo.Account>>deposit:`, is one such, shown
  with the agent's refusal, which names the form to write, and never
  rewritten (`testAMethodBreakpointOnANestedClassStops`,
  `testADottedMethodBreakpointIsSentUnchangedAndShownWithTheAgentsRefusal`).
- **Exception breakpoints** (`NewspeakExceptionBreakpointType`): the
  default "Any exception", on, with boxes for uncaught exceptions (on),
  caught ones (off) and `halt` (on), and one per class the user adds,
  `<id>[@<version>][/<Inner>(.<Inner>)*]` -- `newspeak.core.Kernel/Error`
  for the kernel's `Error` -- with boxes for uncaught and caught
  exceptions; sent by `setExceptionBreakpoints` as the filters
  `uncaught`, `caught` and `halt`, each class breakpoint's filters as
  `filterOptions` whose condition names its class as written, a name the
  agent refuses reported as the request's error
  (`testAnExceptionBreakpointByIdStops`). The agent keeps one list of
  classes for both filters (`Command::SetExceptionBreakpoints`), so a class taken for
  caught exceptions is taken for uncaught ones too when another class
  breakpoint takes those, and "Any exception" taking uncaught or caught
  exceptions makes the class breakpoints' filters take every class.

The step filters (decision 16) are two settings. Whether a step goes
into the kernel's and the platform's code, which by default it passes,
is application-wide, as Java's stepping switches are, kept among the
platform's debugger settings (`NewspeakDebuggerSettings`, an
`XDebuggerSettings` registered as `xdebugger.settings`) and edited in
Newspeak's section of Settings | Build, Execution, Deployment | Debugger
| Stepping (`NewspeakSteppingConfigurable`, the
`DebuggerSettingsCategory.STEPPING` configurable) and by "Step Into Kernel
and Platform Code" in the debug tool window's Debugger Settings
(`NewspeakStepIntoShippedAction`, a toggle in the platform's group
`XDebugger.Settings`, shown for a Newspeak session only), the same
setting, as Java's switches there are its settings'. The classes not to
step into, IntelliJ's "Do not step into the classes", are the project's
(`NewspeakSkippedClasses`, a project service kept in the workspace, as the
breakpoints are), on a page of their own under Debugger, "Newspeak Step
Filters" (`NewspeakStepFiltersConfigurable`, a `projectConfigurable` whose
parent is the debugger's page, as Java's Async Stack Traces is, since the
Stepping section has no project, and registered for real projects only,
`nonDefaultProject`, since the default project has no files to choose). An
entry is never typed: Add lists the project's `.ns` files and the core
library's at once, under a progress the user may cancel, as a tree with a
speed search, each file a node for its whole module, and asks a file's
classes -- those its root nests, as the language server's document symbols
answer them -- only when its node is expanded, on a pooled thread, for at
most 15 seconds, after which a file the server has not answered offers its
whole module only; closing the dialog cancels every question still asked
(`StepFilterChoices.kt`, `StepFilterTree`). The user chooses a file, for
its whole module, or a class, for it and every class nested in it. An
entry is a file and a class path in it, the root's name left out: a
library file by its path below the library's directory --
`platform/CollectionsFactory.ns`, never the fingerprint's -- so that it
names the same file in a rebuilt binary's library, a project file by its
path below the project's directory, one outside it by its own. The page
marks an entry whose file is gone or whose file no longer declares its
class, decided off the event dispatch thread; an entry whose path no file
system holds -- a damaged workspace file -- counts as gone
(`SkippedClass.resolvedAgainst`), so that it never stops a session's
configuration. A session sends the filters as `nsNext/setStepFilters` --
`shipped` the negation of the switch, `entries` each entry whose file is
there by its absolute path, a library file's under the library the project
shows now, so that a library entry is not sent while the project shows no
library -- the page marks it then, as it marks an entry whose file is gone
-- and its `classPath` -- before `configurationDone`, and again whenever
either setting changes: a change is told on the settings' topic, which
each session hears from its start to its `stop()`, its subscription a
child of the project's step filters service; it is sent at once, on the
thread that changed them, which waits for no answer, so that a step
started after a change is sent after it and runs under it, and the filters
are read and their request queued under one lock, so that the
configuration's send and a change's at the session's start reach the agent
in the order they read the settings. The agent maps each entry's file as
it maps a breakpoint's ("The agent's sources"), leaves out an entry of a
file of no module of the program unsaid, and says one it cannot take
otherwise in an `output` event, which the console shows.

A property the user edits -- a pass count, an exception breakpoint's
boxes -- reaches a running session by turning the breakpoint off and on
again through its public setter, whose change events unregister and
register it: the platform's own way to report a plugin's property is
internal API. The cost is that one edit fires two change events, with or
without a session running, and in a session sends the breakpoint's set
several times -- once at each unregister and register -- of which the
last is the one that counts.

Each frame evaluates through the agent (`NewspeakEvaluator`, the
frame's `XStackFrame.getEvaluator`): the Evaluate dialog, every watch at
each stop and the editor's hover send `evaluate` with the frame's id,
and the answer is shown as a `NewspeakValue`, its children paged as a
variable's; an exception that ended it, or the agent's refusal -- "the
expression does not compile: …" -- is the error shown in the value's
place. A hover takes the identifier under the pointer only when it names
one of the frame's variables, read from its `Locals` once a stop: reading
a variable runs no code, while any other name may be a send of the
receiver's, which a hover would run unasked; so a hover cannot hang and
needs none of the agent's hover bound. A frame's own variable has a
`NewspeakValueModifier` (`XValue.getModifier`), Set Value, which sends
`setVariable` to the frame's `Locals` reference and has the platform read
the frame again; a withheld value -- the agent's `readOnly` -- a value's
child and an evaluation's value have none, since the agent sets a frame's
variables only (`testEvaluateAnswersALocalsValue`,
`testAWatchIsEvaluatedAtEachStop`, `testSetValueChangesTheProgramsOutput`).

The line and method breakpoint types give the platform's condition and
log expression fields Newspeak's editor (`getEditorsProvider`). The
platform evaluates neither: `XDebugSession.breakpointReached` leaves a
condition to the debug process, and the handlers send it as the agent's
`condition`, which the runtime decides at each hit, once, so that a
condition with a side effect runs once a hit
(`testAConditionIsSentAsTheCondition`,
`testAConditionIsEvaluatedByTheRuntimeOnly`). A line breakpoint's log
expression is the agent's `logMessage`, the expression in braces, where
nothing needs the stop -- it suspends nothing and logs neither its hit
nor the stack, which the platform does only at a stop -- and the agent
logs it as an `output` event and goes on
(`testALogExpressionIsSentAsTheLogMessage`). Every other log expression,
of a breakpoint that stops and of a method breakpoint, to which DAP gives
no log message, the session evaluates at the stop, once, in the top
frame, and hands to `breakpointReached` to log
(`testALogExpressionAtAStopIsLoggedOnce`). A stop whose condition or log
message failed -- the agent's `breakpoint` stop with a `text` -- is shown
as a stop and says why in the debugger's balloon, whatever the
breakpoint's suspend policy, never through `breakpointReached`, which
would resume a breakpoint that suspends nothing, a logpoint among them,
and drop the error unseen; nor is its log expression evaluated
(`testAFailedConditionStopsAndSaysWhy`).

Smart Step Into is `NewspeakSmartStepIntoHandler`, an
`XSmartStepIntoHandler` over `stepInTargets`: the sends of the stop's
line in the order the line runs them, each highlighted at the span the
agent answers, the chosen one stepped into by `stepIn` with its
`targetId`; it asks the stop's top frame, and where the agent refuses it
-- "smart step into takes the top frame", at an exception's stop, whose
top frames are hidden machinery -- the first frame not hidden
(`testSmartStepIntoOffersTheLinesCalls`). The platform's Reset Frame is
`NewspeakDropFrameHandler`, the debug process's `XDropFrameHandler`, over
`restartFrame`, offered on a frame whose `canRestart` the agent does not
deny -- a frame of the core library's -- and the stop it makes at the
frame's start, of the reason `restart`, is shown as any other
(`testDropFrameRestartsTheMethod`). Force Return
(`NewspeakForceReturnAction`, in the frames view's menu beside Reset
Frame, as Java's) asks for an expression and sends `nsNext/forceReturn`
with the selected frame and the expression, none for a blank one, which
returns `nil`; the stop that follows is where the frame's sender goes on,
and a refusal is reported with the program still at its stop
(`testForceReturnSendsTheExpression`).

Mark Object… and Unmark Object (`NewspeakMarkObjectAction`, in the value
trees' menu, an `XDebuggerTreeBackendOnlyActionBase`, which hands it the
debug process's own value under the split debugger too, where the tree's
node holds a frontend proxy of it) mark a value the agent hands out a
value reference for -- not a withheld one
(`testAWithheldValueIsNeitherSetNorMarked`) -- with a label the user gives
-- an identifier, the value's name by default -- by `nsNext/mark`, and
take a value's label off by `nsNext/unmark`; the session reads its views
again, and the agent shows the label after the value's text wherever a
stop shows it, which the session reads back as the value's label for the
labels it gave (`NewspeakValueMarkers`), and names the object in every
later evaluation as `<label>_DebugLabel`
(`testAMarkedObjectIsNamedInAnEvaluation`). The platform's own Mark Object
stays hidden for a Newspeak session, which gives it no
`XValueMarkerProvider`: the platform hands the provider the value alone,
never the label typed in its dialog, which it keeps in a map only its
internal API reaches (`XDebugSessionImpl.getValueMarkers`), and the agent
names the object by that label.

The memory view (`debugger/memory/NewspeakMemoryView.kt`) is a panel of
the session's tab, Memory, registered by the debug process's
`XDebugTabLayouter.registerAdditionalContent` minimized beside the
variables, as Java's Memory tab is, and so shown with every Newspeak
session and gone with it. At a stop it is shown at, and on its Refresh,
it lists each class's instance count, `nsNext/instanceCounts`, the
largest first, a shipped module's class marked as the core library's --
a walk of the whole object space, never asked while the panel is hidden;
a class chosen lists its instances below as a value tree, the
platform's `XDebuggerTree` under one node whose children are
`nsNext/instances`' pages, by the class's name alone, which names one
module's class, a shipped module's by its `newspeak.core.` id; and
leaving the stop empties both, since the next stop knows none of the
references. Every value the agent hands a
value reference for, in the memory view and in every other tree, answers
the platform's Show Referring Objects through its `XReferrersProvider`,
whose value's children are `nsNext/referrers`' pages: an object, or a
frame's variable, `frame <id> <name>`, which names a place and is neither
expanded nor set (`testTheMemoryViewListsInstanceCounts`).

The core library's frames are library frames. The platform's hook is the
debug process's `isLibraryFrameFilterSupported`, which shows the frames
view's Show Library Frames for the session, and the execution stack
respects the setting it toggles
(`XDebuggerSettingsManager.DataViewSettings.isShowLibraryStackFrames`),
as Java's stack does: while it is off -- its default -- each run of
frames the agent marks `nsNext/shipped` below the top frame is folded
into one greyed line that counts them (`LibraryFrames`), a run that goes
on past a page held until it ends; the top frame, where the program
stands, is always shown. The fold is the plugin's own frame: the
platform's, `HiddenStackFramesItem`, is internal API, which only Java's
stack builds (`testShippedFramesAreLibraryFrames`).

The plugin uses platform API marked experimental, which the platform may
change and `verifyPlugin` reports as warnings: `XDropFrameHandler` (Reset
Frame) and `XDebuggerTreeBackendOnlyActionBase` (Mark Object…). It uses
classes of the platform's `impl` packages that are not marked internal:
`XDebuggerTree` and `XValueNodeImpl`, the memory view's value trees. It
uses no
internal API: where only internal API offers a feature -- the platform's
Mark Object labels, its fold of hidden frames -- the plugin builds its
own.

Show Frame Code (`ShowFrameCodeAction`), in the frames' context menu
(`XDebugger.Frames.Tree.Popup`) and the debug tool window's toolbar,
shown for a Newspeak session and enabled at a stop on one of its frames,
asks the agent for the selected frame's code, `nsNext/frameCode`
("Launching, the start and the end"), and opens it in a read-only
editor tab of the developer tool's listings (`CodeListingFile`;
[developer-tool.md](developer-tool.md), decision 31, "The plugin's
half"): the frame's bytecode, marked where it stands, then the machine
code it goes on in, or "This frame goes on in the interpreter." An
agent that refuses is answered with a notification of the plugin's
"Newspeak" group naming its message, and no tab opens
(`testShowFrameCodeRefusedIsNotified`).
`FrameCodeFollower` then keeps the tab on the selected frame: at every
stop and every selection of another frame it asks again on the
connection's thread and writes the answer on the event dispatch thread,
only the latest request's, since several selections' answers arrive in
no fixed order, as the change between the old text and the new alone,
each editor's caret then at the marked line, so that a step never
scrolls the tab to its start, and that line alone coloured whole, in
every editor of the tab, as the scheme colours the debugger's execution
point in a source editor (`DebuggerColors.EXECUTIONPOINT_ATTRIBUTES`),
the colour gone while the program runs, where a text marks no line
and when the follower's lifetime ends, however it ends
(`FrameCodeFollowerTest`, `testShowFrameCodeOpensTheFramesCodeAndFollowsSteps`,
`testTheFrameCodeLineIsUncolouredWhileTheProgramRuns`); it ends when the
tab's last editor closes -- not one split of it -- or the session stops, its
lifetime a child of the session's (`NewspeakDebugProcess.sessionLifetime`,
disposed with the project at the latest), and is not made for a session
that stopped while the first answer was on its way
(`testShowFrameCodeOpensTheFramesCodeAndFollowsSteps`).

A test debugs programs with the real runtime through the platform's
debugger (`NewspeakDebugProcessTest`), and one sets each kind of
breakpoint over a scripted agent and stops at a temporary one with the
real runtime (`NewspeakBreakpointTypesTest`;
[editor-support.md](editor-support.md), "Plugin testing and gates").

Planned: the plugin will add, over that client, a hot-swap action on the
agent's own requests, and inline values and step times.

## Planned: the order of building it

Stage 6 remains. Stage 1, the
debugging API at the safepoint -- stopping, the command channel and the
commands served while running, the atomic poll words and pause, the
deoptimization of a stop at a poll of optimized code in every build, the
stack and values -- is built ("The debugging API at the safepoint"), and
so are stage 2, the metadata: both lowerers' stop locations and variable
tables, compiled again on demand and verified, which give a stopped frame
its name, its place and its named variables ("Reading the stack at a
stop", "Compiling the metadata again"); stage 3: line breakpoints
(pending ones included), stepping with its filters, run to cursor, method
breakpoints and exception breakpoints, and the breakpoint and step stress
modes ("Where the original lives" to "Step into's held request",
"Exceptions", "Method breakpoints", "The return value of the last
call"); and stage 4: the DAP agent in `ns-cli`, behind `--debug` and
`--debug-connect` on `run` and `load`, hit counts counted by the VM, the
breakable lines the language server answers, and the plugin's
`XDebugProcess` with its three breakpoint types, from which the debugger
is usable in the IDE ("Launching, the start and the end", "Hit counts and
temporary breakpoints", "A breakpoint names a source position", "The
JetBrains plugin"); and stage 5: evaluation, conditional breakpoints and
logpoints, watches and setting variables, marking objects, smart step
into, restart frame and force return, the memory view and folding the
shipped frames, in the VM, the agent and the plugin ("Evaluation",
"Writing a frame", "Marking objects", "Smart step into a block",
"Restart frame and force return", "The memory view", "Folding the
shipped frames", "The JetBrains plugin"), and the MCP server ("The MCP
server"). The stages keep their numbers, which the text above refers
to.

6. Hot swap, then edit-and-continue of running activations;
   `ActivationMirror` for programs.

After stage 6: attaching to an `ns-next` started without `--debug`,
through a trigger of each platform's -- a signal on POSIX, a named event
on Windows -- that starts the agent, as the JVM's Attach API does, and an
authentication that has no secret from the launch (decided with the user
on 2026-11-16; its own questions are decided then).
