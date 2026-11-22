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
frame's code. The MCP server reads the same file for an agent: a traced
`debug_start` has its child write one the server owns, and four tools
answer it bounded, as the tool window's four tabs show it (decisions 36
to 39; [debugger.md](debugger.md), "The MCP server"). The shell is
stage 4 ("Planned: the stages"). The profiler
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
  --trace`, and as one record each into the file of `--trace-file`,
  which the plugin reads and `debug_start`'s `trace` has the MCP
  server's child write, which the server's trace tools answer.
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

Decisions 1 to 9 were decided with the user on 2026-11-18, and the later
groups, each introduced where it begins, on 2026-11-20 (10 to 16),
2026-11-21 (17 to 35) and 2026-11-22 (36 to 39).

**1. One design, three stages: tracing, then printing code, then the
shell.** Tracing first, because it turns the reasons above into types,
which printing the inlining plan needs as well; the shell is the largest
and the most independent piece. The developer tool in the IDE came
between printing code and the shell as stage 3 (decisions 17 to 35),
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
`debug_start` takes the trace categories and the listings, which the
agent reads through tools of their own from a file the server owns
(decision 36); a new MCP tool answers a stopped frame's
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

**9. The lines reach the command line as the process's standard
error.** `--trace` is a flag of `run`, as the statistics are. The IDE
reads the same events from a file of its own instead (decision 17), and
so does the agent, from a file the MCP server owns (decision 36). The
DAP agent is not involved: it still writes nothing to the program's streams, and no
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
the IDE and the agent read both from a file (decisions 20 and 36). V8's code
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
server's tool `debug_code` passes the text on, a window of its lines
(decision 38). DAP's `disassemble`
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

Decisions 17 to 35, stage 3's, the developer tool in the IDE, were
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
since every block of a method is `[] in <method>`. V8 writes
`code-source-info` with offsets and an inlining table
(`v8/src/logging/log.cc:1385-1490`), Turbolizer's positions are
`{scriptOffset, inliningId}` (`v8/src/codegen/source-position.cc:118-127`);
HotSpot logs a method and a bytecode index only, which JITWatch maps
through the class file's line table -- a table this runtime keeps in its
metadata instead.

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
and closing a tab drops its data and its file, as a later run of its
configuration replacing it does (decision 35). Two runs can be compared;
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
the tool window. Methods whose roots stand at one place -- a class's
factory and the initializer bodies the compiler synthesizes for it,
which stand at its header -- share one icon: its tooltip names each
body with its state, and its menu holds a submenu of each body's
actions. With an empty list every traced method shows its state
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

**34. A run keeps an index into its file, not its listings.** The file
holds everything the run traced and listed, and the plugin is its
reader: of a listing of machine code and an inlining plan a run keeps
the fields its views decide by and where the record stands in the file,
and a view that shows one reads that record again. A run of the Are We
Fast Yet harness with `--timed`, every category and every listing,
writes 1.1 GB, of which 1,077 MiB are 10,379 machine-code listings of
635 bodies, one of them installed 1,108 times; kept whole they would
fill the IDE's memory, read once and mostly never shown. The bytecode listings,
one per body, about 2 MiB in that run, stay in memory, since every
machine-code listing names its stretches by them.

**35. A run's tab is replaced by its configuration's next run, as the
Run window's are.** A new traced run of a configuration takes the place
of the tab of that configuration's last run once that run's process has
ended, unless the tab is pinned, which the tab's menu offers as the Run
tool window's does; the replaced run goes as a closed one does, its
data and its file with it. A run still going is never replaced, nor
another configuration's, nor a run under the other executor: a traced
Debug replaces a traced Debug, a Run a Run, as the platform's Run and
Debug windows each reuse their own. IntelliJ's own consoles are reused
by that rule (`RunContentManagerImpl`), so that a run repeated while
working leaves one tab and a pinned one is kept to compare; where no
tab of the configuration qualifies the platform takes another's ended
one, which the tool window does not, since a traced run is kept to be
compared.

Decisions 36 to 39, the agent's way to the trace through the MCP
server, were decided with the user on 2026-11-22, one at a time, after
the MCP specification, Claude Code's limits on a tool's answer and the
MCP servers of Chrome DevTools, mcp-debugger, LLDB, Perfetto, Datadog
and Grafana, beside the JIT tools stage 3 rests on
([debugger.md](debugger.md), "The MCP server", decisions 18 and 19).

**36. The agent reads the trace from a file the server owns.**
`debug_start` takes `trace` and `trace_filter`, and `listings` -- any of
`bytecode`, `code` and `inlining` -- and `listing_filter`, passed as
`--trace`, `--trace-filter`, `--print-<kind>` and `--print-filter`, and
the server has the child write them through `--trace-file` into a file
of the server's, one per session ([debugger.md](debugger.md), "The
trace file"); no trace line enters the output the server's answers show,
so that the program's output stays the program's. Every summary of a
traced session carries one `trace:` line, how much the file holds and
the call that answers an overview: `trace: 412 MB, 16,636 events, 78
bodies, 4,692 listings — debug_trace {"session":1} for an overview`.
This replaces decision 9's path for the agent, the lines on standard
error, which the server collected with the program's output: a trace
there pushes the program's own output out of the summary's last 2,000
characters, a program could write a line that reads as the trace's, and
a listing a few MiB long swells whatever answer it falls into, which is
why the listings did not reach the agent at all. The file is decision
17's, typed, dropping nothing and held by tests, so that the server
reads records rather than lines and the agent and the IDE read one
format; Chrome DevTools' MCP server keeps a trace it records server-side
and answers its summary and its insights from it, never the raw trace.

**37. The agent's tools are a staircase and a query.** Four tools
mirror decision 29's four tabs. `debug_trace` is the overview, as
Methods is the way into a run without a filter: the counts per category
and five ranked lists -- `installs`, a body's installs by tier;
`abandoned`, its abandonments and revivals; `deoptimized`, its
retirements and invalidations with their reasons; `optimized`, its
copies by key; `declined`, the callees the plans declined, by reason --
each entry a method's name and body number and the exact call that goes
on from it. `debug_trace_method` is one method's journal: its bodies
with their numbers, its copies, its events in order with repetitions
folded, and its sends with their inlining decision and cache state, as
Events grouped by method and the editor's hints show them.
`debug_trace_listing` is a window of one listing, as Code and Inlining
show one: a body's bytecode, a copy's machine code or a plan, by its
index among the body's. `debug_trace_events` is the query: the events,
in the run's order, that a category, an event, a reason and a method
select, as Events filters them. The first answer is a menu and every
entry names its next call, so that an agent walks down from the run to
a method to a listing without guessing an argument: the shape JITWatch's
top lists, journal and TriView have, and Chrome DevTools' trace summary,
whose insights each name the id to drill into; the query covers what
the staircase leaves out -- the run-wide order, a cache's transitions,
one reason across the run --, as the Perfetto servers' SQL and Grafana's
bounded queries do beside their fixed tools. A journal folds each body's
events apart, and only its lifecycle -- `compile`, `osr`, `optimize`,
`deoptimize`, `code-zone` --, since a body's churn, compiled, abandoned,
revived, five hundred times over, reads as one line only where no other
body's events fall between its rounds, and a cache's transitions belong
to the send they happen at, which the sends part shows; a cycle of up to
eight steps is matched on its steps' category and event word, the
fields that differ between its rounds summarized beside it (`revived
by=entries ×229, by=decay ×49`, `bytes 6,208–6,400`), so that `compiled →
abandoned → revived ×554` is one entry where exact fields would break it
at every round whose counts differ. Every round's own fields, and the
order across bodies, are `debug_trace_events`', which the journal's head
names. The selector a send shows is the trace file's (`name`, decision
19's `sends`), so that no tool reads source.

**38. Every answer is bounded and says where it stopped.** The bounds
are fixed, as the other debugging tools' are: the overview's lists 10
entries each, a journal's parts and the events 50 an answer, a listing
window 150 lines, and at most 200 entries or 500 lines on request, a
larger `count` capped; `debug_code`'s window is a listing window too.
Every answer is text, as every tool's of the server is, without
`structuredContent`: it is what an agent reads, in fewer tokens. Every
page also stops at the entry or line that would take its answer past
`ANSWER_BUDGET`, 20,000 characters (`mcp/pages.rs`) -- but for the
page's first, which it takes cut to the room left, saying how many
characters it left out, so that a line longer than an answer never
makes a page of nothing whose next call is the call just made --: Claude Code warns
at a tool answer of 10,000 tokens and cuts one at 25,000 by default, and
20,000 characters stay at the warning even at two characters a token,
the density machine code's hex and registers might have, which nothing
measured, so that the budget assumes the worst plausible one. A cut
answer ends in `Showing <first>–<last> of <total> — next: <exact call>`
-- but each list the overview cuts, which says it in its head,
`installs (10 of 64) — next: debug_trace
{"session":1,"list":"installs","start":10}` --, the tool's name and
its arguments as one JSON object, `session` always among them, then the tool's arguments in its signature's order, then
`start` and `count`, as Chrome DevTools' pages end in their range and
the next page's call; a page that is the whole needs no such line, and
`debug_trace_listing` writes it always, since a listing's window is a
place in a text the agent reads in parts. A fixed bound is chosen over a
`max_tokens` argument, which Datadog's tools take: an agent asks for a
page as easily as for a size, and a fixed page is the same for every
agent.

**39. A body is named distinctly and asked for by name or number.** A
class's three initializer bodies have names of their own everywhere --
the trace's lines, the listings' heads, the trace file's `name`, the
debugger's frames, `disasm`, and so the plugin: `Demo/Account
initializer`, the full body; `Demo/Account initializer, own part`, the
part a level a `<:` clause adds runs; and `Demo/Account initializer,
standalone`, the body such a level runs when a program reaches it
through a mirror ([compiler.md](compiler.md), "The three initializer
bodies"). The spellings are written in one place,
`interpreter/instantiate.rs`'s `initializer_name` and the suffixes
beside it, which `trace_events/names.rs`, `debugger/stack.rs`'s
`frame_name`, the metadata's check and the analyzer's `disasm` and
`inspect` write through, and which the trace filter reads back
(`trace_events/filter.rs`'s `body_parts`), so that `Demo/Account...`
still selects every initializer body; one body that serves as both the
full body and the own part, as a class side's does, is named as the
full one (`InitializerBody::named_as`). Every trace tool
takes a method in the tools' grammar, a trailing `...` a prefix and a
method's name covering its blocks, or `#<n>`, a body by the number the
file gives it, described or not, since a name does not identify a body:
every block of a method is `[] in <method>`, and a body the file never
described has its number alone. A name that matches several bodies is
shown whole by `debug_trace_method`, every body with its number, and
asked back by `debug_trace_listing`, which names each candidate with its
listings and the call that picks it.

