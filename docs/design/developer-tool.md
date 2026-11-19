# The developer tool: tracing, printing code, a shell

What V8's `d8` is to V8, `ns-next` becomes to this runtime: the binary that
runs a program also says why the tiers did what they did, prints the code
they made, and evaluates Newspeak interactively. The question it answers --
why a loop stayed in the interpreter, why an optimized copy was retired,
why a send went megamorphic -- is today answered only by a counter or a
print added for the occasion.

Status: the static half exists (below, "What exists"); tracing, printing
the code a run makes and the shell are designed here and built in three
stages, in that order ("Planned: the stages"). The profiler is not part of
it ([OPEN-WORK.md](../OPEN-WORK.md), "A profiler").

## What exists

- `ns-next-analyzer disasm` and `inspect --full` print bytecode, compiling
  or decoding without running anything (`bytecode::disassemble`;
  [compiler.md](compiler.md), "Reading the output").
- `ns-next run` and `bench` take `--cache-statistics`,
  `--collector-statistics` and `--jit-statistics`, each one line on
  standard error after the run (`crates/ns-cli/src/entry.rs`). The cache
  statistics are a scan of the sites on demand, the collector's and the
  JIT's plain counters; none of them says *why*.
- The JIT's decisions have their sites, but few of their reasons are
  values: a baseline compile's refusal is (`jit::Refusal`), while where
  `find_recompilee` stops walking, why the planner declines a callee
  (`Planner::take` returns), why an optimization fails (its `Err(_)` is
  dropped) and why a copy is retired or invalidated are decided by control
  flow and recorded nowhere.
- Every inline cache transition happens in one function,
  `InlineCaches::advance` (`interpreter/inline_cache.rs`), machine code's
  miss leaf included.
- A body's machine code can be read out of the zone (`CodeWriter::bytes`)
  and mapped to its instructions (`ProgramCounterMap`).
- The debugger evaluates an expression as a block over a stopped frame
  ([debugger.md](debugger.md), "Evaluation"); nothing evaluates without
  one. `FrameKind::TopLevel` exists for a top-level activation, which only
  tests push.

## The decisions

Decided with the user on 2026-11-18.

**1. One design, three stages: tracing, then printing code, then the
shell.** Tracing first, because it turns the reasons above into types,
which printing the inlining plan needs as well; the shell is the largest
and the most independent piece.

**2. The runtime reports typed events to a sink the host installs; the
host formats them.** `ns-runtime` stays free of output formats, as it is
for the statistics: an event is a Rust value whose reasons are enums the
compiler holds exhaustive, and `ns-next` writes each as one line of a
stable format on standard error, while the debugger's agent forwards the
same events to its client. Both of V8's channels exist for this reason --
`--trace-opt`/`--trace-deopt` as text at the event's site
(`v8/src/execution/tiering-manager.cc:120-130`) and `--log-ic`, a record
per transition with its old and new state
(`v8/src/logging/log.cc:2037-2064`), read by tools
(`v8/tools/system-analyzer/processor.mjs:177-183`); HotSpot moved its JIT
traces into unified logging, one mechanism with tags
(`jdk/src/hotspot/share/compiler/compileTask.cpp:500-527`). Here the one
typed channel serves both the line on standard error and the agent.

**3. Everything is in the release build.** Performance is judged only on a
release build, so the question why a body was not optimized has to be
answerable there. HotSpot's `PrintCompilation` is a product flag
(`jdk/src/hotspot/share/runtime/globals.hpp:588`), Cog's trace bits and
print functions are compiled into its production VM
(`opensmalltalk-vm/platforms/unix/vm/sqUnixMain.c:1702-1716`), and
Strongtalk keeps `PrintCompilation` among its product flags
(`strongtalk/vm/runtime/debug.hpp:220`); V8 freezes `--print-code` in a
release build (`v8/src/flags/flag-definitions.h:4210-4231`), which is the
choice not taken. Every event site is cold -- a compile, an optimization,
a retirement, a cache miss -- and tests one flag; the hot path's budget is
held by the `codegen` gate as before.

**4. Machine code is decoded by `iced-x86` and `yaxpeax-arm`, normal
dependencies of `ns-cli` only.** The runtime hands out a body's bytes with
their annotations -- each instruction's bytecode index from the
`ProgramCounterMap`, the name of every stub and leaf a call reaches, the
extent of the data words -- and `ns-next` decodes them. The two decoders
are the ones every encoding of `ns-assembler` is already checked against
(dev-dependencies today), so a listing agrees with the encoder's tests by
construction; a decoder of our own would repeat them, and `ns-runtime`
keeps no external dependency. The reason the dependency survives review:
without it machine code cannot be read at all. V8 writes its own decoders
(`v8/src/diagnostics/x64/disasm-x64.cc`), HotSpot loads the external
`hsdis` plugin at run time and falls back to hex without it
(`jdk/src/hotspot/share/compiler/disassembler.cpp:785-878`), Cog and
Strongtalk ship none in the VM.

