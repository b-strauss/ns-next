# ns-next

A native runtime for [Newspeak](https://newspeaklanguage.org/), written in Rust.

`ns-next` parses and runs Newspeak source directly: a hand-written lexer and parser for
the Newspeak3 textual syntax, a name resolver implementing Newspeak's lexical, `outer`
and `super` scoping, and a register-based bytecode compiler with its own VM. A running
world can be serialized to a versioned image and loaded back.

## Status

Working today: the full frontend, Newspeak's real name resolution, nested classes and
mixins with a class object per enclosing instance, the bytecode compiler and VM, and a
source-free image format. Every `.ns` file in the vendored corpus parses but one.

Not built yet, and the limits worth knowing before trying it on real code:

- **No garbage collector.** Objects live in an arena of 32-bit handles and nothing
  reclaims a slot yet, so every object a process creates is kept until it exits.
- **Integers are `i64`.** Overflow raises a Newspeak error; there are no bignums.
- **The platform is minimal**, and widens only when a whitelisted corpus module needs
  something. Six of the 211 corpus modules run end to end; the rest ask for
  collections, streams and mirrors that do not exist yet.
- **No actors, no reflection API, no debugger, no JIT.** Eventual sends and
  Typed-Newspeak annotations parse and are discarded.

`docs/design/` describes how the system works and how much of each aspect exists;
`docs/PROGRESS.md` is the milestone chronology beside it.

## Requirements

Rust 1.98.0, pinned by `rust-toolchain.toml` — `rustup` picks it up automatically. No
other tooling is needed; the project has no external runtime dependencies.

## Build and test

```bash
# build every crate; the ns-next binary lands in target/debug
cargo build --workspace
# the full suite, including the golden program outcomes
cargo test --workspace
# formatting
cargo fmt --all
# lints, must be warning-free
cargo clippy --workspace --all-targets
# parser corpus gate: must report "parsed 210, failed 0, expected-fail 1, stale 0"
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

Every fixture is checked against its recorded `.expected` stdout and an empty stderr,
and `crates/ns-cli/tests/program_outcomes.rs` pins the exit status, stdout and stderr
of programs that cannot be fixtures, byte for byte. The corpus is a vendored set of real
Newspeak modules, kept as a non-regression gate for the parser.

## Usage

```bash
# run a file end to end: parse, resolve, bootstrap, load, instantiate, send the entry selector
cargo run -p ns-cli -- run tests-fixtures/hello.ns
```

The entry selector defaults to `main:args:`; `--send <selector>` picks another one.

```bash
# print one compiled body; nothing is instantiated or run. Takes a .ns file,
# which it compiles, or an artifact, whose bodies it decodes instead
cargo run -p ns-cli -- disasm tests-fixtures/blocks_and_return.ns find:in:
```

```bash
# serialize a running world to an image, then load it back and send it a message
cargo run -p ns-cli -- snapshot tests-fixtures/instance_init.ns world.img
cargo run -p ns-cli -- load world.img --send main:args:
```

```bash
# check that a file parses; --ast prints the syntax tree
cargo run -p ns-cli -- parse tests-fixtures/hello.ns
```

```bash
# compile to a stateless deployment binary: code and structure, no instance
# state. `build` never instantiates; `load` does, against its own loading
# world's platform, then sends the recorded entry (main:args: here, since the
# root class declares it) -- override with --send
cargo run -p ns-cli -- build tests-fixtures/arithmetic.ns app.nsb
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
cargo run --release -p ns-cli -- bench tests-fixtures/bench_loop.ns --iterations 30
```

`ns-next <command> --help` documents the remaining flags.

## Repository layout

```
crates/ns-syntax     lexer, parser, AST, spans
crates/ns-runtime    resolver, object model, dispatch, compiler, VM, core library, image format
crates/ns-cli        the `ns-next` binary
corpus/              vendored Newspeak modules used as a parser corpus
tests-fixtures/      end-to-end programs with their expected output
prior-art/           reference implementations as submodules, read but never built (see PROVENANCE.md)
docs/design/         living design documents — start at its README.md
docs/PROGRESS.md     milestone chronology
docs/superpowers/    implementation plans, kept as execution records
docs/references/     the Newspeak papers the design argues from (third-party, see PROVENANCE.md)
```

`prior-art/` holds the canonical Newspeak implementation, its benchmark suite,
Primordial Soup, SOMns, Nova and the OpenSmalltalk VM, so that a design decision
can be grounded in how a working system solved the same problem. They are
submodules, so a plain clone does not fetch them and nothing in the build, the
tests or either gate depends on them; `git submodule update --init` gets them
when you want them — though `opensmalltalk-vm` alone is about 255 MB, so
initializing by name is usually the better move.

## License

MIT — see [LICENSE](LICENSE). The vendored corpus under `corpus/` is third-party
material with its own terms — see `corpus/PROVENANCE.md` and `corpus/UPSTREAM-LICENSE`.
The submodules under `prior-art/` each keep their own license in their own tree;
none of their code is vendored here — see `prior-art/PROVENANCE.md`.
