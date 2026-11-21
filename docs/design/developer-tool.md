# The developer tool: tracing, printing code, a shell

What V8's `d8` is to V8, `ns-next` becomes to this runtime: the binary that
runs a program also says why the tiers did what they did, prints the code
they made, and evaluates Newspeak interactively. The question it answers --
why a loop stayed in the interpreter, why an optimized copy was retired,
why a send went megamorphic -- is today answered only by a counter or a
print added for the occasion.

**Partial.** Stages 1 to 3 are built. Stage 1, tracing ("Stage 1:
tracing"): `ns-next run --trace=<categories>` writes a line on standard
error for every decision of the tiers and the caches, with its reasons.
Stage 2, printing code ("Stage 2: printing code"): `--print-bytecode`,
`--print-code` and `--print-inlining` print the bytecode a run makes,
the machine code it installs and the inlining plan of every optimizing
compile, and the debugger answers a stopped frame's code
(`nsNext/frameCode`, `debug_code`). Stage 3, the developer tool in the
JetBrains plugin ("Stage 3: the developer tool in the IDE"): `ns-next
run --trace-file=<path>` writes the trace and the listings as typed
JSON lines, each body described once with its source positions; the
plugin switches them on in the run configuration's "Developer Tool"
tab and through a project-wide list of watched methods, reads the file
live in Run and Debug, and shows it in the "Newspeak JIT" tool window
-- Methods, Events, Code, Inlining --, as state icons in the gutter,
inlay hints at the sends, editor tabs of the code and, at a stop, the
frame's code. The shell is stage 4 ("Planned: the stages"). The profiler
is not part of it ([OPEN-WORK.md](../OPEN-WORK.md), "A profiler").

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
  --trace` and by `debug_start`'s `trace`, and as one record each into
  the file of `--trace-file`, which the plugin reads.
- `ns_runtime::tool_names` is the one grammar of the tools' names,
  `<id>[@<version>][/<Inner>(.<Inner>)*][ class]>><selector>`, and
  `tool_names::KnownModule::resolve` the one lookup of a module a name
  gives: the DAP agent's method and exception breakpoints and evaluation
  -- which the MCP server's locations reach through the agent -- and the
  trace filter read names through them, so that a refusal reads the same
  in every tool.
- The listings print the code ("Stage 2: printing code"): every body's
  bytecode when its mixin is made, every install's machine code --
  baseline, twin and optimized, an optimized copy with its tree of
  inlined scopes -- annotated with the bytecode each stretch lowers and
  what each call or jump out of the body reaches, and every optimizing
  compile's inlining plan with its outcome, written by `ns-next run
  --print-bytecode`, `--print-code` and `--print-inlining` between a
  head line and `--- end`, or as records into `--trace-file`'s file; and
  a stopped frame's bytecode and machine code, each marked where the
  frame stands, through the debugger (`Command::FrameCode`, the agent's
  `nsNext/frameCode`, the MCP server's `debug_code`, the plugin's "Show
  Frame Code").
- The IDE shows them beside the source ("Stage 3: the developer tool in
  the IDE"): the plugin passes a run the trace categories, listings and
  filters its configuration and the watched methods ask for, with a
  trace file of its own, reads every record as it is written, and shows
  each traced run in a tab of the "Newspeak JIT" tool window and in the
  editor's gutter and inlay hints.
- The debugger evaluates an expression as a block over a stopped frame
  ([debugger.md](debugger.md), "Evaluation"); nothing evaluates without
  one. `FrameKind::TopLevel` exists for a top-level activation, which only
  tests push.

## The decisions

Decided with the user on 2026-11-18.

**1. One design, three stages: tracing, then printing code, then the
shell.** Tracing first, because it turns the reasons above into types,
which printing the inlining plan needs as well; the shell is the largest
and the most independent piece. The developer tool in the IDE came
between printing code and the shell as stage 3 (decisions 17 to 33),
the shell becoming stage 4 (decided with the user on 2026-11-21).

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
are the ones every encoding of `ns-assembler` is checked against (its
dev-dependencies), so a listing agrees with the encoder's tests by
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

**9. The lines reach the command line and the agent as the process's
standard error.** `--trace` is a flag of `run`, as the statistics are;
the MCP server collects a child's standard error with its output
already, so `debug_start` passes the flag on. The IDE reads the same
events from a file of its own instead (decision 17). The DAP agent is
not involved: it still writes nothing to the program's streams, and no
trace line is dropped by its bounded connection. A channel of their own
over DAP stays possible, since the events are typed (decided with the
user on 2026-11-18).

Decisions 10 to 16, stage 2's, were decided with the user on 2026-11-20.

**10. A body's bytecode is printed when it is made.** Every body of a
mixin is compiled when the mixin is built -- a module's when it is
loaded, an edit's when it is installed -- or decoded, the shipped
modules' from the embedded artifact and an image's from its code
section; `--print-bytecode` prints each body the filter selects at that
moment, its blocks indented under it as `disasm` prints them, whether or
not it ever runs. V8 prints bytecode when an Ignition job is finalized
(`v8/src/interpreter/interpreter.cc:254-283`), which its lazy compile
makes the first call (`v8/src/codegen/compiler.cc:3100-3168`); here the
making is the load, and printing at the first entry instead would test
a bit at every push, in the interpreter and in machine code, the hot
path the `codegen` gate holds. HotSpot prints bytecode only on demand
(`jdk/src/hotspot/share/oops/method.cpp:1988-1997`). What runs is the
trace's to say (`[compile] … trigger=entry`). Compiling a body's
debugger metadata again makes no body and prints nothing, nor does an
edit only checked. A body is printed at the first flush of the trace
after its mixin joined the heap's tables -- the index of new sides the
trace's names already keep (`trace_events/names.rs`) -- so that the
bodies the boot and the program's load made before the host installed
its sink are printed when it does, in the order their mixins joined, a
shipped module built on first demand at the next safepoint, and an
installed edit's new versions likewise.

**11. An optimized copy's listing heads with its tree of scopes; each
stretch of code names its scope by number.** The head numbers the
scopes, `s0` the body's own, each indented under the scope it is
inlined into with the send it is inlined at (`s2 {Demo/Account>>balance}
at s1@5`); wherever the code's scope or program counter changes, a line
names the scope, its innermost body and the program counter
(`s2 {Demo/Account>>balance} @2`). The tree is the one `--print-inlining`
prints. Strongtalk keeps the scopes as a tree apart from the code
(`strongtalk/vm/code/nmethod.cpp:782-807`); HotSpot repeats the whole
chain at every annotated instruction
(`jdk/src/hotspot/share/code/nmethod.cpp:4265-4397`), which the number
spares a listing of deep inlining, and V8 marks a stretch by an
inlining id resolved through a table
(`v8/src/codegen/source-position.cc:14-81`). A number tells apart the
arms of a type-case that inline the same callee; the body's name beside
it keeps every stretch of a callee found by its name.

**12. Listings go to standard error, as the trace's lines do.** A
listing follows the trace line of the event it belongs to, in one
stream, and standard output stays the program's; a host reading
standard error receives it as it receives the trace (decision 9), and
the IDE reads both from its file (decision 20). V8's code
tracer writes to standard output or a file
(`v8/src/diagnostics/code-tracer.h:31-36`), HotSpot's listings to its
`tty`; neither keeps the program's output apart.

**13. `--print-inlining` prints the plan of every compile that made
one, installed or refused.** The tree ends with the compile's outcome --
the copy installed, or the refusal's reason as the trace spells it -- so
that the question why a body stayed unoptimized is answered by the plan
that failed. HotSpot's C1, Strongtalk and V8's `--trace-turbo-inlining`
print the decisions as they are made, whatever the outcome
(`jdk/src/hotspot/share/c1/c1_GraphBuilder.cpp:4858-4880`,
`v8/src/compiler/js-inlining.cc:695-766`); C2 prints the last attempt
only (`jdk/src/hotspot/share/opto/c2compiler.cpp:244-245`). A body
refused before its plan was made -- more instructions than a program
counter holds -- has none to print. A listing of code stays bound to its
install (decision 5).

**14. A machine-code listing shows offsets; the base address appears
once, in its head.** An instruction's line carries its offset from the
body's start; a target in the body is `+0x…`, a data word `data+0x…`,
its offset from the first data word, and a call or jump out of it
names the stub, leaf, exit, cell or target it reaches,
so that a listing differs between runs only in its head and a test
pins it. The code zone is placed anew by every run and compacted in
one, which would make an absolute address in each line, as V8's
(`v8/src/diagnostics/disassembler.cc:425-429`) and HotSpot's carry,
stale. x64 is written in Intel syntax, AArch64 in the architecture's.

**15. The agent answers a frame's code through a request of its own,
`nsNext/frameCode`.** It answers the stopped frame's body's bytecode
and, if the frame goes on in machine code, that code's listing, in the
format `--print-bytecode` and `--print-code` print, each marked where
the frame stands -- the bytecode at the instruction the frame is at,
the machine code where the call the frame waits on returns. A stopped
frame goes on in its body's baseline code or, reified, in its twin,
never in an optimized copy, since a frame that goes on mid-body goes on
in the baseline code ([execution.md](execution.md), "The optimizing
tier's foundation"); so the listing is one of those two. The MCP
server's tool `debug_code` passes the text on. DAP's `disassemble`
request is built on memory addresses
(`instructionPointerReference`), in which bytecode has no place, the
scopes of decision 11 only a `symbol`, and the offsets of decision 14
none; no client of ours uses it, and it can be answered from the same
listings when one does.

**16. The trace folds the declines of callees that are no body into one
event per copy; `--print-inlining` keeps each.** A callee that is no
body -- a leaf or primitive the graph does not express, a nested
class's accessor, a factory -- is declined at nearly every send of it
in every copy: every inlined `==` of a string declines its leaf, so that
a line per decline would be most of a trace of a program that compares,
each naming nothing. The `inlining` category writes them as one
`declined-no-body` line per optimizing compile, counted by reason, and
keeps its own `declined` line for every callee that is a body, the
decline a body explains. The inlining plan is the compile's whole plan,
and lists every decline as a line of its own, each at its scope's send
(decided with the user on 2026-11-20).

Decisions 17 to 33, stage 3's, the developer tool in the IDE, were
decided with the user on 2026-11-21. The prior art they rest on is the
JetBrains plugins that bring HotSpot's compilation log into the editor
-- `yole/jitwatch-intellij` and `JetBrains/jitwatch4i` --, JITWatch
itself, Microsoft's Deopt Explorer for V8's log, V8's log and
Turbolizer, IntelliJ's coverage, and the bytecode and disassembly views
of IntelliJ, Rider and CLion.

**17. The IDE reads a file of JSON lines, `--trace-file=<path>`.**
`ns-next run --trace-file=<path>` writes every event and every listing
as one JSON object a line into the file, at each flush, instead of the
lines on standard error. The file is the one path that serves Run and
Debug alike, drops nothing -- decision 9's guarantee stands -- and keeps
the trace apart from the program's own standard error, which a program
could otherwise imitate. Every one of the prior art's tools reads such a
file: HotSpot's `-XX:LogFile` (`jdk/src/hotspot/share/utilities/ostream.cpp:706-789`),
V8's `--logfile`, each line flushed under a lock
(`v8/src/logging/log-file.cc:109`, `:251`), which jitwatch-intellij
passes as `-XX:LogFile=<temporary path>` and Deopt Explorer opens, and
the coverage file IntelliJ loads after a run. A channel over DAP would
serve Debug alone, and its bounded connection
(`crates/ns-cli/src/debug/connection.rs:128-175`) would have to drop
events or stall the program; the boot's listings arrive before any
client is connected.

**18. The runtime supplies the source positions.** Each body is
described once in the file -- its module's id, version and source path,
its range, a checksum of its source text, and a table from bytecode
index to source range for its statements and sends, from the debugger's
metadata (`debugger/metadata/`), which the DAP agent already turns into
lines and columns -- and every event, listing and position refers to
it. Only the runtime knows which source a bytecode index came from:
the plugin has no compiler, and a body's name does not identify it,
since every block of a method is `[] in <method>` and a class's three
initializer bodies share one name. V8 writes `code-source-info` with
offsets and an inlining table (`v8/src/logging/log.cc:1385-1490`),
Turbolizer's positions are `{scriptOffset, inliningId}`
(`v8/src/codegen/source-position.cc:118-127`); HotSpot logs a method and
a bytecode index only, which JITWatch maps through the class file's
line table -- a table this runtime keeps in its metadata instead.

**19. The file is typed records, bodies by number.** The first line is
a `header` with the format's name, `ns-next-trace`, and its version; a
`body` record gives a body its number in the file at its first mention,
with decision 18's description; `event` records carry the category, the
event's name and its fields, in the text trace's kebab-case vocabulary;
`bytecode` lists a body's instructions each with its index, statement
and text; `code` the machine code `ns-cli` decodes, each instruction
with its offset, text, scope and bytecode index, and the annotations;
`inlining` the plan as a tree. The format is described in this document
and held by tests; the plugin refuses a version it does not know, with
a message. V8's log names each code once by its address and then refers
to it, HotSpot's log each method once by its id
(`jdk/src/hotspot/share/compiler/compileLog.cpp:88-170`); the plugin, on
the JVM, decodes no machine code itself.

**20. `--trace-file` replaces standard error.** With it, standard error
holds the program's own lines only, and the console shows the program;
the tool window shows the trace, with a view in the text trace's line
format.

**21. The run configuration has a tab of its own, "Developer Tool".**
Seven checkboxes for the categories with "all", three for the listing
kinds, and a link to the watched methods (decision 23); each checkbox
says in a sentence on hover what its information holds. They apply to
`run`; for `bench`, which takes no trace, they are greyed. The plugins
for HotSpot's log add a tab of their own the same way
(`jitwatch-intellij`'s "JITWatch" tab, a `RunConfigurationExtension`).

**22. The free field for runtime options stays.** Options without a
control of their own -- `--timed`, `--lowering` and whatever comes --
stay reachable without a new plugin; an option that has a control,
typed into the field, is an error the run configuration's editor
reports. IntelliJ's Java and Kotlin configurations keep their "VM
options" beside their switches.

**23. One project-wide list of watched methods, each entry with its
switches "events" and "listings".** An entry is a method or a pattern
in the tools' syntax, a trailing `...` a prefix; both switches are on
when an entry is made; the plugin builds `--trace-filter` and
`--print-filter` from the list, and an empty list means everything, as
on the command line. The list is the user's and the project's, kept in
the workspace as breakpoints are, made in the editor and managed in a
dialog; the run configuration says only what a run produces. HotSpot's
`-XX:CompileCommand` names a method with what to do for it; IntelliJ's
breakpoints are such a list of entries with properties.

**24. Run and Debug start it.** A run whose configuration checks a
category or a listing kind passes `--trace-file` with a temporary path
and the filters, and the plugin reads the results; there is no executor
of its own. Debug keeps the combination the tool is most worth with: a
stop, the trace up to it and the frame's code. jitwatch-intellij and
JITWatch4i switch their log on in the configuration in the same way;
coverage's executor of its own cannot be debugged.

**25. The plugin reads the file live.** A reader follows the records as
they are appended, and the tool window and the editor are updated in
batches; at a stop everything up to it has arrived, since the runtime
flushes at the safepoint and a stop is one. A program that never ends
shows its trace too.

**26. One tab per traced run.** The tool window keeps a tab for each
traced run, named by its configuration and time, as the consoles do;
the editor shows the selected tab's decorations, the newest by default,
and closing a tab drops its data and its file. Two runs can be compared;
IntelliJ's coverage keeps its runs as suites the same way.

**27. The editor shows a state icon in the gutter and inlay hints at
the sends.** The icon at a method's header says how it ran --
interpreted, baseline, twin, optimized with its copies, deoptimized or
abandoned --, its tooltip the counts and reasons, and a click opens the
tool window at the method, as jitwatch-intellij's marker does; the hint
after a send's selector says what became of it -- inlined, declined and
why -- and its inline cache's state -- `mono`, `poly n`, `mega` -- with
the details on hover, as Deopt Explorer decorates its inline caches and
deoptimizations. Both can be switched off as other inlay hints can. No
underlining, which competes with errors and warnings and says less, and
no code vision line, which repeats the icon.

**28. One icon, only where it says something.** No method carries a
permanent icon: a method is watched through the editor's context menu,
an intention (Alt+Enter) or the gutter's menu, and shows an eye; after a
run its state icon takes the eye's place, a badge saying it is watched;
the icon's menu unwatches, shows the code, shows the inlining and opens
the tool window. With an empty list every traced method shows its state
icon. The shipped modules' sources, the library "Newspeak Core Library",
are decorated as a program's are.

**29. The tool window has four tabs: Methods, Events, Code and
Inlining.** Methods is a sortable overview of every traced method --
state, copies, deoptimizations, abandonments, code size, declined
callees --, the way into a run without a filter, as JITWatch's "Tops"
and jitwatch-intellij's table of failed inlinings are; Events the trace
as a table, filtered by category and body, grouped by method, a double
click going to its place in the source, and a switch to the text
trace's lines; Code the bytecode and machine code of the method at the
caret, a chooser for its copies -- baseline, twin, optimized per
receiver --, source, bytecode and machine code highlighted together
through decision 18's positions, as JITWatch's TriView and Compiler
Explorer do; Inlining each optimizing compile's plan as a tree of its
scopes, arms and declines with their reasons and its outcome, a click
going to the send.

**30. The code is shown in the tool window and in editor tabs.** The
tool window's view follows the caret, to explore, as Rider's IL Viewer,
IntelliJ's bytecode views and JITWatch4i's panels do; "Open in editor"
puts the copy it shows into a read-only editor tab that stays, to
compare.

**31. The debugger shows a frame's code through an action.** "Show
Frame Code", in the frames' context menu and the debugger's toolbar,
asks `nsNext/frameCode` and opens decision 30's editor tab, which then
follows the selected frame and every step, as CLion's disassembly does.

**32. A method edited since the run is marked stale.** The decorations
ride on the document's range markers; a method whose text no longer has
its body's checksum (decision 18) loses its hints, and its icon turns
grey, saying it changed since the run; the rest of the file stays
decorated. IntelliJ's coverage marks its data stale the same way.

**33. The core library is traced and decorated as a program is.** The
shipped modules' bodies are described with their sources as the
library's files, so that decisions 27 to 32 hold there too.

## Stage 1: tracing

**Built** (2026-11-18 to 2026-11-20;
`docs/superpowers/plans/2026-11-18-2153-developer-tool-stage-1-tracing.md`).
`ns-next run --trace=<categories>` writes one line on standard error for
every decision of the tiers and the caches in the categories named, with
its reasons; `debug_start` passes the same flags on, and the plugin's
run configuration passes them with a trace file ("Stage 3: the
developer tool in the IDE").

### The runtime's half

`ns_runtime::trace_events` (`crates/ns-runtime/src/trace_events.rs`,
with `trace_events/names.rs` and `trace_events/filter.rs`; not `trace`,
which is the collector's edge enumeration) declares the events and
formats nothing (decision 2). A host installs a sink with
`Interpreter::set_trace(settings, sink)`: `TraceSettings` -- its
`categories`, a `TraceCategories` set of `TraceCategory`s, and its
`filter`, a `TraceFilter`, beside stage 2's `printing` and
`print_filter` -- and a `Box<dyn TraceSink>`, whose `event` receives a
`TraceEvent { category, body, kind }` -- `body` the name of the body the
event is about, `None` for one about no body, `kind` a `TraceEventKind`
whose reasons are enums, each with a `name()` in kebab case, the
spelling of its line. Installing replaces the sink before and drops what
it had not received; no category and no printing is no tracing.

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
  body no mixin holds is `(unnamed)`. Every body an event or a listing
  names is a `BodyName`, its name beside a number `BodyNames` gives it
  at its first delivery -- 1 the first, none given twice in a sink's
  run, a dead body's entry never lending its number to the body that
  takes its address -- since a name does not identify a body; a body
  only events the filter refuses name takes none. With
  `TraceSettings::describe_bodies`, the flush describes each body it
  numbered just before the first delivery that names it
  (`TraceSink::body`, `trace_events/descriptions.rs`): its module, a
  shipped module's path among the library's files, its source range
  and its FNV-1a 64 checksum, and its statements and sends from the
  debugger's metadata (`Cache::metadata_with_text`), empty where the
  body compiled again is not alike (decision 18). A copy's key is named by two
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
| `inlining` | `declined key= entered= in-body={} callee={} reason= at=` | The plan sends the callee instead, only for a callee that is a body (decision 16). The reasons, in declaration order, which `declined-no-body` counts in too: `not-snapshotted` (the tree holds no body of it -- a nested class's accessor, a factory, a `doesNotUnderstand:` target, or a method past the limits where it is sent) and `not-expressed` (a leaf or primitive the graph does not express), both given only for a callee that is no body and so written only by `declined-no-body`; `returns-otherwise`, `unknown-lazy-field`, `observed`, `reaches-past-the-compile`, `block-escapes`, `creates-a-block`, `over-weight`, `over-compile-limit`, `over-recursion-limit`, `out-of-registers`, given for a body, each in `declined-no-body` too where that body's code id the heap no longer names. |
| `inlining` | `declined-no-body key= entered= count= reasons=<reason>:<count>,…` | Once per optimizing compile that declined a callee that is no body -- a leaf or primitive, or a body the heap no longer names -- `count=` the declines in all and `reasons=` each reason given with its count, in declaration order, as the `declined` row lists them, joined by commas (decision 16). |
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
[inlining] {Demo>>compare} declined-no-body key={Demo} count=3 reasons=not-snapshotted:2,not-expressed:1
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
- **The plugin's run configuration** passes `--trace` and
  `--trace-filter` from its "Developer Tool" tab and the watched
  methods, always with `--trace-file`, after the subcommand and
  `--debug-connect` and before `--script` ("Stage 3: the developer tool
  in the IDE", "The plugin's half"; [editor-support.md](editor-support.md),
  "The project file and running projects").

### Tests

`crates/ns-runtime/tests/trace.rs` runs programs with a collecting sink,
as `tests/jit.rs` does (`#![cfg(ns_jit)]`, the JIT on, a stress mode
that would defeat what a test pins switched off by its debug-only
setter), and names the events that must occur, never a complete
sequence, so that a changed threshold breaks only what it changes. The
rarer events run there too: a twin no empty zone holds
(`refused-for-its-twin`, and its `freed`), a twin the exhausted zone
holds (`held-twin`) and the reified frame that therefore does not
switch at its loop's head (`osr-refused reason=twin-held`), each in a
zone sized from the bytes a run with room measured, so that the tests
hold on every architecture; and a pending request a collection between
runs drops for a reclaimed id (`dropped`), in a build with debug
assertions only, since only a host's collection
(`Interpreter::collect_now`) falls between the run-out that records a
request and the safepoint that compiles it. What no test reaches is
[OPEN-WORK.md](../OPEN-WORK.md)'s "The trace's rarer events end to
end". The plan's declines are unit tests in `jit/optimizing/plan.rs`, the filter's
and the names' in their modules; their trace, the declines of callees
that are no body folded per copy beside a body's own `declined`, is
`declines_of_no_body_are_folded_per_copy`, and the counting in
declaration order `trace_events.rs`'s unit tests. The line format is pinned by
`crates/ns-cli/src/trace.rs`'s unit tests, `--trace` and its refusals by
`crates/ns-cli/tests/cli.rs` (the folded line by
`trace_inlining_folds_the_declines_of_no_body`), `debug_start`'s trace by
`crates/ns-cli/tests/mcp.rs` (`a_trace_arrives_with_the_output`) and the
plugin's options by its `NewspeakRunConfigurationTest`; the
CLI's and the MCP server's tests strip every stress mode from the child
and answer nothing on a target without a JIT.