## Stage 1: tracing

**Built** (2026-11-18 to 2026-11-20;
`docs/superpowers/plans/2026-11-18-2153-developer-tool-stage-1-tracing.md`).
`ns-next run --trace=<categories>` writes one line on standard error for
every decision of the tiers and the caches in the categories named, with
its reasons; `debug_start` and the plugin's run configuration pass the
same flags on with a trace file, which the MCP server's trace tools and
the plugin read ("Stage 3: the developer tool in the IDE"; decision 36).

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
  `Demo/Account initializer`, `Demo/Account initializer, own part`,
  `Demo/Account initializer, standalone` (decision 39),
  `Demo/Account superclass clause`,
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
event about no body. A class's three initializer bodies are told apart by
their names, `{Demo/Account initializer}`, `{Demo/Account initializer,
own part}` and `{Demo/Account initializer, standalone}` (decision 39),
the braces holding the comma and the spaces.

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

#### The inlining events and a withheld plan

Where plans are printed (`--print-inlining`, whatever the print filter
then keeps), a compile whose plan the runtime withholds -- one naming a
scope whose code the heap no longer names, which `record_plan` cannot
name and which no compile at the tree's safepoint meets -- writes none
of its `inlining` events either, on standard error and in a trace file
alike; its `optimized` event still counts its `inlined-sites`. The
events go wherever their plan goes because the plan is a compile's
outcome where `optimize` is not traced: a compile's events are one
stretch that only its outcome ends, and a withheld plan would leave them
waiting for the next compile of the same copy, whose plan would
conclude both. The one decision is `inlined_scopes`, which the trace of
the events (`optimize_one`) and `record_plan` both ask. Without printed
plans every compile's events are written.

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
  the program, which writes the events through `--trace-file` into a
  file of the server's, never into its output, and the server's four
  trace tools answer them from it (decisions 36 to 39;
  [debugger.md](debugger.md), "The MCP server").
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
`crates/ns-cli/tests/mcp.rs` (`a_traced_session_writes_its_own_file`) and the
plugin's options by its `NewspeakRunConfigurationTest`; the
CLI's and the MCP server's tests strip every stress mode from the child,
and on a target without a JIT the MCP server's assert what the answer
says there (`installs: none`) while the CLI's assert nothing.

## Stage 2: printing code

