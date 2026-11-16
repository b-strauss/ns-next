# ns-next

A native runtime for [Newspeak](https://newspeaklanguage.org/), written in Rust.

`ns-next` parses and runs Newspeak source directly: a hand-written lexer and parser for
the Newspeak3 textual syntax, a name resolver implementing Newspeak's lexical, `outer`
and `super` scoping, and a register-based bytecode compiler with its own VM. A running
world can be serialized to a versioned image and loaded back.

## Status

Working today: the specification's grammar (§4.2 to §8.1), Newspeak's real name
resolution, nested classes, §6.2's mixin application with a class object per enclosing
instance, metaclasses, value objects, simultaneous and lazy slots, §8.6's exceptions,
integers of any size, the bytecode compiler and a register VM with inline caches, a
second, deliberately naive lowering the compiler is checked against, a mark-compact
garbage collector, a core library written in Newspeak over one VM mirror, a
source-free image format, a baseline JIT for x64 and AArch64, and the first two
stages of an optimizing JIT above it, which keeps values in machine registers
across a body, compiles a method a copy per receiver class and level, reading its slots
at fixed offsets, and traps where the feedback says a path never ran; it
inlines nothing yet.
Every `.ns` file in the vendored corpus parses.

Not built yet, and the limits worth knowing before trying it on real code:

- **No exact non-integral literal.** The numbers are a symmetric tower --
  `7 / 2` is the `Fraction` `(7/2)` and `1 = 1.0` is `true` -- but there is no
  fraction literal, and a non-integral literal such as `0.1` is a binary float,
  not the exact decimal §5.1.1 names (a recorded deviation).
- **The platform is minimal**, and widens only when a whitelisted corpus module or a
  program under `benchmarks/` needs something: a transcript, the kernel's classes
  and exceptions, a collections module with `List`, and `time` with its
  `Stopwatch`. Six of the 211 corpus modules run end to end; the rest ask for
  collections, streams and mirrors that do not exist yet. The fourteen Are We Fast
  Yet benchmarks and Primordial Soup's Ton80 Richards and DeltaBlue run under
  their harness in `benchmarks/are-we-fast-yet/newspeak/`.
- **No actors.** An eventual send runs synchronously and answers its value
  rather than a promise, a pattern literal evaluates to **nil**, and
  Typed-Newspeak annotations parse and are discarded. Mirrors read a program
  and a MirrorBuilder edits and installs it (`platform mirrors`).
- **A debugger without evaluation yet.** `ns-next run --debug` speaks the
  Debug Adapter Protocol, and the JetBrains plugin debugs with it: line,
  method and exception breakpoints, steps, the stack and its values, the
  shipped modules as a library; evaluating an expression, watches,
  conditions and setting a variable come next (`docs/design/debugger.md`).

`docs/design/` describes how the system works and how much of each aspect exists;
`docs/PROGRESS.md` is the milestone chronology beside it, and `docs/OPEN-WORK.md`
lists everything that is still to be done.

## Requirements

Rust 1.100.0 (edition 2024), pinned by `rust-toolchain.toml` — `rustup` picks it up automatically.
The test suites run under `cargo-nextest`, installed once per machine with
`cargo install cargo-nextest --locked`. The `codegen` gate needs `cargo-show-asm`
(`cargo install cargo-show-asm --locked`) and the target `x86_64-pc-windows-msvc`;
the `benchmarks` gate needs a JDK and a `python` on `PATH`. The project has no external runtime
dependencies. On x86_64 the binaries require an x86-64-v2 processor (SSE4.2,
POPCNT) and say so at start when theirs lacks one. On Windows it needs
Windows 10 version 1803 or Windows Server 2019, for the placeholders the JIT's
code zone is mapped into, and the zone commits at least 1 MiB, its chunk; on an
older Windows the JIT is off, which `run` and `load` say on stderr and
`bench` refuses.

## Build and test

```bash
# build every crate; the ns-next and ns-next-analyzer binaries land in
# target/debug
cargo build --workspace
# the full suite, including every golden under both lowerings, each golden a
# test of its own; nextest runs the tests of every binary in parallel
cargo nextest run --workspace
# the doctests, which nextest does not run
cargo test --workspace --doc
# the goldens alone, or the ones whose name contains a word
cargo nextest run -p ns-cli --test run_goldens
cargo nextest run -p ns-cli --test run_goldens lazy
# the skipped goldens, each named -- the reason it is not run yet is the
# `skipped` line of its .toml
cargo nextest list -p ns-cli --test run_goldens --run-ignored only
# formatting
cargo fmt --all
# lints, must be warning-free
cargo clippy --workspace --all-targets
# parser corpus gate: must report "parsed 211, failed 0"
cargo run -p ns-analyzer -- parse-corpus corpus/modules
# runnable corpus gate: must report "ran 6, failed 0, skipped 205"
cargo run -p ns-cli -- run-corpus corpus/modules
```

An optimized build, which is the one to measure — the debug build is slow enough to
mislead:

```bash
# the binary lands in target/release
cargo build --workspace --release
```

On Windows the C runtime is linked statically (see `.cargo/config.toml`), so
`ns-next.exe` and `ns-next-analyzer.exe` are each one self-contained file: they
need no Rust installation and no VC++ redistributable on the machine that runs
them.

`goldens/` holds end-to-end programs, each a `.ns` source or a project directory
holding a `newspeak.json`, and a `.toml` beside it recording the exit status, stdout and stderr that program must
produce, byte for byte —
under both lowerings, the optimized one and the reference lowering it is checked
against (`docs/design/README.md`, "The goldens"). The corpus is a vendored set of real
Newspeak modules, kept as a non-regression gate for the parser; the six on
`corpus/runnable.toml` also run end to end, under both lowerings.

## The gates and the repository's tasks

`cargo xtask` runs the repository's own tasks (`xtask/`). Each gate group is
one call; several groups also run in one call, one after another, and the
first that fails stops it. `CLAUDE.md`, "The gates", says when each runs.

```bash
# every commit: cargo fmt --all, clippy warning-free, the suite with every
# golden under both lowerings, the doctests, the parser corpus and the
# plugin's check
cargo xtask gates commit
# a commit touching crates/ns-analyzer: the hints' behaviour oracle
cargo xtask gates analyzer
# every golden through build then load
cargo xtask gates images
# every golden with a two-entry global cache
cargo xtask gates caches
# a commit touching crates/ns-memory: its clippy for Linux, macOS and Windows on ARM
cargo xtask gates memory
# the benchmark programs verified at full size, and their Java and Python
# counterparts; needs a JDK and a `python` on PATH
cargo xtask gates benchmarks
# the suite without debug assertions, which runs the size tests
cargo xtask gates release-checks
# the hot path's machine code against its budgets; needs cargo-show-asm
cargo xtask gates codegen
# the suite under the fifteen stress modes (MOVES, CACHES, COLLECT, SCAVENGE,
# WIDE, SUSPEND, JIT, EXIT, ZONE, OPTIMIZE, DEOPT, INVALIDATE, PAUSE,
# BREAKPOINTS and STEP) and
# once without the JIT, each into .temp/stress-<MODE>.log (NO_JIT for the
# last), then run-corpus plain and under each mode
cargo xtask gates stress
# the JetBrains plugin against several IDEs, which it downloads
cargo xtask gates plugin-verification
# images, caches, benchmarks, release-checks, codegen, stress, linux and
# aarch64, for a batch that changed the runtime
cargo xtask gates end-of-batch
# write the embedded shipped code (every gate call also does this first)
cargo xtask shipped-code
# only say whether the embedded shipped code is current
cargo xtask shipped-code --check
# the codegen budgets against another checkout, e.g. an exported older commit
cargo xtask codegen .temp/stage2
```

## Usage

```bash
# run a file end to end: parse, resolve, bootstrap, compile, instantiate the
# root class with `new`, then send it `main: platform args:`
cargo run -p ns-cli -- run goldens/library/platform/hello.ns
```

A program is one `.ns` file, whose root class is instantiated with `new`, or
a project of several modules, whose root is instantiated with `packageUsing:`
a manifest of the others. Either way the application is then sent `main:
platform args:`, the only way the platform reaches it. A project is a
directory holding a `newspeak.json`, which names its application module,
`main`, and its other modules; paths are relative to it, with either
separator. `run`, `bench` and `build` take the directory or its
`newspeak.json`, and `run` and `bench` the current directory when given no
program:

```json
{
  "main": "App.ns",
  "modules": ["Greeting.ns", "Counter.ns"],
  "scripts": {
    "loud": "--loud 'the World'"
  }
}
```

`App.ns` declares `class App packageUsing: manifest = (...)` and reaches each
module by the name its source declares, `manifest Greeting`
(`goldens/tool/manifest/` has three such projects). A script is a command
line for `main:args:`, split into arguments as a POSIX shell splits one, on
every platform and with nothing expanded: `ns-next run --script loud` hands
it `--loud` and `the World`, and arguments after `--` follow them; `bench`
takes `--script` too. The file's schema is `schemas/newspeak.schema.json`;
the JetBrains plugin gives it to every `newspeak.json`, and an editor
without the plugin finds it through a `"$schema"` key, which `ns-next` does
not follow.

```bash
cargo run -p ns-cli -- run goldens/tool/manifest/a_project_reaches_its_modules_through_the_manifest
```

The program's arguments follow `--` (`run app.ns -- first second`) and reach
`main:args:` as an `Array` of `String`s; `load` and `bench` take them the
same way. `--lowering reference` compiles with the naive reference lowering
instead of the optimized one; `run` and `build` take it, as does `ns-next-analyzer disasm`.

`ns-next` runs programs; `ns-next-analyzer` looks at them without running
any: it parses, compiles a body to print it, decodes an image to show its
structure, and is the language server behind the editor plugin.

```bash
# print one compiled body; nothing of the program is instantiated or run.
# Takes a .ns file, which it compiles, or an artifact, whose bodies it decodes
# instead
cargo run -p ns-analyzer -- disasm goldens/language/block/blocks_and_return.ns find:in:
```

```bash
# check that a file parses; --ast prints the syntax tree
cargo run -p ns-analyzer -- parse goldens/library/platform/hello.ns
```

```bash
# package the application: assemble it exactly as `run` does -- its factory and
# every initializer run, with no platform in existence -- and write it just
# before `main:args:` as an image. `load` decodes it and sends it
# `main: platform args:` with its own platform. The image carries every
# module's source, which `load --print-source` prints; --strip-source leaves it out
cargo run -p ns-cli -- build goldens/language/class/instantiation/instance_init.ns app.img
cargo run -p ns-cli -- load app.img
```

```bash
# look inside an artifact without loading or running it: header flags,
# section offsets, the symbol table, every body in the code table, the mixin
# table and the modules
cargo run -p ns-analyzer -- inspect app.img
# everything, the way `javap -v` renders a class file: mixin rows expanded,
# every body disassembled, and the object graph read record by record
cargo run -p ns-analyzer -- inspect app.img --full
```

```bash
# time main:args: alone, a fresh application assembled untimed for every
# sample; measure with --release, never a debug build
cargo run --release -p ns-cli -- bench benchmarks/micro/bench_loop.ns --iterations 30
```

The Are We Fast Yet benchmarks are one project; a benchmark's name after `--`
picks one, and without a name all sixteen run in turn
(`docs/design/execution.md`, "The Are We Fast Yet benchmarks"):

```bash
# run each benchmark once and verify its result: prints `<Name>: ok`
cargo run --release -p ns-cli -- run benchmarks/are-we-fast-yet/newspeak -- Richards
# time one run of the benchmark: median, minimum and maximum of ten
cargo run --release -p ns-cli -- bench benchmarks/are-we-fast-yet/newspeak -- Richards
# let the harness measure itself: runs per second after a warm-up
cargo run --release -p ns-cli -- run benchmarks/are-we-fast-yet/newspeak -- --timed
# the same, as tests: every benchmark verified at its full size
cargo nextest run -p ns-cli --profile benchmarks
```

`ns-next <command> --help` and `ns-next-analyzer <command> --help` document the
remaining flags.

## Repository layout

```
crates/ns-syntax     lexer, parser, AST, spans
crates/ns-runtime    resolver, object model, dispatch, compiler, VM, core library, image format
crates/ns-cli        the `ns-next` binary: runs, packages and times programs
crates/ns-analyzer   the `ns-next-analyzer` binary: the language server, parse, disasm, inspect; never runs a program
crates/ns-tooling    what both binaries share: loading and resolving a source, collecting `.ns` files, diagnostics
crates/ns-memory     reserved, committed and protected memory: the only crate with unsafe
editors/jetbrains/   the JetBrains plugin, over ns-next-analyzer (see its README.md)
corpus/              vendored Newspeak modules used as a parser corpus
goldens/             end-to-end programs, each with its recorded outcome
prior-art/           reference implementations as submodules, read but never built (see PROVENANCE.md)
docs/design/         living design documents — start at its README.md
docs/PROGRESS.md     milestone chronology
docs/OPEN-WORK.md    everything still to be done, in detail
docs/superpowers/    implementation plans, kept as execution records
docs/references/     the Newspeak papers the design argues from (third-party, see PROVENANCE.md)
```

`prior-art/` holds the canonical Newspeak implementation, its benchmark suite,
Primordial Soup, SOMns, Nova, the OpenSmalltalk VM and Strongtalk, so that a
design decision can be grounded in how a working system solved the same
problem. They are submodules, so a plain clone does not fetch them and nothing
in the build, the tests or either gate depends on them;
`git submodule update --init` gets them when you want them — though
`opensmalltalk-vm` alone is about 255 MB, so initializing by name is usually the
better move.

## License

MIT — see [LICENSE](LICENSE). The vendored corpus under `corpus/` is third-party
material with its own terms — see `corpus/PROVENANCE.md` and `corpus/UPSTREAM-LICENSE`.
The submodules under `prior-art/` each keep their own license in their own tree;
none of their code is vendored here — see `prior-art/PROVENANCE.md`.