## Stage 2: printing code

**Built** (2026-11-20;
`docs/superpowers/plans/2026-11-20-1634-developer-tool-stage-2-printing-code.md`).
`ns-next run --print-bytecode`, `--print-code` and `--print-inlining`
print, on standard error, every body's bytecode when its mixin is made,
every install's machine code and every optimizing compile's inlining
plan, selected by `--print-filter`; the debugger answers a stopped
frame's code, which the agent's `nsNext/frameCode` and the MCP server's
`debug_code` pass on.

### The runtime's half

`trace_events/listings.rs` declares the listings, and the sink receives
them beside the events: `TraceSink::listing(&Listing)`, a default that
ignores it, so that a sink of events alone needs nothing new. A
`Listing` is a `BytecodeListing`, a `CodeListing` or an
`InliningListing`. `TraceSettings::printing` (`Printing { bytecode,
code, inlining }`) says which are made, and `print_filter`, a second
`TraceFilter` of the trace filter's patterns, which bodies they are made
of; the trace's categories and filter say nothing of them, so that
printing needs no category. The runtime formats only bytecode, which it
alone reads (`bytecode::disassemble`); everything else it hands out as
values, and the host formats it.

- **One queue.** A code listing and an inlining listing are recorded
  pending in the trace's one queue (`PendingListing`, recorded by
  `EventTracer::record_listing` right after the event of the install or
  outcome it belongs to), holding bodies as `Rc<CompiledCode>`, keys as
  their `CustomizationKey` and an arm's receiver as its dispatch id --
  named before a full collection reclaims the id, as a key is
  (`trace_events::name_freed_keys`) -- and no handle; a flush names them
  as it names events, applies the print filter to the body and delivers
  each in its place among the events, so that a listing follows the line
  of its event (decision 12). Nothing of it is `cfg(debug_assertions)`:
  a release build prints (decision 3).
