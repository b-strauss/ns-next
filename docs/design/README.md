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
| Values, objects, mixins, classes, instantiation | [object-model.md](object-model.md) | Built, §6.2's mixin application included: each `<:` clause adds an ordinary class level, so lookup, `super`, `outer` and the caches need nothing new. §3.2's class hierarchy is reified: every class object's class is its metaclass, over `Class`, and every metaclass's class is `Metaclass`. All four of §3.1.1's value conditions are enforced, refused where an instance is made, and a module's root may name only `Object` or `Value`, or write `Object <: Value`. §6.3.2's simultaneous slots, through pull futures: a clause's slots initialize simultaneously, so well-founded mutual recursion between them resolves ("Simultaneous slots"). A value is one tagged word, Spur's: a handle, a 61-bit `SmallInteger`, a `SmallFloat64` or `nil`, `true` and `false`, every other number a `LargeInteger`, a boxed `Float` or a `Fraction`; a heap object stores bare eight-byte words, and a store checks the value it stores ("Values and objects"). Every object is a header word, a class word and its fields, its kind a format -- class objects, closures and reified frames included (road step 3a, "The object layout") -- in one VM-owned object space, a handle the object's 64-bit byte offset (road step 3b, "Handles into the object space"); `Symbol` is a subclass of `String` | `become:` as an eager bulk forward, reached only through mirrors (road step 5, "Planned: `become:` as an eager bulk forward") |
| Name resolution, lookup, access control, sends | [dispatch.md](dispatch.md) | Built, the inline caches included: primitives as method-table entries (3a) — since M4's step 6 on the VM mirror's class alone, with kernel primitive methods run in `act` without an activation — per-heap caches with a global cache behind them (3b), the five refinements decided before 3c — flat metaclasses, the primary factory as a metaclass entry, the lexical global-cache key, unserialized call-site numbers and the cache's least-recently-used groups — and the dispatch id in the object header (3c), read from the header word's low 22 bits since road step 3a ("The guard in the header word"); M4's step 3 is complete; a §5.7 member set is now keyed by a real class identity, and a setter send skips every local that declares no setter (a parameter or an immutable local) | Open: whether the class side gets a scope of its own ("Open: the class side's scope"); guarding a private lexical site on the mixin rather than its application, once a program has one to measure |
| AST to bytecode: instructions, lowering, inlining | [compiler.md](compiler.md) | Built; call sites numbered per code family, the capture analysis and converted register layout closure conversion needs, an inlined fast path for the nine binary integer leaves, and a kernel mirror send's failure block lowered inline (`SendMirror`) | dropping `IntegerBinary`'s `Jump` |
| Activations, the VM loop | [execution.md](execution.md) | Built, including the trampoline; nothing re-enters the loop from Rust, the loop runs over the frame's state in locals, one entry per stretch between two safepoints, and polls a safepoint, its sends go through the inline caches, and the optimized lowering converts closures — a captured variable is copied into the closure or lives in its scope's indirection vector, and only by-reference code keeps a returning frame's registers; a body is stored and run as eight-byte instructions with side tables, read back as `Instruction`s through a decoded view, each send kind joining its own send path ("The compact instruction encoding") | The JIT tiers, after the representation steps of the road -- an execution model for compiled code, a baseline JIT and an optimizing one ("Planned: JIT tiers") |
| Object lifetime, identity, collection | [memory.md](memory.md) | Built — the memory layer, `crates/ns-memory` ("The memory layer"), and in it the object space: every object end to end in one reservation, byte-offset handles, bump allocation (road step 3b, "The object space"); frames objects in it, the activation stack, the safepoint, tracing by format, host roots, the stress modes, the lifetime brand; two generations (road step 4, "Generations"): a nursery of eden and two survivor spaces below the old space, a copying scavenger with age-based tenuring, a write barrier and remembered set behind the store accessors, large objects allocated old, and a mark-compact full collection that marks in the header and slides the survivors of both generations into the old space through a live-word bitmap; an object is reclaimed once nothing reaches it | Inline allocation ("Planned: inline allocation") |
| Serialization: the packaged application, the project file | [image-format.md](image-format.md) | Built: one artifact, an application packaged just before `main:args:`, carrying every module of a project; a project is the directory its `newspeak.json` describes (`main`, `modules`, `scripts` for `--script`), described by `schemas/newspeak.schema.json`, which the agreement test `crates/ns-cli/tests/project_schema.rs` holds the reader to ("Packaging"); frames are object records, and the decoder sizes every record from its own bytes before it allocates any ("Two passes"); a load tenures once before the graph pass, which allocates old ("The image under generations"); an image names every object the binary owns -- a shipped module's instance, a class its root declares, their metaclasses and levels -- by its declaration, resolved by table lookup against the shipped world the bootstrap builds whole, under a rule of what may be named where, and its header carries that world's fingerprint ("Shipped names") | Suspend and resume ("Planned: suspend and resume") |
| Builtins and the platform | [core-library.md](core-library.md) | **Partial:** the kernel is still being completed to Primordial Soup's core API ("Planned: the kernel completed to Primordial Soup's core API"). Built as Newspeak source over a VM mirror: every builtin method is Newspeak in `crates/ns-runtime/core-library/Kernel.ns` -- a forwarder to one of the mirror's `MIRROR` rows, which are the leaves, or a quick method or ordinary method that reaches no leaf, as all of `True`'s, `False`'s and the nil protocol's are. Its hierarchy is §3.2's and §3.1.1's — `Top`, then `Object`, then `Value` over the value classes with `Number` among them, and `Behavior` over `Class` and `Metaclass` — and `Value` derives `=`, `==` and `hash` from an instance's state. Every class object's class is its own metaclass, its class side applied to `Class`, and every metaclass's class is `Metaclass`; a class object is a value when its enclosing object is. `Object` carries §3.2's accessors for the predefined classes, so a bare `String` is a send answered without an activation. §8.6's exceptions are Newspeak over kernel-private activation objects: `Exception`, `Error`, `MessageNotUnderstood`, `Halt`, `Message`, `ArgumentError`, `CannotReturn` and `CannotInstantiate`, reached through `platform kernel` and the two roots also bare, through `Object`'s accessors, with ANSI's `on:do:`, `ensure:`, `ifCurtailed:`, `return:`, `retry`, `resume:` and `pass`; every VM error and every message nothing understands is signalled, and the depth limit keeps a reserve for signalling its own; `platform kernel` answers `Proxy` and the public `Future` too, whose `computing:` makes the runtime's future, and every core class answers its own `isKindOf…` question, `false` answering one it does not declare. The platform is an instance of the nested `Platform` of the shipped module `crates/ns-runtime/core-library/PlatformFactory.ns`, whose root is booted with the mirror and the kernel instance from the table of shipped modules (`interpreter/core_library.rs`) and never handed out (the factory pattern); its transcript, `collections`, `kernel` and `time` members are lazy slots, its module's classes are handed out by class accessors, and an image names every shipped class by its declaration. `collections` is the facet of a second shipped module, `CollectionsFactory.ns`; every shipped module is built once at every boot, and `PlatformFactory`'s root reaches the collections module through the mirror's `moduleInstanceNamed:`; its `List`, `Map`, `IdentityMap` and `Set` are Newspeak written here, under the kernel's abstract `Collection` with `Array`, each with Primordial Soup's protocol and signalling `ConcurrentModification` when changed while iterated. A `String` is its code points, one byte or four each ("Strings are code points in two widths"), with Primordial Soup's protocol in code points but for the UTF-8 conversions, and a NaN equals a NaN. Integers are unbounded, as §5.1.1 requires: one that does not fit a `SmallInteger`'s 61 bits is a `LargeInteger`, a kernel subclass of `Integer` whose arithmetic (`crates/ns-runtime/src/arbitrary_integer/`) is adapted from `num-bigint` and `num-integer` ("Large integers"). The numbers are a symmetric tower of `Integer`, `LargeInteger`, `Float` and `Fraction`, a builtin made only in normal form: any two compare by exact value and hash alike when equal, arithmetic without a `Float` is exact, so `7 / 2` is `(7/2)`, and `Number`, `Integer`, `Float` and `Fraction` carry Primordial Soup's whole protocol ("The numeric tower") | real bodies for `whileTrue:`, `whileFalse:` and `do:` before the baseline JIT (road step 8) |
| Debugger, mirrors, edit-and-continue, actors | [liveness-and-reflection.md](liveness-and-reflection.md) | Planned | mirrors and atomic installation (road step 5, "Planned: road step 5, mirrors and atomic installation"); the debugger, edit-and-continue and actors after it |
| Editor support and the analyzer: the language server, the JetBrains plugin, `ns-next-analyzer` | [editor-support.md](editor-support.md) | Built — highlighting, errors as you type, code folding and a structure view with breadcrumbs in every commercial JetBrains IDE from 2026.1.4, kept through syntax errors; the server bundled for Windows x64. For `newspeak.json`, the schema from `schemas/`, and a "Newspeak" run configuration with play buttons that run or benchmark the project with the `ns-next` the settings name; a module `ns-next` can run from its source gets a play button beside its `main:args:` and "Run" in its context menu, from the document symbols the structure view shares. The server is `ns-next-analyzer lsp`; the same binary parses (`parse`, `parse-corpus`), prints a compiled body (`disasm`, [compiler.md](compiler.md)) and an image's structure (`inspect`, [image-format.md](image-format.md)), and never runs a program; `fmt` formats sources and the server answers `textDocument/formatting` ([formatting.md](formatting.md)) | range formatting and formatting on type, go to definition and find usages, more platforms; a heap dump and inspection of JIT-compiled code as further analyzer commands |

