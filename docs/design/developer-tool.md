# The developer tool: tracing, printing code, a shell

What V8's `d8` is to V8, `ns-next` becomes to this runtime: the binary that
runs a program also says why the tiers did what they did, prints the code
they made, and evaluates Newspeak interactively. The question it answers --
why a loop stayed in the interpreter, why an optimized copy was retired,
why a send went megamorphic -- is today answered only by a counter or a
print added for the occasion.

**Partial.** Stage 1, tracing, is built ("Stage 1: tracing"): `ns-next
run --trace=<categories>` writes a line on standard error for every
decision of the tiers and the caches, with its reasons. Printing the
code a run makes and the shell are designed here and built as stages 2
and 3, in that order ("Planned: the stages"); stage 2 is next. The
profiler is not part of it ([OPEN-WORK.md](../OPEN-WORK.md), "A
profiler").

## What exists

- `ns-next-analyzer disasm` and `inspect --full` print bytecode, compiling
  or decoding without running anything (`bytecode::disassemble`;
  [compiler.md](compiler.md), "Reading the output").
- `ns-next run` and `bench` take `--cache-statistics`,
  `--collector-statistics` and `--jit-statistics`, each one line on
  standard error after the run (`crates/ns-cli/src/entry.rs`). The cache
  statistics are a scan of the sites on demand, the collector's and the
  JIT's plain counters; none of them says *why*.
- The trace says why ("Stage 1: tracing"): every decision of the JIT --
  a baseline compile, its refusal and its hold, on-stack replacement and
  its refusal, an abandonment, a revival and a refusal instead, an
  optimization requested, put off, dropped, installed or refused, where
  `find_recompilee` stopped walking, every callee the inlining plan took
  or declined and why, a copy retired or invalidated, machine code freed
  and the zone compacted -- and every inline cache transition and
  vacating of every cache is a typed event with its reasons as enums
  (`ns_runtime::trace_events`), written as one line each by `ns-next run
  --trace`, by `debug_start`'s `trace` and by the plugin's runtime
  options.
- `ns_runtime::tool_names` is the one grammar of the tools' names,
  `<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>`, and
  `tool_names::KnownModule::resolve` the one lookup of a module a name
  gives: the DAP agent's method and exception breakpoints and evaluation
  -- which the MCP server's locations reach through the agent -- and the
  trace filter read names through them, so that a refusal reads the same
  in every tool.
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
covers every version; [modules.md](modules.md), decision 8), with `...`
ending a prefix (`Demo/Account>>de...`, `com.acme...`; decided with the
user on 2026-11-20), as V8's filters
(`v8/src/utils/utils.cc:225-265`) and HotSpot's `CompileCommand=print`
(`jdk/src/hotspot/share/compiler/compilerOracle.hpp:52-68`) select one
method. Their `*` cannot be the mark here: `*`, `**` and `*=` are binary
selectors (§4.2, PDF p. 20), so `Demo>>*` names the method `*`, while
`...` occurs in no name -- a selector holds no `.`, and an id has no
empty segment. The filter's flag is given once for each pattern, never
a comma-separated list, since `,` is a binary selector too and
`newspeak.core.Kernel/String>>,` a method a pattern must name (decided
with the user on 2026-11-20). Beside it, the debugging API answers the code of a stopped frame.

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
`[inlining] {Demo>>main:args:} declined in-body={Demo>>main:args:} callee={[] in Demo>>run} reason=over-weight at=31` or
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

## Stage 1: tracing

**Built** (2026-11-18 to 2026-11-20;
`docs/superpowers/plans/2026-11-18-2153-developer-tool-stage-1-tracing.md`).
`ns-next run --trace=<categories>` writes one line on standard error for
every decision of the tiers and the caches in the categories named, with
its reasons; `debug_start` and the plugin's run configuration pass the
same flags on.

### The runtime's half