- **Bytecode when a mixin is made** (decision 10). A listing of bytecode
  is never pending: at a flush, with `printing.bytecode` set, the
  trace's names index the sides of every mixin that joined the heap's
  tables since the last flush (`BodyNames::index_new_sides`), and each
  root body named for the first time -- a method, a factory, the three
  initializer bodies, a superclass clause, never a block, which is
  listed nested under its maker -- is listed before the events, in the
  order the mixins joined. A body a new version shares with the one it
  superseded was named already and is not listed again. Installing the
  sink resets the names, so `set_trace`, which flushes before it
  returns, lists everything the boot and the load made; a shipped
  module built on first demand and an installed edit are listed at the
  next safepoint or host return, whose flush runs, when bytecode is
  printed, even with no event pending (`EventTracer::flush_due`). The
  compile trigger's own flush tests only for pending events, so
  that nothing is added on its path. The debugger's metadata compile and
  an edit only checked add no mixin to the heap's tables and print
  nothing.
- **Machine code at its install** (decision 5). A compile annotates
  when `CompileSettings::annotate` is set, which every compile reads
  from the tracer (`EventTracer::prints_code`): the macro assembler then
  records positions and annotations as labels, which emit no byte, so
  that the bytes are those of a compile without them
  ([execution.md](execution.md), "The baseline JIT's foundation";
  `annotating_changes_no_byte`). At the install (`jit::captured_code`,
  `JitState::installed_listing`) the listing takes the body's bytes, its
  data words zeroed, its base address, `code_end` -- the end of the last
  instruction, past which the words' alignment padding is no instruction
  -- the data words by kind (`DataWord`: the budget word, the
  dispatching rows and tail, a twin transfer's address, a closure's or
  an outer local's site), the positions (`Position::Instruction { scope,
  program_counter }` at each instruction's start, `Position::OutOfLine`
  where code out of the straight line starts: the cold paths, a backward
  jump's tail, the dispatching entry), and the annotations of the calls
  and jumps out of the body (`Annotation::Leaf`, `Stub`, `Exit { kind,
  program_counter }`, `Cell { site }`, and `Target` for a callee a leaf
  decided, `initializer-callee` or `forcing-callee`, and for the `twin`
  a transfer goes on in; a `Stub` may mark the instruction that selects
  the stub, as a send's load of the polymorphic stub does). A baseline
  body's listing is recorded after its `compiled` event, a twin's after
  its `compiled-twin`, in the leaf that compiled it, and delivered at
  the next flush: captured at the install, it survives a compaction or a
  free before that flush.
- **An optimized copy's scopes** (decision 11). The emitter records a
  position wherever the scope or the program counter of the code
  changes -- at each block, frame node, node and block end with a
  program counter of its own, poll and trap, and out of line at the
  mixed paths and trampolines -- and the listing numbers the scopes the
  code lays out from `s0`, the body's own, without a gap, each with the
  scope and program counter it is inlined at (`PlanDecisions::scopes`),
  its body held from the compile. The listing is recorded after the
  `optimized` event, its key named as the events name one.
- **The inlining plan** (decision 13). Every optimizing compile that
  made a plan records it once its outcome is known: after `optimized`,
  with the bytes installed; after `refused`, or `deferred` for a zone
  with no room, with the refusal. It lists the scopes the code lays out,
  numbered as the code listing's, each type-case arm in the order of its
  scope and program counter, then of the cache -- an inlined callee as
  its scope's number, or what an always-inlined callee does, in kebab
  case (`AlwaysInlined::name`: `getter`, `setter`, `lazy-getter`,
  `quick`, `builtin-class`, `value-of-parameter`, `cull-parameter`, and
  for a leaf `identity`, `class`, `indexed`, `integer`, `float`,
  `primitive`) -- and
  each callee sent instead, with stage 1's reason. An arm or a decline
  in a scope the code does not lay out, and an arm whose inlined scope
  is not laid out, are left out: they have no number, and the graph
  sends their site.
- **No JIT.** On a target without `cfg(ns_jit)` bytecode is printed, and
  `--print-code` and `--print-inlining` print nothing.

### The listings and their text

`crates/ns-cli/src/listing.rs` writes each listing between a head line
`--- <kind> {<body>} …` and `--- end`, the body named as the trace names
it, so that a script cuts a listing out of standard error. The sink
writes a listing in one `write_all` on the locked standard error, so
that neither the program's output nor a trace line tears it.

A body's bytecode, as `disasm` prints it, its blocks nested under their
`MakeClosure`:

```
--- bytecode {Demo>>total}
method Demo>>total (args 0, registers 20)
   0        -  MakeVector r15, size 1
   1   stmt 0  SendLexical r16 <- depth 0 #Account () @0
   …
   8   stmt 2  MakeClosure r18, [:index] -> copying r15 r13
        block block (args 1, registers 19)
           0   stmt 0  LoadVector r16 <- r14[0]
           …
  11   stmt 3  Return r16