Real programs to measure the runtime on, beside the microbenchmarks under
`goldens/tool/bench/`, live in `benchmarks/`: today
`benchmarks/are-we-fast-yet/`, the fourteen Are We Fast Yet benchmarks,
Primordial Soup's Ton80 Richards and DeltaBlue, and their harness, which
[execution.md](execution.md), "The Are We Fast Yet benchmarks", describes and
`goldens/tool/awfy/` verifies.

## Architecture

Six crates, with the name-resolution pass as the seam the bytecode compiler
plugs into, and the JetBrains plugin beside them:

```
crates/
  ns-syntax/    lexer, tokens, a lossless AST with its comments attached,
                spans, diagnostics, the printer (print_module, format);
                for an editor, parse_for_editor: a recovered module and
                its comments
  ns-runtime/   resolver, object model, dispatch, compiler, VM, the leaves, image;
                core-library/Kernel.ns, the core library in Newspeak
  ns-cli/       the `ns-next` binary, which runs programs: run, run-corpus,
                load, bench, build
  ns-tooling/   what both binaries share: loading a source, collecting `.ns`
                files, printing diagnostics
  ns-analyzer/  the `ns-next-analyzer` binary, which looks without running:
                lsp (the language server: diagnostics, semantic tokens,
                fold ranges, document symbols, formatting), dump-tokens,
                parse, parse-corpus, disasm, inspect, and fmt, which
                rewrites sources in the formatter's layout
  ns-memory/    reserved, committed and protected memory; the only crate with unsafe
editors/
  jetbrains/    the JetBrains plugin: a Kotlin lexer mirroring ns-syntax's,
                highlighting, a structure view, and the connection to
                ns-next-analyzer over the native LSP API
```