`ns_runtime::trace_events` (`crates/ns-runtime/src/trace_events.rs`,
with `trace_events/names.rs` and `trace_events/filter.rs`; not `trace`,
which is the collector's edge enumeration) declares the events and
formats nothing (decision 2). A host installs a sink with
`Interpreter::set_trace(categories, filter, sink)`: a `TraceCategories`
set of `TraceCategory`s, a `TraceFilter` and a `Box<dyn TraceSink>`,
whose `event` receives a `TraceEvent { category, body, kind }` -- `body`
the name of the body the event is about, `None` for one about no body,
`kind` a `TraceEventKind` whose reasons are enums, each with a `name()`
in kebab case, the spelling of its line. Installing replaces the sink
before and drops what it had not received; no category is no tracing.

- **The switch.** The tracer (`EventTracer`, `JitState::tracer`) holds
  one bit per category, and every site tests its category's bit
  (`EventTracer::traces`, `#[inline]`) before it builds anything. Every
  site is cold -- a compile, an optimization, a retirement, an
  invalidation, a free, a compaction, a cache miss's transition; no
  template, stub or leaf of machine code changed for it, and the
  `codegen` gate's budgets held unchanged. Nothing of it is
  `cfg(debug_assertions)`: a release build traces (decision 3). A target
  without a JIT (`cfg(not(ns_jit))`) records only `caches`.
- **Pending events.** A site holds only the JIT's state, so it records
  a pending event (`EventTracer::record`): the bodies it names as
  `Rc<CompiledCode>`, a copy's key as its `CustomizationKey`, a selector
  as its `Symbol`, and the receiver of a cache transition as its class's
  qualified name, a `String` computed when it is recorded. No pending
  event holds a handle, so no collection between its record and its
  delivery moves what it names, and it keeps no object of the program
  alive ("A handle means nothing against a `Heap` that did not mint it",
  `CLAUDE.md`).
- **Delivery.** A flush (`trace_events::flush`, cold) names the bodies
  and keys, applies the filter and delivers each event in the order it
  was recorded: at the end of every compile trigger the interpreter
  makes (`jit::after_compile_trigger`), at the run loop's safepoint --
  on its way in and again on its way out, whichever way it ends, so
  that what the safepoint itself recorded (an optimization, a
  compaction, a collection's frees and invalidations) never outlives it
  -- when a host's run returns, and before a host method that records
  outside a run returns (`set_jit_enabled`, `set_jit_zone_bytes`,
  `compact_jit_zone`, `invalidate_caches`), so that no event waits for a
  later run. A copy's
  key whose id a full collection reclaims is named in the collection,
  before the id is freed (`trace_events::name_freed_keys`): named at the
  flush, the id would be no class's, or another class's that was minted
  it meanwhile. **A leaf of machine code only records**
  (`jit::after_compile_trigger_in_leaf`): a flush runs the host's sink,
  which must not run with machine code's frames on the native stack, so
  the sink is called outside machine code only. A leaf's events wait
  for the next safepoint or host return the run reaches on its own, and
  no safepoint is requested for them: tracing must not change what it
  observes, and a requested safepoint would make machine code exit only
  when tracing. So a leaf's events are lost when the process reaches
  neither -- killed while machine code loops without a poll that
  safepoints, or ended from the DAP agent's own thread, as a
  `disconnect` that terminates the debuggee ends it, before the run
  reaches either. Between two safepoints the leaves
  record at most a compile per body and per twin and three transitions
  per send site; a site passes its transitions again only after an
  invalidation vacated it, and a body compiles again only after a
  revival, a compaction or a replaced zone freed its code, each of which
  passes a safepoint or a host's return, which delivers the queue -- so
  it never grows with the run.
- **Names.** A body is named as the debugger names a frame
  (`debugger/stack.rs`'s `frame_name`), its owner by `qualified_name`
  (`debugger/metadata/again.rs`), the module's id and version, `/` and
  the classes inside it ([modules.md](modules.md), decision 8):
  `Demo>>factorial:`, `Demo/Account>>deposit:`, `Demo/Account
  class>>named:` (a factory's too), `[] in Demo>>main:args:`,
  `Demo/Account initializer`, `Demo/Account superclass clause`,
  `com.acme.util.Util@1.0.0/Inner>>run`,
  `newspeak.core.Kernel/Array>>do:`. `BodyNames`
  (`trace_events/names.rs`) caches each name beside a `Weak` of its body
  and, at a miss, indexes the bodies of every mixin it has not indexed
  yet -- none while the mixin tables' generation stands, so that each
  mixin is walked once in a run whose nested classes register one by
  one -- methods,
  factory entries, the three initializer bodies, the superclass clause
  and every block they create, transitively, the mixins an edit
  superseded included, so that a body an edit replaced keeps its name. A
  body no mixin holds is `(unnamed)`. A copy's key is named by two
  fields, `key=` the receiver's class and `entered=` the class the body
  was entered at, the latter only where it is another, both absent for a
  body optimized as one copy; `#<id>` where no application holds the
  dispatch id any longer. A cache transition's receiver whose class has
  no declaration -- the VM mirror's -- is named by its class's display
  name, unqualified.

### The events and their lines

Each event is one line (decision 8; `crates/ns-cli/src/trace.rs`'s
`line`, every event's line pinned by `every_event_has_its_line`):
`[<category>] {<body>} <event>`, then its fields in a fixed order --
`key=` and `entered=` first where present, `at=` last for the inlining
plan's -- every name in braces, every reason in kebab case, `{}` for an
event about no body.

| Category | Line after the body | What it says |
|---|---|---|
| `compile` | `compiled trigger= bytes=` | Baseline code installed (`jit::compile_into_slot`). `trigger=entry` an entry the interpreter makes -- a push, an initializer's invocation, a future it forced; `call` one machine code makes -- the miss leaf, the initializer leaf, the forcing leaf (`note_entry_by_call`); `back-jump` the 40th interpreted backward jump. |
| `compile` | `compiled-twin bytes=` | The body's reifying twin installed (`jit::compile_twin`). |
| `compile` | `refused trigger= reason=` | No baseline code, for good but for `zone-full`, after which the body counts its entries again: `jit-off`, `too-many-instructions`, `unassembled`, `zone-full`, `too-large`, `exits-at-once` (`CompileRefusal::from_refusal`). |
| `compile` | `held trigger= reason=` | A body that came due was not tried: `zone-full`, the exhausted zone is known to have no room for it (`jit::hold_compile_of`). Its first hold since it was last compiled only, not every entry the zone holds it at. |
| `compile` | `held-twin reason=` | A frame needed the body's twin, which the exhausted zone holds likewise; the frame goes on interpreted. |
| `compile` | `abandoned exits=` | The body's code spent its exit budget and was freed (`jit::abandon`). |
| `compile` | `revived by=` | An abandoned body counts its entries again: `decay` (`JitState::decay_exits`) or `entries` (`note_abandoned_entry`). |
| `compile` | `refused-for-its-twin` | Refused for good, its code freed, since its twin was refused (`jit::refuse_for_its_twin`). |
| `compile` | `refused-abandoned reason=` | An abandoned body refused for good rather than revived: `jit-off`, the JIT switched off (`JitState::refuse_abandoned`). |
| `osr` | `osr-compiled at=` | Compiled at an interpreted backward jump to the loop's head `at=` (`jit::back_edge_ready`). |
| `osr` | `osr-entered at=` | An interpreted frame switched into the body's code there (`jit::at_back_edge`). |
| `osr` | `osr-refused at= reason=` | A frame of a body with machine code did not switch at the loop's head `at=`, and tries again after as many back jumps as compile a body (`jit::back_edge_ready`, `jit::at_back_edge`): `native-stack-full`, `no-machine-code`, `other-feedback-base` (the frame runs against another feedback base than the code), `twin-held` (a reified frame enters the twin, which the exhausted zone holds), `no-entry`. A twin refused for good is the body's `refused-for-its-twin` instead, its code freed, and a frame after it `no-machine-code`. Once per body, loop head and reason, until a frame switches there or the body is compiled again (`EventTracer::first_osr_refusal`), so that a frame that can never switch is one line, not one per wait. |
| `optimize` | `requested key= entered= spent-by={} walk-end=` | The body, the recompilee, is requested an optimization: `spent-by`'s budget ran out and the walk up the stack (`jit::find_recompilee`) stopped here for `bottom-of-stack`, `no-frames-between`, `search-length`, `inlined-into-a-copy-below`, `not-recompilable` or `not-inlined`; `not-walked` for the optimize stress mode's request of the spending frame's own copy. |
| `optimize` | `not-due key= entered= spent-by={} walk-end=` | The walk found the body, but its copy for the key was installed or refused meanwhile: nothing is requested, and `spent-by`'s budget stays parked until the next safepoint arms it. |
| `optimize` | `dropped key= entered= cause=` | A pending request dropped before the safepoint compiled it, since what it would be compiled against changed: `edit`, `reclaimed-id` (`drop_stale_requests`); the budget is armed afresh. |
| `optimize` | `deferred key= entered= reason=` | The optimization put off, the budget armed afresh: `patched` (a breakpoint stands in the body), `zone-full`. |
| `optimize` | `optimized key= entered= bytes= inlined-scopes= inlined-sites= type-case-arms= eliminated-blocks=` | The copy installed. |
| `optimize` | `refused key= entered= reason=` | The copy refused for good, the reason a `CompileRefusal`'s. |
| `inlining` | `inlined key= entered= in-body={} callee={} at=` | The plan inlined `callee` at the send at `at=` of the scope running `in-body`. |
| `inlining` | `declined key= entered= in-body={} callee={} reason= at=` | The plan sends the callee instead (`callee={}` where it is no body): `not-snapshotted` (the tree holds no body of it -- a method past the limits where it is sent, a nested class's accessor, a factory, a `doesNotUnderstand:` target), `not-expressed` (a leaf or primitive the graph does not express), `returns-otherwise`, `unknown-lazy-field`, `observed`, `reaches-past-the-compile`, `block-escapes`, `creates-a-block`, `over-weight`, `over-compile-limit`, `over-recursion-limit`, `out-of-registers`. |
| `deoptimize` | `retired key= entered= reason= version=` | The copy retired for `traps` or `exits` (`jit::retire_optimized`), its version raised to `version=`, or `none` when it is refused for good. |
| `deoptimize` | `invalidated key= entered= cause=` | The copy freed since what it was compiled against changed: `edit`, `bulk-forward`, `reclaimed-id`, `moved-feedback-base`, `breakpoint`, `stress` (the invalidate stress mode's round). |
| `code-zone` | `freed key= entered= tier= cause=` | A body's `baseline` or `optimized` code freed, an optimized copy's key named as the optimizing tier's lines name it (`zone.rs`'s `mark_free`): `compaction`, `dead-body` (by a full collection; about no body, since its code died), `moved-feedback-base`, `breakpoint`, `abandoned`, `refused-for-its-twin`. A retired or invalidated copy's free is its `deoptimize` event alone. |
| `code-zone` | `compacted freeing= bodies-freed=` | About no body: the zone compacted, freeing `to-a-quarter` or `every-inactive-body`, each body freed also its own `freed` event. |
| `code-zone` | `replaced bodies-freed=` | About no body: the zone replaced -- its size set afresh, the JIT switched on or off -- dropping the code of `bodies-freed` bodies at once, none of them its own `freed` event. |
| `caches` | `<before>-><after> selector=#<selector> site= receiver={}` | A miss moved a site `vacant->monomorphic`, `monomorphic->polymorphic` or `polymorphic->megamorphic` (`InlineCaches::advance` answers the transition; `global_found`, `decided_miss` and `fill` record it), the miss leaf's included; the body is the root of the family the site is in and `site=` its number within the family's range, `{}` for a site of no family's, a closure's an image decoded. A refill and a megamorphic site record nothing. |
| `caches` | `vacated cause=` | About no body: every site, polymorphic block and the global cache vacated at once (`inline_cache::vacate_every_cache`), so that the `vacant->` transitions after it have their reason: `edit`, `bulk-forward`, `breakpoint` (a method's fast path switched), `jit-switched`, `zone-replaced`, `host` (`Interpreter::invalidate_caches`), `stress` (the cache stress mode's safepoint). |

For example:

```
[compile] {Demo>>square:} compiled trigger=call bytes=1216
[osr] {newspeak.core.Kernel/Number>>to:do:} osr-compiled at=3
[optimize] {Demo/Account>>deposit:} requested key={Demo/Account} spent-by={[] in Demo>>main:args:} walk-end=not-inlined
[inlining] {Demo>>main:args:} declined in-body={Demo>>main:args:} callee={[] in Demo>>run} reason=over-weight at=31
[deoptimize] {Demo>>run} retired reason=exits version=none
[code-zone] {} compacted freeing=to-a-quarter bodies-freed=7
[caches] {Demo>>main:args:} monomorphic->polymorphic selector=#deposit: site=3 receiver={Demo/Account}
```

The inlining plan's decisions are collected by every optimizing compile
(`optimizing::compile_tree_explained` answers them, `PlanDecisions`,
beside the copy or its refusal) and traced whether the copy is installed
or refused after planning; a callee dropped late -- a block that escapes
after all, a scope that would create a block -- is declined at its
sender's send, and one dropped only with its parent, which lies in no
scope of the copy, is not reported.

### The filter

`TraceFilter` (`trace_events/filter.rs`) delivers the events of the
bodies one of its patterns selects, and every event about no body. A
pattern is written in the tools' method syntax,
`<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>`, read by the
tools' one grammar (`tool_names`); a trailing `...` makes it a prefix:
`Demo/Account>>...` and `Demo/Account>>de...` (a class's selectors),
`Demo/Account class>>...` (its class side), `Demo/Acc...` and
`Demo/...` (classes inside a module), `com.acme.util.Util@1....` (a
version prefix), `Demo...` and `com.acme...` (an id prefix). Never `*`:
`*`, `**` and `*=` are binary selectors, so `Demo>>*` names the method
`*` (decision 5). A pattern is matched against a body's name by parts --
the module's id and version, the path inside it, the side and the
selector -- so that a pattern without a version covers every version
(`ModuleName::covers`), a block's event matches through its `[] in `
owner, and an initializer or a superclass clause, having no selector,
only by the class forms. A pattern is one pattern, never split at a
comma: `,` is a binary selector, and `newspeak.core.Kernel/String>>,` a
method a pattern must be able to name. `TraceFilter::parse` refuses a
pattern not of the form, `TraceFilter::check` one whose module no module
of the program or the binary has, through
`tool_names::KnownModule::resolve`, as the debugger refuses a method
breakpoint's name; each refusal names the pattern.

### The hosts

- **`ns-next run`** (`crates/ns-cli/src/trace.rs`, `commands/run.rs`)
  takes `--trace=<categories>`, comma-separated category names or `all`,
  as HotSpot's `-Xlog` takes its tags rather than V8's flag per kind,
  and `--trace-filter=<pattern>`, once for each pattern, which clap
  refuses without `--trace`. An unknown category and a malformed pattern
  are refused before the program is read, and a pattern naming a module
  neither the program nor the binary has once the program is loaded
  (`program::known_modules`, the list the DAP agent's
  `Sources::module_of` reads too), before the application is assembled
  -- each with exit status 2, nothing run. The sink
  (`StandardErrorSink`) writes each line and its newline in one write on
  standard error, so that the program's own output to standard error
  never tears a line; a failed write is ignored, so that a closed
  standard error never stops the program. Only `run` takes the flags;
  `load` and `bench` do not.
- **`debug_start`** takes `trace`, `--trace`'s value, and `trace_filter`,
  an array of patterns, each passed as one `--trace-filter`, both before
  the program; the lines arrive with the program's output in every
  answer ([debugger.md](debugger.md), "The MCP server"; decisions 7 and
  9).
- **The plugin's run configuration** takes runtime options, ns-next's
  own options placed after the subcommand and `--debug-connect` and
  before `--script` ([editor-support.md](editor-support.md), "The
  project file and running projects"; decision 9).

### Tests

`crates/ns-runtime/tests/trace.rs` runs programs with a collecting sink,
as `tests/jit.rs` does (`#![cfg(ns_jit)]`, the JIT on, a stress mode
that would defeat what a test pins switched off by its debug-only
setter), and names the events that must occur, never a complete
sequence, so that a changed threshold breaks only what it changes; the
plan's declines are unit tests in `jit/optimizing/plan.rs`, the filter's
and the names' in their modules. The line format is pinned by
`crates/ns-cli/src/trace.rs`'s unit tests, `--trace` and its refusals by
`crates/ns-cli/tests/cli.rs`, `debug_start`'s trace by
`crates/ns-cli/tests/mcp.rs` (`a_trace_arrives_with_the_output`) and the
runtime options by the plugin's `NewspeakRunConfigurationTest`; the
CLI's and the MCP server's tests strip every stress mode from the child
and answer nothing on a target without a JIT.

## Planned: the stages

### Stage 2: printing code

- `--print-bytecode`, `--print-code` (baseline and optimized, at install)
  and `--print-inlining` (the plan as a tree of scopes, arms and declined
  sites with stage 1's reasons), all selected by `--print-filter`, whose
  patterns are the trace filter's (`--print-filter` given once for each
  pattern, a prefix ending in `...`). Stage 1 leaves what this builds on:
  the bodies' names (`trace_events/names.rs`), the filter
  (`TraceFilter`), the plan's decisions with their reasons
  (`PlanDecisions`, collected by every optimizing compile) and the events
  of an install (`compiled`, `compiled-twin`, `optimized`), at whose
  sites a listing is taken.
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