--- end
```

Machine code (decision 14). The head is `--- code {<body>} <tier>
<architecture>`, the tier `baseline`, `twin` or `optimized`, the
architecture `x64` or `aarch64`, then ` key={…}` and ` entered={…}`
where the copy has them, as the trace's events write a key, then
` base=0x<address> bytes=<length>`, the base the only field that differs
between runs. An optimized copy's head is followed by `scopes:` and its
tree, `s0` two spaces in and each scope two spaces deeper than the one
it is inlined into, `  s1 {Demo/Account>>balance} at s0@14`, the
children of a scope in the order of their numbers. Then the code: a
position line wherever the position changes -- `  @<pc>  <instruction>`
in baseline and twin code, `  s<n> {<body>} @<pc>  <instruction>` in an
optimized copy, the instruction as the bytecode listing writes it
without its index, and `  out of line` -- and a line per instruction,
`    +0x<offset:04x>  <decoded>`, ending in `  ; leaf <name>`,
`  ; stub <name>`, `  ; exit <kind> @<pc>`, `  ; cell site <n>` or
`  ; target <name>` where an annotation stands on it. A stub's
annotation names the entry-block word the call or jump loads by that
word's own name (`BlockWord::name`, `; stub polymorphic-stub`), so that
each word has one name and a search for it finds every use. The prologue's
instructions before the first position have no position line. A target
in the body is written `+0x<offset>` and a data word's address
`data+0x<offset from the first data word>`; an address outside the
body, and an immediate, as the decoder writes it. x64 is decoded by
`iced-x86` in Intel syntax (`listing/x64.rs`, a symbol resolver naming
the body's addresses), AArch64 by `yaxpeax-arm`'s `Display`
(`listing/aarch64.rs`, a branch's or a literal load's `$+0x…` renamed,
`adrp`'s page left as it is); each binary carries the decoder of its
own architecture alone, and a listing of another is shown in hex, four
bytes a line. Last, `data:` and a line per data word,
`    data+0x<offset>  <kind>`: `budget word`, `dispatching rows`,
`dispatching tail`, `twin transfer @<pc>`, `site`.

```
--- code {Demo>>total} optimized x64 key={Demo} base=0x2961e3e6d00 bytes=2438
scopes:
  s0 {Demo>>total}
    s1 {Demo/Account>>balance} at s0@14
  s0 {Demo>>total} @0  stmt 0  SendLexical r16 <- depth 0 #Account () @0
    …
    +0x0263  mov rcx,[r14+80h]  ; stub polymorphic-stub
    …
    +0x0271  call rcx  ; cell site 0
    …
  s1 {Demo/Account>>balance} @2  stmt 0  IntegerBinary Add r15 <- r13, r14 else -> 4
    …
    +0x073a  jmp r11  ; exit trap @2
  out of line
    …
    +0x0849  call r11  ; leaf grow-stack
    …
