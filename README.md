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
garbage collector, a core library written in Newspeak over one VM mirror, and a
source-free image format.
Every `.ns` file in the vendored corpus parses.

Not built yet, and the limits worth knowing before trying it on real code:

- **No numeric tower.** An `Integer` answers `//` but not `/` or `\\`, `1 = 1.0` is
  `false`, and a non-integral literal is a binary float.
- **The platform is minimal**, and widens only when a whitelisted corpus module needs
  something: a transcript, the kernel's classes and exceptions, and a collections
  module with `List`. Six of the 211 corpus modules run end to end; the rest ask for
  collections, streams and mirrors that do not exist yet.
- **No actors, no reflection API, no debugger, no JIT.** An eventual send runs
  synchronously and answers its value rather than a promise, a pattern literal
  evaluates to **nil**, and Typed-Newspeak annotations parse and are discarded.

`docs/design/` describes how the system works and how much of each aspect exists;
`docs/PROGRESS.md` is the milestone chronology beside it, and `docs/OPEN-WORK.md`
lists everything that is still to be done.

## Requirements

Rust 1.98.0, pinned by `rust-toolchain.toml` — `rustup` picks it up automatically.
The test suites run under `cargo-nextest`, installed once per machine with
`cargo install cargo-nextest --locked`. The project has no external runtime
dependencies.

## Build and test

```bash
# build every crate; the ns-next binary lands in target/debug
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
cargo run -p ns-cli -- parse-corpus corpus/modules
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
`ns-next.exe` is one self-contained file: it needs no Rust installation and no VC++
redistributable on the machine that runs it.

`goldens/` holds end-to-end programs, each a `.ns` source and a `.toml` beside it
recording the exit status, stdout and stderr that source must produce, byte for byte —
under both lowerings, the optimized one and the reference lowering it is checked
against (`docs/design/README.md`, "The goldens"). The corpus is a vendored set of real
Newspeak modules, kept as a non-regression gate for the parser; the six on
`corpus/runnable.toml` also run end to end, under both lowerings.

## Usage

```bash
# run a file end to end: parse, resolve, bootstrap, load, instantiate, send the entry selector
cargo run -p ns-cli -- run goldens/language/hello.ns
```

The entry selector defaults to `main:args:`; `--send <selector>` picks another one.
The program's arguments follow `--` (`run app.ns -- first second`) and reach
`main:args:` as an `Array` of `String`s; `load`, `snapshot` and `bench` take them
the same way.
`--lowering reference` compiles with the naive reference lowering instead of the
optimized one; `run`, `snapshot`, `build` and `disasm` take it.

```bash
# print one compiled body; nothing is instantiated or run. Takes a .ns file,
# which it compiles, or an artifact, whose bodies it decodes instead
cargo run -p ns-cli -- disasm goldens/language/blocks_and_return.ns find:in:
```

```bash
# serialize a running world to an image, then load it back and send it a message
cargo run -p ns-cli -- snapshot goldens/language/instance_init.ns world.img
cargo run -p ns-cli -- load world.img --send main:args:
```

```bash
# check that a file parses; --ast prints the syntax tree
cargo run -p ns-cli -- parse goldens/language/hello.ns
```

```bash
# compile to a stateless deployment binary: code and structure, no instance
# state. `build` never instantiates; `load` does, against its own loading
# world's platform, then sends the recorded entry (main:args: here, since the
# root class declares it) -- override with --send
cargo run -p ns-cli -- build goldens/language/arithmetic.ns app.nsb
cargo run -p ns-cli -- load app.nsb
```

```bash
# look inside an artifact without instantiating or running it: header flags,
# section offsets, the symbol table, every body in the code table, and the
# mixin table. Takes a stateless binary or an image written by `snapshot`
cargo run -p ns-cli -- inspect app.nsb
# everything, the way `javap -v` renders a class file: mixin rows expanded and
# every body disassembled, plus an image's object graph read record by record
cargo run -p ns-cli -- inspect world.img --full
```

```bash
# time one entry send; measure with --release, never a debug build
cargo run --release -p ns-cli -- bench goldens/bench/bench_loop.ns --iterations 30
```

`ns-next <command> --help` documents the remaining flags.

## Repository layout

```
crates/ns-syntax     lexer, parser, AST, spans
crates/ns-runtime    resolver, object model, dispatch, compiler, VM, core library, image format
crates/ns-cli        the `ns-next` binary
crates/ns-lsp        the `ns-next-lsp` binary: a language server for `.ns` files
crates/ns-memory     reserved, committed and protected memory: the only crate with unsafe
editors/jetbrains/   the JetBrains plugin, over ns-next-lsp (see its README.md)
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