**5. Code is printed when it is installed, selected by a filter.** Every
compile is printed once, as it is made -- a recompilation and a
replacement copy included, which a dump at the end would miss -- as V8's
`--print-opt-code` and HotSpot's `PrintAssembly` do. The filter names
bodies in the syntax every tool names a method by
(`id[@version][/Inner…][ class]>>selector`: `Demo/Account>>deposit:`,
`com.acme.json.Json@2.1.0/Parser class>>on:`; a name without a version
covers every version; [modules.md](modules.md), decision 8), with `*`
ending a prefix, as V8's filters
(`v8/src/utils/utils.cc:225-265`) and HotSpot's `CompileCommand=print`
(`jdk/src/hotspot/share/compiler/compilerOracle.hpp:52-68`) select one
method. Beside it, the debugging API answers the code of a stopped frame.

**6. The shell evaluates against a workspace object, as the Newspeak IDE
does.** `ns-next shell [program]` runs in the process, as `d8`'s shell does
(`v8/src/d8/d8.cc:5909-5950`). An input is a code body evaluated with a
workspace object as its receiver: `platform` is reachable, a temporary
declared at the input's top lives on as a workspace variable, and with a
program given its module declaration is reachable by its name. The
canonical implementation's workspace is a generated class whose slots are
the workspace variables
(`newspeak/WorkspaceManager.ns:123,186-210`), and an expression is
compiled in its receiver's mixin and run against it
(`primordialsoup/newspeak/Mirrors.ns:1585-1622`;
`newspeak/Newspeak2SqueakCompilation.ns:2217-2262`). §3.8 (PDF pp. 19-20)
defines top-level expressions, run in a top-level activation without an
enclosing object, so with no `self` and no implicit receiver send, not
even `platform`; §8.1 (PDF p. 46) puts the IDE's evaluation of
expressions and statements beside them, and §8.3 (PDF p. 47) names
`platform` in a workspace. The shell is that evaluation, not a top-level
expression, so it deviates from nothing.

**7. Each stage reaches the agent and the IDE as it is built.**
`debug_start` takes the trace categories and answers their lines with the
program's output (decision 9); a new MCP tool answers a stopped frame's
bytecode and machine code through a request of the agent; the shell needs
no tool of its own, since `debug_evaluate` evaluates at a stop
([debugger.md](debugger.md), "The MCP server").

**8. A trace event is one line: `[<category>] {<body>} <event>`, then
`name=value` pairs, every name in braces.** For example
`[compile] {Demo>>factorial:} compiled trigger=entry bytes=412`,
`[inlining] {Demo>>main:args:} declined callee={[] in Demo>>run} reason=over-weight at=31` or
`[caches] {Demo>>main:args:} monomorphic->polymorphic selector=#deposit: site=3 receiver={Demo/Account}`;
`{}` stands for an event that names no body, every reason is spelled in
kebab case, and no line carries a time, so that a test pins it. A name
holds spaces (`[] in Demo>>main:args:`, `Demo/Account class>>named:`)
and nearly every punctuation character, since a binary selector may be
made of `+ - / \ * ~ < > = @ % | & ? ! ,` (§4.2, PDF p. 20), but never
a brace: the braces delimit it, so that a line splits at the spaces
outside them, for a script as for the eye (decided with the user on
2026-11-19). It is
V8's `--trace-*` kind of line and HotSpot's unified logging's, a category
first; HotSpot's `PrintCompilation` columns
(`jdk/src/hotspot/share/compiler/compileTask.cpp:187-300`) fit one event
kind, not seven, and JSON lines, V8's `--log-*` kind, are for a tool the
typed event already serves (decided with the user on 2026-11-18).

**9. The lines reach the agent and the IDE as the process's standard
error.** `--trace` is a flag of `run`, as the statistics are; the MCP
server collects a child's standard error with its output already, so
`debug_start` passes the flag on, and the plugin's run configuration
takes runtime options for the same reason. The DAP agent is not
involved: it still writes nothing to the program's streams, and no trace
line is dropped by its bounded connection. A channel of their own over
DAP stays possible, since the events are typed (decided with the user on
2026-11-18).

## Planned: the stages

### Stage 1: tracing