--- end
```

The inlining plan: the head `--- inlining {<body>}` with the key's
fields, the tree of scopes as the code listing's, a line per arm,
`  s<n>@<pc> {<receiver>} -> s<m>` or `-> always <kind>`, a line per
decline, `  s<n>@<pc> declined callee={<body>} reason=<reason>`
(`callee={}` for a callee that is no body), and the outcome,
`  installed bytes=<n>` or `  refused reason=<reason>`:

```
--- inlining {Demo>>total} key={Demo}
  s0 {Demo>>total}
    s1 {Demo/Account>>balance} at s0@14
  s0@14 {Demo/Account} -> s1
  s1@0 {Demo/Account} -> always getter
  s0@0 declined callee={} reason=not-snapshotted
  s0@1 declined callee={} reason=not-snapshotted
  installed bytes=2438
--- end
```

A frame's code (decision 15) is marked where the frame stands, every
column kept: in its bytecode the instruction the frame is at,
`{index:4}->`, the arrow in place of the two spaces after the index
(`   7->`, ` 100->`; `bytecode::disassemble_marked`, the root body's
instruction only), and in its machine code the instruction the call it
waits on returns to, `  ->+0x…`, the arrow in place of two of the
line's four leading spaces (`CodeListing::mark`).

### A stopped frame's code

`Command::FrameCode { frame }`, served at a stop only -- refused with
"the program is running" while it runs, "no run executes" between runs
and "no frame at that index" past the stack -- answers
`Event::FrameCode { body, bytecode, machine }`: the frame's body named
as the stack trace names it, its bytecode marked at the frame's program
counter, and, for a running frame whose activation is compiled, the
code its return resumes (`jit::frame_code_listing`): the body's lean
code compiled against the frame's feedback base, or its twin for a
reified frame -- `None` where the frame goes on interpreted, for a
suspended frame, and for a reified frame whose twin is not compiled
yet, since asking compiles nothing. The bytes are read from the zone;
the positions and annotations, which an install keeps nowhere, come
from the same body assembled again with annotating on, a debug build
holding every byte to the zone's -- the precedent of the debugger's
metadata, compiled again on demand; the data words are zeroed as an
install's listing has them, and the mark is the return of the frame's
one call at its program counter (`ProgramCounterMap`'s `returns`). The
top frame of a stop runs in the interpreter, so its machine code is
`None`; a caller that called from machine code has one.

### The hosts

- **`ns-next run`** (`main.rs`, `commands/run.rs`, `trace.rs`'s
  `StandardErrorSink::listing`) takes `--print-bytecode`,
  `--print-code` and `--print-inlining`, any of them together and beside
  `--trace`, and `--print-filter=<pattern>`, once for each pattern, in
  `--trace-filter`'s syntax, which clap refuses without a print option.
  A malformed pattern and one naming no module of the program or the
  binary are refused as `--trace-filter`'s are, `--print-filter:` and
  the reason, with exit status 2 before anything runs. Only `run`
  prints; `load` and `bench` do not.
- **`debug_start`** takes no print option, on purpose: its answers keep
  the last 2,000 characters of the program's output and standard error,
  which a run's listings would fill. An agent asks for the code of the
  frame it stopped in instead.
- **`nsNext/frameCode { frameId }`**, a request of the agent's own
  (`debug/agent.rs`), forwards `Command::FrameCode` for a frame of the
  stop and answers `{ body, bytecode, machineCode }`, each listing's
  text as `ns-next run` prints it, `machineCode` `null` for a frame
  without; an unknown frame id is refused, and so is a stop the program
  has left meanwhile, with "the program went on"
  ([debugger.md](debugger.md), "Launching, the start and the end").
- **`debug_code(frame?)`**, the MCP server's tool (`mcp/tools.rs`,
  `mcp/session.rs`), asks it for frame `frame`, the top one by default,
  and answers the bytecode listing, then the machine code's listing or
  `the frame goes on in the interpreter` ([debugger.md](debugger.md),
  "The MCP server").
- **The plugin's run configuration** passes the print options and
  `--print-filter` as it passes `--trace`, with `--trace-file` ("Stage
  3: the developer tool in the IDE", "The plugin's half").

### Tests

`crates/ns-runtime/tests/printing.rs` collects listings with a sink of
its own: bytecode at the sink's install, with no category, for a
shipped module built on demand and for an installed edit, and nothing
for a checked edit and for the debugger's metadata compile; and, in its
module `code` (`cfg(ns_jit)`, the JIT on, a stress mode that would
defeat what a test pins switched off by its debug-only setter), a
baseline listing, a twin's, the data words, the filter, an optimized
copy's scopes and positions -- every arithmetic node named in the
straight line -- the scopes against the `inlined` events of the same
compile, and the plan installed, refused in a zone sized one copy
short, and with a decline in its scope. In the crate,
`annotating_changes_no_byte` holds every body of the core library and
the goldens, under all three conventions, lean and twin, to the same
bytes with and without annotations; `positions_ascend_and_cover_the_code`
does so for every tree of the inlining corpus;
`a_listing_survives_a_compaction_before_its_flush` holds a listing to
its install; and `bytecode.rs`'s tests pin the mark.
`crates/ns-runtime/tests/debugger.rs` pins `FrameCode`: the marked
bytecode under both lowerings, the refusals, and a caller in baseline
code and in its twin. `crates/ns-cli/src/listing.rs`'s unit tests pin
each kind's text, the mark and, on x64, the decoder's names;
`listing/aarch64.rs`'s its own on AArch64; `tests/cli.rs` the options,
their refusals, one write per listing beside the program's standard
error, and the whole listing of a body without a send on Windows x64
(`a_leafless_body_pins_its_listing`, its base masked);
`tests/debug_adapter.rs` `nsNext/frameCode`; and `tests/mcp.rs`
`debug_code`. A test of machine code asserts the lines that must occur,
never a whole listing but that one, and the CLI's and the MCP server's
tests strip every stress mode from the child and answer nothing without
a JIT.

## Stage 3: the developer tool in the IDE

**Built** (2026-11-21;
`docs/superpowers/plans/2026-11-21-0145-developer-tool-stage-3-the-ide.md`;
decisions 17 to 33). The runtime numbers the bodies a trace names and,
on request, describes each with its source positions; `ns-next run
--trace-file=<path>` writes the trace and the listings into a file as
typed JSON lines; and the JetBrains plugin switches them on from the run
configuration and a project-wide list of watched methods, reads the file
of every traced Run and Debug live, and shows it in the "Newspeak JIT"
tool window, in the gutter and inlay hints of the editor, in editor tabs
of the code, and at a stop as the frame's code.

### The runtime's half

Every body a delivery names carries a `BodyName` (`trace_events.rs`):
its name as the debugger names a frame and its number, which tells apart
the bodies one name covers -- every block of a method is `[] in
<method>`, a class's three initializer bodies share a name. The numbers
are minted at the flush by `BodyNames` (`trace_events/names.rs`), from
1 in the order the trace first names the bodies, keyed by each body's
`Rc` address with a `Weak` beside it, so that a body that died and the
body that took its address get two numbers: a number is unique for the
sink and never reused. 0 names a body outside the trace, as a stopped
frame's code (`Command::FrameCode`) does, and never reaches a trace
file. The text trace and the listings print the name alone.

With `TraceSettings::describe_bodies` set -- only `--trace-file` sets
it -- the flush hands the sink a `BodyDescription`
(`trace_events/descriptions.rs`, `TraceSink::body`) before the first
delivery that names its number: a block's family root, the body's
module, a shipped module's path among the library's files, its source
span and its checksum (`source_checksum`, FNV-1a 64 over the source's
UTF-8 bytes with every `\r` left out), and each statement's
instructions and each send's index, site, call and selector tokens,
from the debugger's metadata (`debugger/metadata/`), compiled again
from the module's current text and kept in its weak cache. A
description is made from the body's `Rc<CompiledCode>` and its mixin,
never a handle, and never in a leaf; nothing on the hot path changes,
since a site still records only what it recorded. A sink's `flushed`
is called at the end of every flush's deliveries. A bytecode listing
carries its bodies' instructions apart (`ListedBody`,
`ListedInstruction`; `bytecode::listed_instructions`, the walk the text
listing's lines are made by too), so that a record lists them without
parsing text.

### The trace file

`ns-next run --trace-file=<path>` (`crates/ns-cli/src/trace_file.rs`,
`TraceFileSink`) writes every delivery the runtime hands its sink into
the file at `<path>`, replacing a file there, instead of on standard
error, which then holds the program's own lines only (decision 20). It
needs `--trace` or a print option, which clap enforces; the filters
select as they do on standard error. The file is created once every
option is checked and the program is loaded, before anything of it
runs; a path no file can be created at is refused, naming it, with
exit status 2, as a malformed filter is (`commands/run.rs`). With a
file, the trace's settings set `describe_bodies`, so that the runtime
hands the sink each body's description (decision 18,
`trace_events/descriptions.rs`).

Each record is one JSON object on one line, ended by a newline and
written with one `write_all`; the file is flushed at the end of every
flush's deliveries (`TraceSink::flushed`), so that a reader following
it is current at every safepoint and every stop, and never sees half a
record a flush wrote. A file that can no longer be written loses the
trace, never the run. A record's keys are the text trace's names, in
its kebab case; the order of the keys within an object is no part of
the format. Every body is named by its number: unique for the run,
never reused, and never 0, which names a body outside the trace
(`debug_assert!`ed where a record is written). A range is `[startLine,
startColumn, endLine, endColumn]`, counted from 0, the columns in
UTF-16 code units of the module's text (`ns_tooling::lines::LineIndex`,
the language server's and the DAP agent's count).

The records:

- `{"kind":"header","format":"ns-next-trace","version":1,"architecture":…,"categories":[…],"listings":[…]}`,
  the first line: the format's name and version, which a reader that
  does not know them refuses, the architecture the file's `code`
  records are instructions of, `"x64"` or `"aarch64"`, `null` for a
  binary without a JIT, the categories traced, by their names in their
  documented order, and the listings printed, of `"bytecode"`, `"code"`
  and `"inlining"`: what tells a reader which records can arrive. An
  optimizing compile's outcome comes only as an `optimize` event or an
  `inlining` record, so a reader holds a compile's `inlined` and
  `declined` events until it arrives and drops them for a refused or
  deferred copy -- unless neither can arrive, when it takes them at
  once.
- `{"kind":"body","body":N,"name":…,"family":N|null,"module":{"id":…,"version":…|null},"source":…,"range":[…],"checksum":…,"statements":[…],"sends":[…]}`,
  before the first record that names `N`: the body's name as the text
  trace writes it (`Demo>>main:args:`, `[] in Demo>>run`); for a block,
  `family`, the number of its family's root -- the method, factory,
  initializer or superclass clause whose code created it, at any depth
  --, which is described before it and which the cache transitions of
  the block's sites name, `null` for a root; its module,
  and its source: `{"file":"<absolute path>"}` for a program's module,
  `{"library":"<path>"}` for a shipped one, its path among the core
  library's files (`platform/TimeFactory.ns`, as `ns-next
  shipped-sources` lays them out), `null` for a module read from no file
  (the embedded manifest). `range` is the body's source; `checksum` is
  the FNV-1a 64 of that source's UTF-8 bytes with every `\r` byte left
  out (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`), sixteen
  lower-case hex digits, by which a reader tells whether the text it
  holds is the text the body was compiled from; the `\r`s are left out
  because an editor holds a file with `\n` line ends whatever the file's
  are, so that a file with CRLF or mixed line ends still matches. `statements` are
  `{"instructions":[from,to],"range":[…]}`, each statement's
  instructions and source in emission order, and `sends` are
  `{"at":index,"site":n|null,"call":[…],"selector":[[…],…]}`, each send
  instruction's index, its site as the `caches` category numbers it
  (`null` for a send no cache serves), the call from its receiver to its
  last argument and each token of its selector; both empty for a body
  whose metadata the runtime could not compile again alike. A range is
  `null` only where the host holds no text of the module. Two kinds of
  body take a number and no `body` record -- one no mixin holds and one
  whose range lies outside its module's text --, so a reader accepts a
  number it has no description of.