Inside `ns-runtime`, one file cuts across every other: `heap.rs`. It owns the
object space (`heap/space.rs`), every object end to end in one reservation of
`ns-memory` -- header word, class word, fields -- and the activation stack
every running activation lives on; an activation becomes a frame object in
the space only when something reifies it. `ObjectReference`,
`ClassReference` and `FrameReference` are opaque handles, each an object's
64-bit byte offset in the space, and `ActivationReference` an index into the
stack; each one's inner field is private to the `heap` module (`heap.rs` and
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
                                and instantiated with the mirror
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
argument parser, the dispatch to the commands and the thread every command
runs on — a thread of its own because the symbol interner is thread-local,
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
for byte and exit status included (`crates/ns-cli/tests/run_goldens.rs`).
That runner is a `harness = false` binary on `libtest-mimic`, one test per
golden per lowering named by its path below `goldens/` --
`golden::optimized::<path>` and `golden::reference::<path>`, with `<path>`
such as `language/block/blocks_and_return`, so nextest runs the two lowerings
of a golden in parallel rather than one trial running both in sequence -- and
in a debug build `golden_with_a_two_entry_global_cache::<path>` as
well -- so the suite lists each golden, reports every failure with the expected
and actual stream, runs them in parallel, and runs one or a family by name
(`cargo nextest run -p ns-cli --test run_goldens lazy`); a skipped golden is
listed as skipped, and `--run-ignored all` runs it. The build/load round
trip is the same, one test per golden
(`crates/ns-cli/tests/golden_round_trips.rs`).
Every golden that is not skipped, the ones recorded as raising included, also
goes through the round trip, `build` then `load`, which holds the started
image's three streams against what `run` answers on the source; a golden
whose assembly fails must fail its `build` with `run`'s status and report.