- **The events.** `ns_runtime::trace_events` declares `TraceEvent` and
  the reason enums the decisions above turn into values (`trace_events`,
  since `trace` is the collector's edge enumeration):
  - a baseline compile, its trigger (entry, call, back jump) and its
    `Refusal`;
  - on-stack replacement compiled and switched into;
  - a body abandoned for its exits, revived by decay or by its counted
    entries;
  - an optimization requested (the budget spent, the recompilee and why
    the walk up the stack stopped there), installed or refused (why);
  - the inlining plan's decisions, each declined callee with its reason
    (observed, reaching past the compile, over the weight, the compile
    limit or the recursion limit, out of registers);
  - an optimized copy retired (traps, exits; its version raised or
    refused for good) or invalidated (an edit, a bulk forward, a reclaimed
    id, a moved feedback base, a breakpoint's patch, the invalidate
    stress mode's round), each copy named by its key;
  - machine code freed, per body and tier, by a compaction, a full
    collection (a dead body, a moved feedback base), an abandonment for
    its exits, a refusal for good once its twin was refused, or a
    breakpoint's patch -- a retired or invalidated copy's free is its
    `deoptimize` event alone -- each compaction summed up, how it freed
    and how many bodies, and a replacement of the zone with the number of
    bodies it dropped, both naming no body;
  - an inline cache's transition, from its old to its new state
    (vacant, monomorphic, polymorphic, megamorphic), with the send site
    and the receiver's application.
- **No handle leaves the call.** An event names a body as the debugger
  names a frame -- its owner by `qualified_name`
  (`debugger/metadata/again.rs`), `id@version/Inner>>selector` -- and a
  receiver application by its class's qualified name, computed when it
  is recorded; a pending event holds the code it names as
  `Rc<CompiledCode>` and plain values, so no collection between its
  record and its delivery can move what it names, and it keeps no
  object of the program alive ("A handle means nothing against a `Heap`
  that did not mint it", `CLAUDE.md`; decided with the user on
  2026-11-19).
- **The switch.** One flag per category, read at the cold site; installing
  a sink sets them (`Interpreter::set_trace`).
- **`ns-next run`** takes one flag, `--trace=<categories>` (`compile`,
  `osr`, `optimize`, `inlining`, `deoptimize`, `code-zone`, `caches`, or
  `all`), as HotSpot's `-Xlog` takes its tags, rather than V8's flag per
  kind (decided with the user on 2026-11-19), and
  `--trace-filter=<selection>`; each event is one line on standard
  error (decision 8), the format pinned by tests.
- **`debug_start`** takes `trace` and `trace_filter` and passes them to
  the child; the plugin's run configuration takes runtime options placed
  before the program (decision 9).
- **Pending events.** A site that holds only the JIT's state records the
  event with the bodies it names as `Rc<CompiledCode>`; the bodies are
  named and the filter applied at a flush -- at the end of every compile
  trigger the interpreter makes, at the run loop's safepoint and when a
  host run returns, never in a leaf of machine code, so that no host code
  runs with machine code's frames on the native stack; a leaf's events,
  bounded by the program (a compile per body and twin, a few cache
  transitions per site), wait for the next safepoint the run reaches on
  its own, since a requested one would make machine code exit only when
  tracing -- so that no site needs the heap and the queue never grows
  with the run.

### Stage 2: printing code

- `--print-bytecode`, `--print-code` (baseline and optimized, at install)
  and `--print-inlining` (the plan as a tree of scopes, arms and declined
  sites with stage 1's reasons), all selected by `--print-filter`.
- The runtime answers a listing: the body's name, its tier and, for an
  optimized copy, its receiver application; its bytes and base address;
  the bytecode index per machine offset; the named targets of its calls.
  `ns-next` decodes it for the target it runs on, x64 or AArch64; a target
  without a JIT prints bytecode only.
- The agent answers a request for the code of a stopped frame -- the
  body's bytecode, and the machine code the frame runs if any -- and the
  MCP server's new tool asks it.

### Stage 3: the shell

- `ns-next shell [program]`: reads inputs from standard input, an input
  continuing while the parser finds it incomplete; prints each value's
  `printString`, or the error, and goes on.
- The workspace: an instance whose class holds a slot per workspace
  variable and `platform`; with a program given, the module declaration by
  its name.
- An input runs under the depth limit and its reserve like any program
  code, and an unhandled error ends the input, not the shell.

## Open questions

Each is decided in its stage's plan.

- **Stage 2:** when a body's bytecode counts as made for
  `--print-bytecode` (its mixin's compile, or its first entry); how an
  optimized copy's inlined scopes are marked in its listing.
- **Stage 3:** how an input is compiled -- a method on the workspace's
  mixin, as the Newspeak IDE does, or a block over a top-level activation
  (`FrameKind::TopLevel`) -- and how a new workspace variable becomes a
  slot (an atomic edit through `interpreter/edit.rs` and `install.rs`, or
  a workspace class rebuilt); whether earlier results are named (`r1`,
  `r2`, as `newspeak/AI_IDE_Support.ns:2658-2659` does); how an
  interrupt stops a running input; line editing without a new dependency.