- `{"kind":"event","category":…,"body":N|null,"event":…,…}`, an event:
  its category, its body, `null` for an event about none, its name as
  `TraceEventKind::name` answers it, and every value its line writes as a
  field of the line's name ("The events and their lines"): a number as a
  number, a reason as its kebab-case name, a body (`in-body`, `callee`,
  `spent-by`) as its number, a class (`key`, `entered`, `receiver`) as
  its name, `key` and `entered` `null` where the line leaves them out. A
  cache transition, whose line spells its two states as the event word,
  is the event `transition` with the fields `before` and `after`, its
  `selector` without the line's `#`; `declined-no-body`'s `reasons` an
  array of `{"reason":…,"count":n}` in the line's order; `retired`'s
  `version` `null` where the line writes `none`.
- `{"kind":"bytecode","body":N,"bodies":[{"body":N,"instructions":[{"index":i,"statement":s|null,"text":…}]}]}`,
  a body's bytecode, the root and then the blocks it makes, each
  instruction's text as the text listing writes it after its index and
  statement columns.
- `{"kind":"code","body":N,"tier":"baseline"|"twin"|"optimized","key":…|null,"entered":…|null,"architecture":…,"bytes":n,"instructions":[…],"data":[…],"scopes":[…],"mark":offset|null}`,
  an install's machine code, decoded by `ns-cli` as the text listing
  decodes it (`listing::annotated`, the one walk that places the
  positions and the annotations on the instructions for both): each
  instruction
  `{"offset":o,"text":…,"scope":s|null,"at":pc|null,"annotation":…|null}`,
  its offset in the body, its text, the scope and program counter of the
  stretch it stands in (`null` before the first position and out of
  line), and what a call or jump out of it reaches as the text listing's
  annotation writes it (`leaf miss`, `cell site 3`; two on one
  instruction joined by `; `); each data word `{"offset":o,"word":…}`,
  its offset from the first data word, as the instructions' texts name
  it (`data+0x0008`), and what it holds (`budget word`); each scope
  `{"scope":s,"body":N,"parent":[scope,at]|null}`. The base address, an
  address of the run, is left out.