A golden's root class is instantiated with `new`, and a project golden's
root with `packageUsing:` its manifest; either reaches the platform only as
`main:args:`'s first argument (§8.4), as every program does
([image-format.md](image-format.md), "Packaging"). A project golden is
the directory `goldens/<category>/<name>/`, its `newspeak.json` in it and
`<name>.toml` beside it; its modules live in the directory too, or in a tree
several goldens share outside `goldens/`, which its `newspeak.json` names by
relative paths: each `goldens/tool/awfy/` golden is a directory holding only a
`newspeak.json` over `benchmarks/are-we-fast-yet/`. The modules are not
goldens themselves, and `goldens/tool/manifest/` pins the manifest.

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

| Directory | What belongs in it |
|---|---|
| `language/` | what the compiler, the resolver and the object model decide |
| `language/syntax/` | the grammar's rules: the unreserved words, literals and their signs, symbols' forms, precedence, cascades, type parameters |
| `language/module/` | a module's root: every kind of root the root rule refuses, a module with a language id and no category, and `main:args:`'s arguments |
| `language/builtin-names/` | which names a program sees: no `Character`, no `Top`, no mirror class, and a member of a program's own named like a builtin, which wins |
| `language/class/` | classes and their objects |
| `language/class/instantiation/` | factories, superclass clauses and their arguments, the initializer and the order instantiation runs in |
| `language/class/class-side/` | class sides and metaclasses: an implicit class side, `outer` from one, class-side `super` reaching `Class`, a class side's `doesNotUnderstand:`, the class of a class and of a metaclass, their names, the factory as a class-side member, and the refusal to instantiate a class object |
| `language/class/nesting/` | nested classes: `outer`, redeclaration, overriding, nested `super`, a slot overriding a nested class |
| `language/class/mixin/` | §6.2's mixin application: lookup and `super` through the applied levels, `outer` from an applied mixin, the order of several `<:` clauses, the initializer order, the level's factory selector, the errors a clause raises, and a module that declares itself a value with `<: Value` |
| `language/slot/` | slots |
| `language/slot/accessor/` | slot accessors and setters, an immutable slot, and §6.3.3's lazy slots |
| `language/slot/simultaneous/` | §6.3.2's simultaneous slots and temporaries: futures, data cycles, ill-founded recursion and an abandoned clause |
| `language/method/` | a method body: its answer without a return, its parameters and its temporaries (§3.5, §3.6, §5.1.8) |
| `language/send/` | sends and lookup: `doesNotUnderstand:`, a native declining to `Object`'s method, public, protected and private access (§5.5, §6.3.1), lexical before inherited (§5.7), `self` and `super` sends |
| `language/block/` | blocks and closures: what a block captures -- copied variables, indirection vectors, the fallback closures of an inlined conditional --, a closure outliving its method, non-local return and its `cannotReturn:`, an immutable block parameter |
| `language/control-flow/` | conditionals, `and:`/`or:`, loops and `Number`'s iteration, inlined and through their fallback, and a non-Boolean receiver or condition |
| `language/evaluation-order/` | an operand or an assignment's value that keeps what it evaluated to while a later send writes its variable |
| `language/expression/` | what surrounds an expression without changing what it means: parentheses around an inlinable conditional's receiver and blocks |
| `library/` | the core library: `Kernel.ns`, `PlatformFactory.ns` and `CollectionsFactory.ns` |
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
| `runtime/` | what the VM guarantees beyond the language's rules |
| `runtime/cache/` | an outcome an inline cache could change, each added before any cache existed |
| `runtime/depth-limit/` | the activation limit: what still runs at 1000 deep, what overflows, the reserve a handler runs in, and a recursion through the core library the two lowerings count alike |
| `tool/` | the tools around `ns-next run`: projects and the benchmark harnesses |
| `tool/manifest/` | projects: a root assembled with `packageUsing:` and the manifest of the other modules, a name the manifest does not hold, and a manifest the application keeps through `build` and `load`; each project's modules in the directory of its name |
| `tool/awfy/` | the Are We Fast Yet harness, verified; its sources live in `benchmarks/are-we-fast-yet/` |
| `tool/bench/` | the measurement protocol's own sources, the ones `ns-next bench` times |