**Built** (2026-11-20;
`docs/superpowers/plans/2026-11-20-1634-developer-tool-stage-2-printing-code.md`).
`ns-next run --print-bytecode`, `--print-code` and `--print-inlining`
print, on standard error, every body's bytecode when its mixin is made,
every install's machine code and every optimizing compile's inlining
plan, selected by `--print-filter`; the debugger answers a stopped
frame's code, which the agent's `nsNext/frameCode` and the MCP server's
`debug_code` pass on, and the MCP server's `debug_trace_listing` answers
a window of any listing a traced session's file holds.

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
  sends their site. A plan naming a scope whose code the heap no longer
  names is withheld, and with it the compile's `inlining` events ("The
  inlining events and a withheld plan").
- **No JIT.** On a target without `cfg(ns_jit)` bytecode is printed, and
  `--print-code` and `--print-inlining` print nothing.

### The listings and their text

`crates/ns-cli/src/listing.rs` writes each listing between a head line
`--- <kind> {<body>} …` and `--- end`, the body named as the trace names
it -- an initializer body as `{Demo initializer}`, `{Demo initializer, own
part}` or `{Demo initializer, standalone}` (decision 39) --, so that a
script cuts a listing out of standard error. The sink
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
- **`debug_start`** takes `listings`, any of `bytecode`, `code` and
  `inlining`, each passed as `--print-<kind>`, and `listing_filter`, one
  `--print-filter` each, refused without `listings`; the child writes
  them into the session's trace file, never into the output its answers
  show ([debugger.md](debugger.md), "The MCP server").
- **`nsNext/frameCode { frameId }`**, a request of the agent's own
  (`debug/agent.rs`), forwards `Command::FrameCode` for a frame of the
  stop and answers `{ body, bytecode, machineCode }`, each listing's
  text as `ns-next run` prints it, `machineCode` `null` for a frame
  without; an unknown frame id is refused, and so is a stop the program
  has left meanwhile, with "the program went on"
  ([debugger.md](debugger.md), "Launching, the start and the end").
- **`debug_code(frame?, start?, count?)`**, the MCP server's tool
  (`mcp/tools.rs`, `mcp/session.rs`), asks it for frame `frame`, the top
  one by default, and answers a window of the bytecode listing followed
  by the machine code's listing or `the frame goes on in the
  interpreter`, 150 lines by default and 500 at most, a cut window ending
  in its `Showing` line (decision 38; [debugger.md](debugger.md), "The
  MCP server").
- **`debug_trace_listing`**, the MCP server's window of a listing in a
  traced session's file, writes it as `--print-*` does, through the same
  `listing::View` (`listing.rs`, `mcp/trace_listings.rs`), its first line
  naming the listing in place of the head ([debugger.md](debugger.md),
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
`tests/debug_adapter.rs` `nsNext/frameCode`; `tests/mcp.rs`
`debug_code` and `debug_trace_listing`; and `mcp/trace_listings.rs`'s
unit tests every kind of record rendered as `--print-*` prints it, and
every listing of a whole run so
(`every_listing_of_a_run_is_rendered_as_it_is_printed`).
A test of machine code asserts the lines that must occur,
never a whole listing but that one, and the CLI's and the MCP server's
tests strip every stress mode from the child; without a JIT the MCP
server's assert what the answer says there, the CLI's nothing.

## Stage 3: the developer tool in the IDE

**Built** (2026-11-21;
`docs/superpowers/plans/2026-11-21-0145-developer-tool-stage-3-the-ide.md`;
decisions 17 to 35). The runtime numbers the bodies a trace names and,
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
<method>`. The numbers
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
UTF-8 bytes with every `\r` left out), the module's current text with
where it holds the body's source, if it does (`CurrentText`), and each
statement's instructions and each send's index, site, call and selector
tokens, from the debugger's metadata (`debugger/metadata/`), and the
selector the send instruction names, read from the compiled code. Every span
counts in the text the body's version was read in -- the module's text
as loaded, or as the install that made the version left it -- since
that is the text the body was compiled from: an edit installed before
or during the run changes the module's text around the bodies it leaves
alone, and the runtime keeps no text but the current one whole. So the
source span is the body's own (`CompiledCode::source`) and the checksum
its own source's, sliced from its version's text. The metadata is
compiled again from the body's own source, laid out as it was compiled
(`Cache::own_placement`): the family root's own source is laid where
the current text's declaration of the same member stands -- the
current text itself where it holds that source there -- the level
compiled again from that text, paired as the debugger pairs it, and
every span moved from where the root's source stands in that text to
the root's own span, the same bytes, so that a body whose counterpart
differs only in its layout -- a comment edited inside it -- keeps its
statements and sends, and none is ever moved across text laid out
otherwise. A span outside the root's source is the declaration's own --
a full initializer's statements of the superclass and of each `<:`
clause stand at the clauses, or at the declaration's name where it
writes none --: it moves by the same distance where the body's version
text holds the same bytes there, its true place, and has no place
where it does not -- an edit moved the clause and the body apart, or
the place lies outside the declaration's text or across a hole in it
-- (`moved_into`), never a place made up; every statement and send is
kept either way, so that every initializer body -- the full one, the
own part, the standalone body, the superclass clause -- is described
with all its statements and sends wherever it runs, its slots'
statements linked as any. The
current text is handed where its declaration of the member holds the
root's own source, with where the body stands in it: at its own span,
or where an edit above the member moved it. Each family's placement is
kept in the debugger's metadata cache, which every install clears and
which drops its dead entries once it holds twice what lived at the last
drop; the families of one level described in one flush share the
level's compile from the current text, which the flush's end lets go of,
since its pairs hold every running body of the level. The
host places the body in the text it read the module's file in where
that holds the body's source at its span, by the checksum -- a body
compiled from the file, whether an edit left it alone, moved it or
superseded it -- and otherwise in the current text the description
hands, every span moved with the body's source and one outside that
source placed in neither, since only the source is known to stand
there, so that a body an edit
compiled is placed in the edited text, and one a later edit moved where
it stands now. A version without a text of its own, a builtin's or an
image's without source, is described as the module's current text
holds its span; a body the compiler synthesizes, which has no source --
a factory's, and an initializer of a declaration that writes none --
is described where its metadata places it as its own version declared
it: a factory at its header's factory pattern, or the class's name for
a header that writes none, an initializer at its declaration's header,
with the statements and sends that metadata gives (a factory none). Its
source and checksum are those of the text its version was read in, as
every other body's, and the current text is handed only where it holds
the same text at the body's place, so that a factory an edit renamed
the header of, which compiles alike with the new one, is placed in a
text that still holds its own header and never at the new one. A block's family root is
named even when its code is gone by the flush -- a collection reclaimed
the version that held it while an event of the block waited -- by the
number the root was given while it lived or one given then, which every
block of the family shares; such a root is never described, since a
description is made from the code. A description is made from the
body's `Rc<CompiledCode>` and its mixin, never a handle, and never in a
leaf; nothing on the hot path changes, since a site records only what it
recorded before descriptions existed.
A sink's `flushed`
is called at the end of every flush's deliveries. A bytecode listing
carries its bodies' instructions apart (`ListedBody`,
`ListedInstruction`; `bytecode::listed_instructions`, the one walk the
text listing's lines are made from too, `instruction_lines` and
`disassemble` alike), so that a record lists them without parsing text
and the two never differ.

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