- `{"kind":"inlining","body":N,"key":…|null,"entered":…|null,"scopes":[…],"arms":[…],"declined":[…],"outcome":…}`,
  an optimizing compile's plan: its scopes as a `code` record's; each
  arm `{"scope":s,"at":pc,"receiver":…,"inlined":{"scope":s}|{"always":"identity"}}`;
  each decline `{"scope":s,"at":pc,"callee":N|null,"reason":…}`; and
  `{"installed":bytes}` or `{"refused":"<reason>"}`.

`event_record`, `listing_record` and `body_record` each match their
input exhaustively, so that a new kind of event or listing, or a new
field of a description or a listing, does not compile until it has its
record.

The ranges are counted in the text the host read each module from
(`ModuleTexts`), and the descriptions are made from the module's
current text: a body of an edit installed during a traced run, and a
body an edit superseded, are therefore described against a text their
ranges and checksums do not fit, and the plugin shows them stale
([OPEN-WORK.md](../OPEN-WORK.md), "What the developer tool's stage 3
left"). A `code` record writes an out-of-line stretch's position as it
writes one before any position, `null`.

### The plugin's half

The developer tool lives in the plugin's `devtool` package, its
decorations in `devtool/editor/`, its views in `devtool/ui/`, the
records and a run's model in `devtool/model/`; "Show Frame Code" is the
debugger's (`debugger/`).

**The "Developer Tool" tab** (`DeveloperToolTab.kt`, decision 21) of a
Newspeak run configuration has a checkbox for each of the seven
categories, with "All", one for each listing kind, each saying on hover
in a sentence what it holds, and the link "Manage watched methods…";
the configuration stores them (`traceCategories`, `printBytecode`,
`printCode`, `printInlining`). They apply to `run`, and are greyed while
the command the configuration's form shows is `bench`. The first tab's
free field, "Further runtime options" (decision 22), keeps every other
option of ns-next's own; an option the Developer Tool tab owns --
`--trace…`, `--print-…` -- typed there is an error of the
configuration's check, which names it. `newspeakCommandLine` places the
tab's options after the subcommand and `--debug-connect` and before the
free options: `--trace=<categories>` (`--trace=all` for all seven) with
a `--trace-filter` per events pattern, a `--print-<kind>` per listing
kind with a `--print-filter` per listings pattern, and `--trace-file`.

**The watched methods** (`WatchedMethods.kt`, decision 23) are one list
per project, kept in its workspace file as the breakpoints are: each
entry a method or a pattern in the tools' syntax, a trailing `...` a
prefix, with its switches "events" and "listings", both on when it is
made. An entry is made in the editor -- Watch Method in the editor's
context menu and the gutter's (`WatchMethodAction`), and as an
intention (`WatchMethodIntention`) -- for the method whose declaration
holds the caret or the line, named by the language server's
`newspeak/methodNames` ([editor-support.md](editor-support.md), "Method
names"), since the plugin parses no Newspeak; the id a project file
gives the module with `as` replaces the analyzer's. "Manage watched
methods…" shows the list as a table (`WatchedMethodsDialog`), which
refuses an entry not of the trace filter's form (`WatchPatterns.kt`,
the grammar `TraceFilter::parse` reads). A run's filters are built from
the list as it stands at its start: everything for an empty list, and
otherwise each kind narrowed to the entries that want it and name a
module the run has -- one its project file names, by its `as` or else
by the id and version `newspeak/moduleIdentity` answers, the single
module run from its source, or a shipped one -- since `ns-next run`
refuses a pattern naming no module of the program with exit status 2.
A kind no entry is left for is not asked for at all, since a run
without a pattern would trace every body.

**Traced runs** (`TracedRuns.kt`, decisions 24 and 26). Run and Debug
start their process through `NewspeakRunState.startProcess` alike; a
run that traces or lists anything is handed a temporary file,
`newspeak-trace/trace-<uuid>.jsonl` under the IDE's temporary
directory, and gets a `TraceRun` named by its configuration and the
time and a tab of the tool window, and is selected. Closing its tab
drops the run, its reader and its file, the newest run left becoming
the selected one, whose decorations the editor shows. The reader
(`TraceFileReader`) follows the file on a pooled thread, looking for
appended bytes every 50 ms, keeps an incomplete last line until its
newline arrives, parses each record with the platform's Gson
(`TraceRecords.kt`), and publishes them in batches of at most 5,000
records through `invokeLater`, no record waiting more than 200 ms for
its batch; once the process ends it reads to the end. A first line that
is no header of `ns-next-trace`, a version other than 1, a malformed
line and an unreadable file end the reading with a message in the run's
tab. A `TraceRun` (`TraceRun.kt`), read and changed on the event
dispatch thread only, keeps the records by body number and decides each
body's state from its events in order -- interpreted, baseline, twin,
optimized with its copies counted, deoptimized once its last copy is
gone, abandoned until revived -- and each send's outcome: the inlining
decision of the last optimizing compile whose copy was installed, held
until the compile's outcome arrives unless the header says none can,
and the state of its cache, `mono`, `poly <n>` with the receivers seen
since it was last vacant, or `mega`, a block's sites by its family's
root.

**The tool window**, "Newspeak JIT" (`ui/`, decision 29), has a tab per
traced run (`RunPanel`) of four views. Methods lists every method the
run described -- a block counted as its method's -- with its state,
copies, deoptimizations, abandonments, the code bytes of its largest
copy and the callees declined in it, sortable, a double click opening
it. Events is the trace as a table, filtered by category and by a part
of the method's name, grouped by method on request, a double click
going to the event's place in the source, and with "Text" the same rows
as the text trace's lines. Code shows the bytecode and machine code of
the body at the caret of a source editor, the innermost whose range
holds it, with a chooser of its copies -- its bytecode, then each
install, baseline, twin and optimized per key -- over a read-only
viewer of the listing in the text listing's format, built from the
records; "Open in Editor" (decision 30) opens the copy shown in a
read-only editor tab that stays (`CodeListingFile`), the tab already
open on it if there is one. Source, bytecode and machine code are
highlighted together (`LinkedHighlighting`): the caret's statement in
the source and the listing's lines whose instructions or machine-code
stretches belong to it, in either direction, one colour per statement
from six keys of the colour settings' own. The plugin's bytecode
listing has no method line and does not nest a block at its
`MakeClosure`, as the text listing does. Inlining is a tree per
optimizing compile, its scopes nested under the scope whose send
inlined them, the arms and the declines under each, the outcome at the
root, a click going to the send. A view opens a program body's file by
its path and a shipped body's under the project's core library
(decision 33).

**The editor** (`DecorationsService`, decisions 27, 28, 32 and 33)
decorates every open editor of a body's file, a program's or the core
library's, from the selected run's records: an icon at each method's
header (`StateGutterRenderer`) -- its state, badged when the method is
watched, its tooltip the counts and the last deoptimization's reason, a
click opening Unwatch, Show Code, Show Inlining and Open in Tool Window
-- and a hint after each send the run says something of
(`SendInlayRenderer`), `inlined` or `declined: <reason>` and the cache's
state, the events' fields on hover; each block at its own range. A
watched method the run has no body of shows an eye. Both icons and
hints can be switched off in the plugin's settings
(`DeveloperToolDecorationsSettings`). The decorations ride on a range
marker per body and send, made once per document and run, so that they
follow every edit; a method whose text in its markers no longer has its
records' checksums is stale (decision 32): its hints go, its icon greys
with "Changed since the run", the Code tab links nothing in it, and the
rest of the file stays decorated.

**Show Frame Code** (`debugger/ShowFrameCodeAction.kt`, decision 31),
in the frames' context menu and the debugger's toolbar, asks the agent
for the selected frame's code (`nsNext/frameCode`) and opens it in a
read-only `CodeListingFile` tab, which `FrameCodeFollower` fills with
the selected frame's code at every stop and every selection of another
frame until the tab closes or the session ends
([debugger.md](debugger.md), "The JetBrains plugin").

### Tests

`crates/ns-runtime/tests/trace.rs` and `trace_events/names.rs`'s unit
tests pin the numbers -- unique, and never a dead body's for the body
that took its address -- and the descriptions;
`crates/ns-runtime/tests/printing.rs` the structured bytecode listing
beside its text. `trace_file.rs`'s unit tests hold every event of
`trace.rs`'s `every_event` to a record with every field its line
writes, every record to one line, a range to UTF-16 columns, a shipped
body to the library and a `code` record's instructions to the text
listing's; `tests/cli.rs` a run's file -- its header, every record
parsed, every body number described before a record names it -- and the
two refusals. The analyzer's `tests/protocol.rs` pins
`newspeak/methodNames` and `newspeak/moduleIdentity`. In the plugin,
under `test`: `NewspeakRunConfigurationTest` the tab's options on the
command line, its greying and the free field's refusal;
`WatchedMethodsTest` the list, its storage, the patterns a run takes
and its filters; `TraceFileReaderTest` and `TraceRunTest` the records,
a partial line, a refused version, a large file read in bounded batches
and the states; `TracedRunsTest` the tabs and the file's deletion;
`TabsTest` the four views; `DecorationsTest` the icons, the hints, the
eyes, staleness, the checksum over an emoji and over CRLF line ends,
and a library file; `CodeTabTest` the caret's body, the chooser, the
linked highlighting and the editor tab. `NewspeakDebugProcessTest`, in
`debugProcessTest`, runs Show Frame Code against the real runtime. And
`TracedRunEndToEndTest`, in a Gradle task of its own, `traceFileTest`
(`runsLikeUnitTests`, `debugsWithTheRuntime`), runs a configuration
with `compile` and machine code checked and one method watched through
the platform's runner with the debug build of `ns-next`: the program's
output reaches the console without a trace line, the selected run has
the method compiled with its `code` record, and the program's editor
shows the method's icon.

## Planned: the stages

### Stage 4: the shell

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

- **Stage 4:** how an input is compiled -- a method on the workspace's
  mixin, as the Newspeak IDE does, or a block over a top-level activation
  (`FrameKind::TopLevel`) -- and how a new workspace variable becomes a
  slot (an atomic edit through `interpreter/edit.rs` and `install.rs`, or
  a workspace class rebuilt); whether earlier results are named (`r1`,
  `r2`, as `newspeak/AI_IDE_Support.ns:2658-2659` does); how an
  interrupt stops a running input; line editing without a new dependency.