A golden named `spec_<section>_...` pins one named rule of
`newspeak-spec.pdf`, its section in the file name; it lives with its theme.

The goldens are a tree. A directory that holds files is a category and holds
goldens only; one that holds none groups categories or further groups. The
set is found by walking it — no list, no count, so adding a golden needs no
edit anywhere, and `cargo nextest run -p ns-cli --test run_goldens
library/number` runs one subtree. A project golden is a
directory holding a `newspeak.json`, the one directory a category may hold;
its modules live in it, or in a shared tree outside `goldens/`, as
`tool/awfy/`'s do; neither's contents are goldens, and neither is walked. A file at the
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
module. The `awfy/` goldens share one root, the harness, so theirs is the
harness's own header in `benchmarks/are-we-fast-yet/AreWeFastYet.ns`, which
says what an untimed run prints and gives the usage line the refusals pin;
their file names say which case each is. Every `goldens/tool/awfy/*/newspeak.json` lists
every module of `benchmarks/are-we-fast-yet/newspeak.json`, because the
harness names each benchmark when it is instantiated; a new benchmark is added
to the harness tuple, the project file, each `awfy` golden's `newspeak.json`,
`all_benchmarks_verify.toml` and the name list in `crates/ns-cli/tests/run.rs`'s
`a_timed_run_prints_a_score_for_every_benchmark`. The `.toml` holds the recorded
answer and nothing else.

## Gates

These are non-regression gates, not suggestions. Each has caught a real
regression.

```bash
cargo nextest run --workspace            # includes every golden, under both lowerings
cargo test --workspace --doc             # the doctests, which nextest does not run
NS_NEXT_STRESS_MOVES=1 cargo nextest run --workspace   # the same, moving everything at the run loop's safepoints
NS_NEXT_STRESS_CACHES=1 cargo nextest run --workspace  # the same, vacating every inline cache at every safepoint
NS_NEXT_STRESS_COLLECT=1 cargo nextest run --workspace # the same, collecting at every safepoint
NS_NEXT_STRESS_SCAVENGE=1 cargo nextest run --workspace # the same, scavenging at every safepoint after a check of the remembered set
NS_NEXT_STRESS_WIDE=1 cargo nextest run --workspace    # the same, every instruction in its wide form
# (the finishing-a-batch skill runs the five stress modes at once, each into its own log)
# the suite above is every crate's, ns-analyzer's included: its disasm and inspect tests guard the compiler's output and the image format
cargo clippy --workspace --all-targets   # must be warning-free, the naming lint included
cargo fmt --all                          # before every commit
cargo run -p ns-analyzer -- parse-corpus corpus/modules   # parsed 211, failed 0
cargo run -p ns-cli -- run-corpus corpus/modules          # ran 6, failed 0, skipped 205 -- every row under both lowerings
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules   # the same report
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules  # the same report
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules # the same report
NS_NEXT_STRESS_SCAVENGE=1 cargo run -p ns-cli -- run-corpus corpus/modules # the same report
NS_NEXT_STRESS_WIDE=1 cargo run -p ns-cli -- run-corpus corpus/modules    # the same report
./editors/jetbrains/gradlew -p editors/jetbrains check   # the plugin's own gate, per commit that touches it
./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin   # once at the end of a batch that touched the plugin: downloads several IDEs
```

The five stress modes, ten gate invocations, are Git Bash syntax and do
anything only in a debug build ([memory.md](memory.md), "The move stress
mode", "The gates the collector needs", "Generations";
[dispatch.md](dispatch.md), "Invalidation"; [execution.md](execution.md),
"The compact instruction encoding"). Clippy's gate
includes `min_ident_chars` (`clippy.toml`), which refuses identifiers of one
or two characters: names are written out
(`CLAUDE.md`, "Conventions").

Never judge performance from a debug build; `cargo build --workspace --release`
is the one to measure, and every binary a measurement compares — the current
tree included — is built with the same explicit `RUSTFLAGS`, which pin
function alignment so that code placement cannot confound the comparison;
the shipped build is unpinned ([execution.md](execution.md), "What removing
the re-entries moved", the protocol).

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
| There is no mirror-based reflection: no mirror a program can obtain, no MirrorBuilder and no atomic install, so a running program can neither examine nor change its classes, methods or objects reflectively. Road step 5 closes it but for changing the class of an individual object and access to the current activation, which it defers | §8.2 | [liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors and MirrorBuilder" |
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