The file has two readers: the plugin, which a traced Run or Debug hands
a file of its own ("The plugin's half"), and the MCP server, whose
traced sessions' children write into a file the server owns and indexes
as it grows (decision 36; [debugger.md](debugger.md), "The MCP server",
"The trace file"). Both accept what they do not know of version 1 --
an unknown record, an unknown category's event, a header without its
categories or listings, taken to trace and list everything -- and refuse
another format or version.

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
  deferred copy -- and where neither can arrive takes none of them,
  since whether the copy was installed is never heard. A compile's
  inlining events are one stretch, followed by its outcome: any other
  record but a `body` or a `bytecode` record, which stand wherever a
  body is first named, ends it, and a compile whose outcome a filter
  withheld is dropped there. Where plans are printed, a compile whose
  plan the runtime withholds traces no inlining events either ("The
  inlining events and a withheld plan"), so that two compiles of one
  copy in a row never run together into one stretch that the second's
  plan concludes.
- `{"kind":"body","body":N,"name":…,"family":N|null,"module":{"id":…,"version":…|null},"source":…,"range":[…],"placed":"source"|"current"|null,"checksum":…,"statements":[…],"sends":[…]}`,
  before the first record that names `N`: the body's name as the text
  trace writes it (`Demo>>main:args:`, `[] in Demo>>run`); for a block,
  `family`, the number of its family's root -- the method, factory,
  initializer or superclass clause whose code created it, at any depth
  --, which is described before it unless its code was gone by then, and
  which the cache transitions of the block's sites name, `null` for a
  root; its module,
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
  `{"at":index,"site":n|null,"call":[…],"selector":[[…],…],"name":"…"|null}`,
  each send instruction's index, its site as the `caches` category
  numbers it (`null` for a send no cache serves), the call from its
  receiver to its last argument, each token of its selector, and the
  selector as the compiled send names it (`at:put:`, a super send's
  without `super`, a mirror send's mirror selector; `null` for an
  instruction that names none), read from the instruction rather than
  the source, so that a reader names the sends of a body no text holds
  as it was compiled, without reading source; both empty for a body
  whose metadata the runtime could not compile again alike from its own
  source. A statement's or a send's range, or a token's, is also `null`
  in a body whose own range is placed, where it lies outside the body's
  source: a full initializer's statements of its superclass and its `<:`
  clauses stand at the clauses, or at the class's name where it writes no
  clause, which every class without a superclass clause has -- the
  statement listed all the same, so that the instructions keep their
  statements' numbers, and nothing linked to it. Every range is counted in a text that holds the body's source
  as it was compiled: the program file's text as read where it holds it
  at the body's span or at the span the runtime's current text holds it
  at, else the runtime's current text where its declaration of the
  body's member holds it; every range is `null` where neither does -- a
  body an edit compiled and a later edit replaced --, whose `checksum`
  is still that of its own source, so that a reader decorates nothing of
  it rather than decorate the wrong place. `placed` names the text the
  ranges count in: `"source"`, the text `source` names -- the program's
  file as read, or the core library's file, whose text no edit changes
  --; `"current"`, the runtime's current text of the module, which an
  edit made and no file holds, so that a reader holding the file places
  nothing of the body there; `null` where every range is. A block whose
  family's root was described is placed only where the root was: in the
  file at its distance inside the root there, never by its own span; in
  the runtime's current text, the core library's among them, as the root
  is; in none where the root is in none: its bytes alone may stand in
  the file at its span inside the file's other version of a method an
  edit replaced around it, or at another place altogether. A reader of a record without
  `placed` takes `"source"`.
  A body the compiler synthesizes -- every factory, and the
  initializer bodies of a declaration that writes none, a class without
  slots among them -- is described where its metadata places it, its
  factory pattern or its declaration's header, and placed only where its
  own version declared it ("The runtime's half"). Three kinds of
  body take a number and no `body` record -- one no mixin holds, one
  whose source no text holds, as `VmMirror class>>new`, the factory of
  the VM mirror's class, which has no declaration, and a block's family
  root whose code was gone by the flush --, so a reader accepts a number
  it has no description of, and the MCP server's tools write it
  `#<n> (not described)`.
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
- `{"kind":"bytecode","body":N,"bodies":[{"body":N,"kind":…,"in":M|null,"at":i|null,"arguments":a,"registers":r,"instructions":[{"index":i,"statement":s|null,"text":…}]}]}`,
  a body's bytecode, the root and then the blocks it makes in the order
  the text listing nests them, each body with what the text's line above
  its instructions writes -- its `kind` (`CodeKind::word`: `method`,
  `block`, `initializer`, `superclass clause`) and its counts of
  arguments and registers, a reader taking a body without them as one
  whose line has none --, a block's `in` and `at`, the body that makes
  it and the index of its `MakeClosure` there, under which the text
  nests it, `null` for the root, and each instruction's text as the
  text listing writes it after its index and statement columns.
- `{"kind":"code","body":N,"tier":"baseline"|"twin"|"optimized","key":…|null,"entered":…|null,"architecture":…,"base":address,"bytes":n,"instructions":[…],"data":[…],"scopes":[…],"mark":offset|null}`,
  an install's machine code, decoded by `ns-cli` as the text listing
  decodes it (`listing::annotated`, the one walk that places the
  positions and the annotations on the instructions for both): `base`,
  the address the code was installed at as a number, the text listing's
  `base=` (decision 14), an address of the run, which nothing in the
  record is placed by; each instruction
  `{"offset":o,"text":…,"scope":s|null,"at":pc|null,"out-of-line":bool,"annotation":…|null}`,
  its offset in the body, its text, the scope and program counter of the
  stretch it stands in (`null` before the first position and out of
  line), whether that stretch is out of line, which tells the two apart
  -- so that a reader names a first stretch out of line as the text
  listing does --, a reader taking a record without it as `false`,
  what a call or jump out of it reaches as the text listing's
  annotation writes it (`leaf miss`, `cell site 3`; two on one
  instruction joined by `; `, which no annotation holds -- the macro
  assembler's annotations are a closed set, and a debug build asserts
  it where they are joined (`trace_file.rs`'s `joined_annotations`) --,
  so that a reader splits them there), and, only where instructions before it
  emitted no code, `"passed":[{"scope":s,"at":pc}|{"out-of-line":true}]`,
  the positions they left beside its own, in order, each of which the
  text listing names on a line of its own; each data word `{"offset":o,"word":…}`,
  its offset from the first data word, as the instructions' texts name
  it (`data+0x0008`), and what it holds (`budget word`); each scope
  `{"scope":s,"body":N,"parent":[scope,at]|null}`, and with
  `"lines":[…]`, its body's instruction lines as the bytecode listing
  writes them, where no earlier `bytecode` record of the file lists the
  body -- one no mixin holds, a factory a host's instantiation built for
  itself, has none ever, and one the print filter or the printing left
  out none in this file --, so that a reader writes every position line
  as the text listing does from the file alone (`ListedBodies`).
- `{"kind":"inlining","body":N,"key":…|null,"entered":…|null,"scopes":[…],"arms":[…],"declined":[…],"outcome":…}`,
  an optimizing compile's plan: its scopes as a `code` record's; each
  arm `{"scope":s,"at":pc,"receiver":…,"inlined":{"scope":s}|{"always":"identity"}}`;
  each decline `{"scope":s,"at":pc,"callee":N|null,"reason":…}`; and
  `{"installed":bytes}` or `{"refused":"<reason>"}`.

`event_record`, `listing_record` and `body_record` each match their
input exhaustively, so that a new kind of event or listing, or a new
field of a description or a listing, does not compile until it has its
record.

The host keeps each program module's file text as read, with its line
index, and a line index per runtime text a description hands
(`ModuleSources`, `placing`), and names a program module's source by the
file it was read from.

### The plugin's half

The developer tool lives in the plugin's `devtool` package, its
decorations in `devtool/editor/`, its views in `devtool/ui/`, the
records and a run's model in `devtool/model/`; "Show Frame Code" is the
debugger's (`debugger/`).

**The "Developer Tool" tab** (`DeveloperToolTab.kt`, decision 21) of a
Newspeak run configuration has a checkbox for each of the seven
categories, with "All", one for each listing kind, each saying on hover
in a sentence what it holds -- `compile`'s, `deoptimize`'s and
`code-zone`'s also which of the editor's tier icons need it --, and the
link "Manage watched methods…";
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
module the run has -- one its project file names, by a path relative
to it or absolute, as ns-next resolves it, by its `as` or else by the
id and version `newspeak/moduleIdentity` answers, the single module
run from its source, or a shipped one -- since `ns-next run` refuses a
pattern naming no module of the program with exit status 2. The start
asks the server off the event dispatch thread, under a modal progress
where it is on it, and with no client yet starts one and waits for it
to run, once for the whole start, at most 10 s
(`CLIENT_START_MILLISECONDS`), a server that stopped or failed ending
the wait at once, and on a thread other than the event dispatch
thread under a background progress, cancellable, where a wait is
possible, waited for at most that wait, a request's timeout for each
module and a second (`readBound`), a task the platform never ran
answering as a cancel does -- so that the first run of a session, before any editor
started the server, keeps its patterns; a module whose identity no answer gives is left out, since a
run refused is worse than a watch left out. A kind no entry is left
for is not asked for at all, since a run without a pattern would trace
every body; where that leaves a run that asks for events or listings
nothing to trace, a balloon of the plugin's notification group says so
and names the entries left out (`nothingTracedNotice`), which name no
module the run knows of.

**Traced runs** (`TracedRuns.kt`, decisions 24 and 26). Run and Debug
start their process through `NewspeakRunState.startProcess` alike; a
run that traces or lists anything is handed a temporary file,
`newspeak-trace/trace-<uuid>.jsonl` under the IDE's temporary
directory, and gets a `TraceRun` named by its configuration and the
time and a tab of the tool window, and is selected. A run's tab takes
the place of the tab of its configuration's last run under the same
executor (decision 35) where that run's process has ended and its tab
is not pinned -- Pin Tab in the tab's menu, the platform's content
pinning (`Content.isPinnable`), a tab starting pinned where the advanced
setting "start run configurations pinned"
(`start.run.configurations.pinned`) says so --, which goes as a closed
one does, once the new tab is added and selected, so that the selection
never passes through another run. The platform's windows choose the tab
they reuse the same way (`RunContentManagerImpl`'s
`getContentFromManager`): an unpinned tab whose process has ended, by
the configuration's name, the selected one first, then the first in the
tabs' order, in the one window of the executor. The tool window differs
where the platform's rule does not fit one window for both executors
and runs kept to compare: it matches the executor itself, Run and Debug
sharing it, and it never falls back, as the platform does, on another
configuration's ended tab, so that a run is replaced only by its own
configuration's. Closing its tab
drops the run and its reader at once, and its file once its process has
ended, since the process writes it until then, the newest run left becoming
the selected one, whose decorations the editor shows, and every
listing tab built from it (`CodeListingFile`) keeps its text and lets
the run go, linking nothing from then on. No trace file outlives the
IDE session that wrote it: the files are the application's
(`TraceFiles.kt`), each named live from before its process starts
until its run lets it go; a run whose project closed before the run
was registered lets its file go once its process has ended, as a closed
tab does; and the first project opened
in an IDE session sweeps the directory of every file no live run
names (`TraceFilesSweep`), which a deletion an earlier session's end
cut short, or a file a still running process held open, left behind
-- application-wide, so that it never deletes another open project's
running trace. A file's size on disk is bounded by what its run traces,
as any log's is: the categories, listings and filters a run is started
with are what keeps it small. The reader (`TraceFileReader`) follows the file on a
pooled thread, looking for appended bytes 50 ms after it last found
some, each look after a quiet one waiting twice as long, up to 500 ms;
keeps an incomplete last line until its newline arrives, parses each
record with the platform's Gson (`TraceRecords.kt`), and publishes
them in batches of at most 5,000 records through `invokeLater`, no
record waiting more than 200 ms for its batch, the next batch handed
over only once the event dispatch thread applied the last, so that a
backlog waits in the file and never in the thread's queue; once the
process ends it reads to the end. A first line that
is no header of `ns-next-trace`, a version other than 1, a malformed
line and an unreadable file end the reading with a message in the run's
tab, after the records read before it. A body placed in the runtime's
current text (`"placed":"current"`) is read with every range `null`
(`Body.placedInCurrentText`): its lines are those of text no file holds,
so nothing decorates, links or navigates into the file there. A `TraceRun` (`TraceRun.kt`),
read and changed on the event dispatch thread only, keeps the records
by body number, the header's categories and the filters the run was
started with, and decides each body's state from its events in order
-- interpreted, baseline, twin, optimized with its copies counted,
deoptimized once its last copy is gone, abandoned until revived --, a
`code` record no event announced counted as its install. **A tier is
shown only where its end can be heard**, never one that may be over:
optimized, with its copies, only where `deoptimize` (a retirement, an
invalidation) and `code-zone` (a copy freed, the zone replaced) are
traced; baseline and twin only where `compile` (an abandonment) and
`code-zone` (the code freed, the zone replaced) are; interpreted where
`compile` is; deoptimized and abandoned as they are heard -- each for
the body's own events passing the run's filter, and a header that does
not name its categories taken to trace them all. An optimized body
whose copies' end cannot be heard answers its baseline tier if that
one's can, and a body whose every tier may be over has no state, as
has a body the trace names only as a callee, a scope, a cache's family
or by a bytecode listing, since a body is described whenever any
delivery names it. By the same rule, each send's outcome: the inlining
decision of the last optimizing compile that planned it and whose
installed copy still lives -- held until the compile's outcome arrives,
never taken where the header says none can, and dropped unshown where the next
record but a description or a bytecode listing is no part of the
compile, its outcome withheld by a filter, or the file ends, so that
at most one compile's decisions wait; the hint's tooltip shows the
event that made the decision shown, an installed copy's, never a
refused, deferred or dropped compile's; gone with the copy's retirement,
invalidation, freeing or the zone's replacement, and shown only where
`deoptimize` and `code-zone` are traced --, and the state of its cache, `mono`, `poly <n>` with
the receivers seen since it was last vacant, or `mega`, a block's
sites by its family's root. A run keeps its last 200,000 events as
such (`KEPT_EVENTS`), the Events tab's rows, dropping the oldest; what
the events say of a body's state, a send, a cache and the Methods
tab's and the icons' counts is kept beside them for the run's length,
so that nothing the editor or the other views show changes when an
event is dropped. Of a machine-code listing and an inlining plan a run
keeps only its entry (decision 34; `CodeEntry`, `PlanEntry`, which
`TraceRecord.indexed` makes as the reader reads each line whole): of a
listing its body, tier, key, entered application, base and bytes, by
which the Methods tab, the Code tab's chooser and the state icons
decide; of a plan its body, key, entered application and outcome, by
which the Inlining tab names its root and a send's decision is
concluded; and of either the place of its line in the file, its first
byte's offset and its length in bytes, counted by the reader as it
splits the bytes it reads, a line across reads included. The bodies'
descriptions, one per body, the bytecode listings, one per body, and
the events kept stay in memory. A view reads a record again from the
file (`readRecord`, `TraceRun.readCode`, `TraceRun.readPlan`) with the
reader's own parsing, on a pooled thread, since one listing's line can
be a few MiB -- a machine-code listing's text built there too, from a
copy of the names and bytecode it needs made on the event dispatch
thread (`ListingNames`), so that only the viewer's text is written there,
about 40 to 150 ms for 4 MiB in a test's IDE --, its outcome delivered
on the event dispatch thread in the modality it was asked in
(`RecordReads`); it keeps what it shows until the record arrives, says
in its place that it is reading only once a read has taken 300 ms
(`SLOW_READ_MILLISECONDS`), and why it cannot where the file is gone,
unreadable or shorter than the record, any other failure of a read
logged and said as one; a record whose read failed is read again when
the user asks for it again; the file lives while the run's tab does.

**The tool window**, "Newspeak JIT" (`ui/`, decision 29), has a tab per
traced run (`RunPanel`) of four views, above them the error that ended
the reading of the run's file, if one did, and a line naming the
categories the run's header lacks that the tier icons need
(`TraceRun.missingTierCategories`): "No tier icons: trace … too"
without `compile`, "No icons of baseline, twin or optimized code: …"
without `code-zone`, "No icons of optimized copies, which show as their
baseline or twin tier: …" without `deoptimize`. Methods lists every method the
run names -- described, or named by its number alone, `#<number>` as
the events name it -- a block counted as its method's, with its state,
blank for one the run has none of, copies, deoptimizations,
abandonments, the code bytes of its largest install of each tier, a
column each for baseline, twin and optimized, from the installs'
events and listings alike, and the callees declined in it by the
compiles whose copy was heard installed, sortable, a double click
opening it; a block whose root's code was gone by the flush counts as
that root's method, named by its blocks' name. Events is the trace as a table, filtered by category and by a part
of the method's name, grouped by method on request, a double click
going to the event's place in the source, and with "Text" the same rows
as the text trace's lines; it shows the events the run keeps, and says
how many earlier ones were dropped. Code shows the bytecode and machine code of
the body at the caret of a source editor, the innermost whose range
holds it, with a chooser of its copies -- its bytecode, then each
install, baseline, twin and optimized per key, the chooser wide enough
to paint every label whole (`CopyChooser`): the UI paints the closed
value in the combo's width less its right inset, the arrow button the
layout makes and the padding, and cuts a wider label, while its
preferred width counts the arrow as a square button and the label with
the renderer's border, so the chooser wants the widest label measured in
its font without that border plus what the UI takes, bounded by the
room the tab leaves but never below its shortest label's -- over a read-only
viewer of the listing in the text listing's format, built from the
records, an install's listing read from the file when it is chosen,
the text shown before it kept until it arrives as text alone -- no line
of it linked and "Open in Editor" off, so that nothing takes it for the
chosen copy --, a read for a copy no
longer chosen when it arrives dropped, a batch that offers the
copies again with the chosen install unchanged leaving the viewer's
text, caret and scroll as they are and reading nothing, and a copy
whose read failed read again when it is chosen again or its body shown
afresh, never by a batch, a copy
installed again under the same tier and key numbered
(`baseline (2)`), so that the choice a batch keeps is the same install
-- two `optimized` entries of one key are two installs of that copy,
the second compiled anew after the first was retired or freed;
"Open in Editor" (decision 30) opens the copy shown in a
read-only editor tab that stays (`CodeListingFile`), the tab already
open on it if there is one. Source, bytecode and machine code are
highlighted together (`LinkedHighlighting`): the caret's statement in
the source and the listing's lines whose instructions or machine-code
stretches belong to it, in either direction, one colour per statement
from six keys of the colour settings' own; only a Newspeak file's or a
listing's caret moves the link, and an edit that makes a method stale
takes its link away at once (`DecorationsService.addStalenessListener`).
"Scroll with the editor", a toggle in the tab's bar, off until the user
turns it on and remembered per project (`CodeTab.syncWithEditor`),
makes the link scroll too: a source caret on a statement the shown
listing links scrolls the viewer to the listing's lines of it, and a
viewer caret on a line linked to a statement scrolls every source
editor that shows its body -- the selected editor of each window, a
background tab showing nothing -- to the statement, bringing the body's
file forward, or opening it, behind the viewer and with its caret where
it is where no editor shows it, by the rule a view opens a file by, so
that a body without a range in its file is opened nowhere, and a file
found only after the toggle went off or the tab closed is not opened;
each only where the lines are not shown already, and only once the
editor's viewport shows -- for the viewer, once it has an extent, which
a tool window never shown leaves it without and a hidden one keeps. It scrolls and never moves a caret. Turning the toggle
on brings the viewer to the source's caret at once. What its opening
of a file sets off is not followed, so that neither direction sets the
other off, at whatever time the platform finishes: everything within
the opening's call; the one selection change the opening makes, which
the platform publishes from a coroutine, after the input that caused
it, and none where the selection did not change; and, until the
opening is over, every caret move in any editor of the file but one
made by the user's click or key. The opening is over once the platform
has told that it opened the file, which it does after restoring the new
composite's editors' saved state (`EditorComposite.applyFileEditorsInEdt`),
and the file's editors are loaded; a file open before for which the
call made no editor, a tab brought forward, restores nothing and waits
for its editors alone. It is dropped when nothing opened, when the file is closed or
when its editors are released first. The tab then follows the editors
as any again, a debugger's step included, and scrolls them to the
statement the viewer's caret names by then, queued in the editor's own
modality so that it comes after the platform's own scroll to the
restored caret; the wait ends with the tab, the toggle going off and
the editor's release, and a queued scroll runs no more once the tab is
gone or the toggle off. Turning the toggle off ends every opening at
once. The plugin's
listings are the text listing's: the bytecode with its body's line and
each block nested eight columns deeper under the `MakeClosure` the
record names by `in` and `at`; the
machine code's head with `base=` from the record's `base`; a first
stretch out of line named, by the record's `out-of-line`; each position
an instruction's `passed` names on a line of its own, linked as the
stretch's own is; and two annotations of one instruction each after
`  ; `. `ListingsFixtureTest` holds the plugin's listings to the text the
CLI writes for the same records, from the fixture `trace_listings.rs`'s
`the_listings_fixture_is_current` keeps current. Inlining is a tree per
optimizing compile in the file's order, each batch's plans appended so
that the tree keeps its expansion and selection, each plan's root made
from its entry and its nodes read from the file as it is first
expanded, a plan read kept from then on, so that the tab holds the plans
the user opened alone, and one the file did not give read again at its
next expansion, its scopes nested
under the scope whose send inlined them, the arms and the declines
under each, the outcome at the root, a click going to the send. A view
opens a program body's file by its path and a shipped body's under the
project's core library (decision 33), looked up without a refresh on
the event dispatch thread, a file the virtual file system does not hold
yet refreshed on a pooled thread and opened once found.

**The editor** (`DecorationsService`, decisions 27, 28, 32 and 33)
decorates every open editor of a body's file, a program's or the core
library's, from the selected run's records: an icon at each method's
header (`StateGutterRenderer`) -- its state, badged when the method is
watched, its tooltip the counts, the last deoptimization's reason and,
for a method shown as its baseline or twin tier because its optimized
copies' end cannot be heard, the categories missing (`TraceRun.unheardCopies`),
a click opening Watch or Unwatch, Show Code, Show Inlining and Open in
Tool Window -- for a method the run says how it ran; a neutral icon
with the same menu, "The run listed this method's code but traced
nothing of how it ran", for one the run lists the bytecode, machine
code or a plan of but says no state of (`TraceRun.isListed`), its
tooltip naming the categories the run lacks (`TraceRun.missingTierCategories`)
where the run traced events of the method but could not say whether
its tiers are over; the eye
otherwise if it is watched; the methods whose roots stand at one
offset -- a class's factory and its synthesized initializers at its
header -- one icon between them (decision 28; `sharedIcon`), drawn with
the state of the body of the lowest number, badged if any is watched,
its tooltip a line per body, `<name>: <that body's tooltip>`, and its
menu a submenu of each body's actions -- and a hint after each send
the run says something of
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
frame until the last editor of the tab closes -- a split's closing
leaves it following, which is decided once the close is done -- or the
session ends, replacing only the text
between the longest common start and end of the old and the new, so
that a step leaves the view where the text is unchanged, then putting
each editor's caret at the marked line and scrolling it into view only
where it is not shown, that one line coloured whole in every editor of
the tab as the debugger's execution point is in a source editor, while
the program stands at a stop, until a text marks none or the follower
ends, its lifetime the
session's (`NewspeakDebugProcess.sessionLifetime`); a refusal is shown
as a notification of the plugin's group with the agent's words
([debugger.md](debugger.md), "The JetBrains plugin").

### Tests

`crates/ns-runtime/tests/trace.rs` and `trace_events/names.rs`'s unit
tests pin the numbers -- unique, and never a dead body's for the body
that took its address, a block's family named after its root's code is
gone -- and the descriptions, an edited module's bodies each in its own
text, a body whose counterpart differs only in a comment with its
statements and sends, a body a later edit moved where it stands
now, and every initializer body -- of a plain class and of one a
superclass and a `<:` clause make -- with its slots' statements and
sends, every factory and synthesized initializer at its metadata's
place (`every_factory_body_is_described`), each send's `name`, and
`debugger/metadata/cache.rs`'s unit tests a synthesized body placed
where its own version declared it and the place of a span
outside a body's source: moved onto the same bytes, and none where the
bytes differ, the place lies outside the declaration's text or across a
hole in it;
`crates/ns-runtime/tests/printing.rs` the structured bytecode listing
beside its text, each body's line among them. `trace_file.rs`'s unit
tests hold every event of `trace.rs`'s `every_event` to a record with
every field its line writes, and the plugin's fixture
`src/test/resources/devtool/events.jsonl` -- each event's record, its
bodies numbered apart, and its line -- to what this build writes
(`the_events_fixture_is_current`, written again by the ignored
`regenerate_the_events_fixture`), a bytecode record to each body's
line, every record to one line, a statement and a send without a place,
or placed outside the body's source, to a `null` range beside the
placed ones, a range to UTF-16 columns counted in
the file's text where it holds the body and else in the text the
description hands, the file placing a moved body where it holds it,
each record's `placed`, a block in its root's text at its distance
inside the root, a shipped block in the library's, a body no text holds to `null` ranges, a shipped body to the library and a `code` record's
instructions to the text listing's, an out-of-line stretch told from
no position; `tests/cli.rs` a run's file -- its header, every record
parsed, every body number described before a record names it, an edit
installed during the run and the bodies it superseded or moved each
described in the text it was compiled from and said placed there, a
library method's blocks placed in the library inside their root, a recommented body's
statements at their file lines and a body a later edit moved where it
stands, and a whole run of the benchmarks with every listing describing
every body it numbers but `VmMirror class>>new`
(`a_whole_run_describes_every_body_it_numbers`) -- and the two
refusals; `trace_file.rs`'s unit tests also the annotations' separator,
and `mcp/trace_listings.rs`'s a `code` record's `passed` written back as
the text listing's position lines. The analyzer's `tests/protocol.rs` pins
`newspeak/methodNames` and `newspeak/moduleIdentity`. In the plugin,
under `test`: `NewspeakRunConfigurationTest` the tab's options on the
command line, its greying and the free field's refusal, and the bound
of the start's read of the modules;
`WatchedMethodsTest` the list, its storage, the patterns a run takes
and its filters, an absolute module path, the wait for the server and
the notice of a run left nothing to trace; `TraceFileReaderTest` and `TraceRunTest` the records,
a partial line, a refused version, a malformed line and an unreadable
file after records, a body placed in the current text read unplaced, a
large file read in bounded batches, a backlog handed over a batch at
a time once the last was applied, the waits of a quiet file, the states, a state only where the
trace knows it, a tier only where its end can be heard, events and
listings alike, a send's inlining going with its copy and shown
only where its end can be heard, none where no outcome can arrive, a
compile whose outcome was withheld dropped, the declines of installed
copies alone, and the events kept beside counts that keep every one, a listing's
place in bytes across reads and characters of several bytes, a run of
a thousand listings and plans keeping none of their text anywhere it
reaches, and a
listing its file no longer gives read as an error that names why;
`TracedRunsTest` the tabs, closable as the registered tool window
declares them and pinnable, the file's deletion, at a tab's close, once its process
ended for a tab closed while it runs, and for a project closed before
its run registered once its process ended, the sweep, a closed run's
listing tabs, and a rerun replacing its configuration's ended, unpinned
tab under its executor and deleting its file, the selected one first,
then the first in the tabs' order, the selection going to the new run
alone, but neither a pinned one, one the setting started pinned, a
running one, another configuration's nor the other executor's;
`TabsTest` the four views, a dead root's blocks counted as its method's, the bytes of
every tier and a body the run did not describe, the run's line of
missing categories, the plans and the grouped events that arrive after
their tab was made shown a row each, the inlining tree kept across a batch, a plan read
as it is first expanded, on a pooled thread, kept across a collapse,
saying it reads only once a read takes long, saying why in words where
its file is gone and read again after a failed read, the Events
tab's oldest rows dropped with the run's; `EventsFixtureTest` the
Events tab's text of every event of the fixture equal to its line;
`DecorationsTest` the icons, a deoptimized one, the neutral icon and
its menu for a method only listed, the categories it lacks where its
tiers cannot be heard, the categories an optimized method
shown as its baseline tier needs, none for a body placed in the
runtime's current text, none for a method a caches-only run
or a filtered-out callee says nothing of, the hints, the eyes,
staleness, the checksum over an emoji and over CRLF line ends, a
library file, and one icon for the bodies at one header, naming each
with its state; `ListingsFixtureTest` the plugin's bytecode and code
listings, a passed position and two annotations on one instruction
among them, equal to the CLI's text for the same records, from the
fixture `src/test/resources/devtool/listings.jsonl`; `CodeTabTest` the
caret's body, the chooser, a listing
built once per choice and read on a pooled thread, the text before it
kept, linking nothing and opening nothing, and the notice of a slow read, a batch that keeps the chosen install
leaving the viewer alone, a read for a copy no longer chosen dropped, a
listing whose file is gone saying so in words, a statement without a
range linking no source, a failed listing read
again when chosen again and never by a batch, a read that throws shown
as a failure, a listing's text built off the event dispatch thread from
the names that stood when it was chosen, every label
painted whole under the IDE's `DarculaComboBoxUI` with the New UI's
renderer and the chooser bounded by a narrow tab, a copy installed
again, the listings, a block nested where the record names its
`MakeClosure`, the linked highlighting,
kept through another editor's caret and gone with an edit, scrolling
with the editor in either direction, off by default and remembered,
a background tab brought forward and scrolled, the user's clicks and
keys followed as production makes them, a file opened for it scrolled
once loaded and told opened, after the platform's scroll to the
restored caret and to the viewer's statement of then, followed while
its opening lasts in the user's input alone, in every editor of it,
and in everything after, a file closed meanwhile followed when opened
again, a file found after the toggle went off not opened, a viewer not
shown scrolled once it shows and a hidden one with its extent at once,
the toggle turned on scrolling at once,
everything followed again and nothing scrolled once the toggle is off
or the tab gone, a line of no statement scrolling nothing, and the
editor tab; `FrameCodeFollowerTest` the frame's code written as a
change that shows the marked line, that line alone coloured as the
execution point in every editor of the tab, only the tab's last editor
ending the follower, and a deferred step expiring with its lifetime.
`NewspeakDebugProcessTest`, in
`debugProcessTest`, runs Show Frame Code against the real runtime, the
marked line coloured and the colour gone at the session's end, the
colour gone while the program runs and back at the next stop against a
fake agent, and
its refusal against a fake agent. And
`TracedRunEndToEndTest`, in a Gradle task of its own, `traceFileTest`
(`runsLikeUnitTests`, `debugsWithTheRuntime`), runs a configuration
with `compile`, `code-zone` and machine code checked and one method watched through
the platform's runner with the debug build of `ns-next`: the program's
output reaches the console without a trace line, the selected run has
the method compiled with its `code` record, read back from the file the
runtime wrote at the place the reader counted, under the Run executor,
and the program's editor shows the method's icon; and a traced Debug of
it is told to the runs as Debug's; and a run with `compile`, `optimize`,
`deoptimize`, `code-zone`, machine code and plans, as a user starts one,
shows a row in the Inlining tab for each plan the runtime wrote; and a
slot's initializer of a class instantiated often links its source and
its code in the Code tab both ways.

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
